// DJ Platform UI shell.
//
// The shell renders the engine's published state and forwards user intent as commands. It owns no
// audio: the engine is a separate process (ADR-0003), reached over a Unix socket with
// `command-v1`/`state-v1` (DJP-ARCH-004).
//
// Modes:
//   (default)             start the window and connect to the engine
//   --smoke               load the QML scene, count Qt warnings, exit
//   --connect PATH        engine socket (default: $XDG_RUNTIME_DIR/hauler-dj-engine.sock)
//   --self-check          connect, press Play on deck A, require the engine to acknowledge it and
//                         publish `playing`, then exit 0. This is the A06 acceptance, headless.

#include "BridgeClient.h"
#include "Scenarios.h"
#include "UiSettings.h"
#include "LayoutSettings.h"
#include "SessionState.h"
#include "Waveform.h"
#include "WaveformItem.h"
#include "WaveformPainter.h"

#include <QCommandLineOption>
#include <QCommandLineParser>
#include <QElapsedTimer>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QEventLoop>
#include <QGuiApplication>
#include <QPalette>
#include <QDir>
#include <QThread>
#include <QMap>
#include <QTemporaryDir>
#include <QImage>
#include <algorithm>
#include <QQuickWindow>
#include <QLinearGradient>
#include <QPainter>
#include <QQmlApplicationEngine>
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQmlContext>
#include <QString>
#include <QQuickItemGrabResult>
#include <QSharedPointer>
#include <QTimer>

#include <cstdio>
#include <functional>
#include <memory>

namespace {

int g_warningCount = 0;

void collectMessages(QtMsgType type, const QMessageLogContext &context, const QString &message)
{
    if (type != QtDebugMsg && type != QtInfoMsg) {
        ++g_warningCount;
    }
    const char *category = context.category != nullptr ? context.category : "default";
    std::fprintf(stderr, "qt.%s: %s\n", category, qPrintable(message));
}

QString defaultSocketPath()
{
    const QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (!runtime.isEmpty()) {
        return QString::fromLocal8Bit(runtime) + QStringLiteral("/hauler-dj-engine.sock");
    }
    return QStringLiteral("/tmp/hauler-dj-engine.sock");
}

QVariantMap deckById(const QVariantMap &snapshot, const QString &deck)
{
    const QVariantList decks = snapshot.value(QStringLiteral("decks")).toList();
    for (const QVariant &entry : decks) {
        const QVariantMap deckState = entry.toMap();
        if (deckState.value(QStringLiteral("deck")).toString() == deck) {
            return deckState;
        }
    }
    return {};
}

bool deckHasHotcue(const QVariantMap &snapshot, const QString &deck, int slot)
{
    const QVariantList decks = snapshot.value(QStringLiteral("decks")).toList();
    for (const QVariant &entry : decks) {
        const QVariantMap deckState = entry.toMap();
        if (deckState.value(QStringLiteral("deck")).toString() != deck) {
            continue;
        }
        const QVariantList cues = deckState.value(QStringLiteral("hotcues")).toList();
        if (slot >= cues.size()) {
            return false;
        }
        return !cues.at(slot).isNull() && cues.at(slot).isValid();
    }
    return false;
}

bool deckIsPlaying(const QVariantMap &snapshot, const QString &deck)
{
    const QVariant decks = snapshot.value(QStringLiteral("decks"));
    if (!decks.isValid()) {
        return false;
    }
    const QVariantList list = decks.toList();
    for (const QVariant &entry : list) {
        const QVariantMap deckState = entry.toMap();
        if (deckState.value(QStringLiteral("deck")).toString() == deck) {
            return deckState.value(QStringLiteral("playing")).toBool();
        }
    }
    return false;
}

// Connect, ask deck A to play, and require exactly what the contract promises: an ack that says
// "applied at sample N", and a snapshot that then reports the deck as playing. The UI must never
// assume either.
namespace {

// QML items created by a Repeater are not always in the window's QObject tree, so a search by name
// has to walk the visual tree — the one the user actually sees.
QQuickItem *findItem(QQuickItem *root, const QString &name)
{
    if (root == nullptr) {
        return nullptr;
    }
    if (root->objectName() == name) {
        return root;
    }
    const QList<QQuickItem *> children = root->childItems();
    for (QQuickItem *child : children) {
        if (QQuickItem *found = findItem(child, name)) {
            return found;
        }
    }
    return nullptr;
}

} // namespace

// A playing deck must refuse a load, and the refusal has to reach the client as such: the UI's own
// guard is a courtesy, the engine's rule is the one that protects the sound.
static int runBusyCheck(BridgeClient &bridge, const QString &socketPath, const QString &track,
                        const QString &other, int timeoutMs)
{
    QEventLoop loop;
    QTimer watchdog;
    watchdog.setSingleShot(true);
    QObject::connect(&watchdog, &QTimer::timeout, &loop, &QEventLoop::quit);
    watchdog.start(timeoutMs);

    enum Stage { Stopping, Loading, Playing, AskingBusy, AskingAgain, Done };
    // Any deck left playing by an earlier run would make the first load look refused; the check
    // starts from a known state instead of assuming one.
    Stage stage = Stopping;
    bool stopAsked = false;
    int pending = 0;
    bool refusedBusy = false;
    bool secondAccepted = false;

    QObject::connect(&bridge, &BridgeClient::acked, &loop,
                     [&](int id, bool applied, bool rejected, const QString &reason) {
                         if (id != pending) {
                             return;
                         }
                         std::fprintf(stdout, "dj-ui: ack %d applied=%d rejected=%d %s\n", id,
                                      applied ? 1 : 0, rejected ? 1 : 0, qPrintable(reason));
                         if (stage == AskingBusy) {
                             refusedBusy = rejected && reason.contains(QStringLiteral("Busy"));
                             // The deck has to take the track once it is stopped.
                             bridge.play(QStringLiteral("A"), false);
                         } else if (stage == AskingAgain) {
                             secondAccepted = !rejected;
                             stage = Done;
                             loop.quit();
                         }
                     });

    QObject::connect(&bridge, &BridgeClient::snapshotChanged, &loop, [&]() {
        const QVariantList decks = bridge.snapshot().value(QStringLiteral("decks")).toList();
        bool deckAPlaying = false;
        QString loadState;
        for (const QVariant &entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                continue;
            }
            deckAPlaying = deck.value(QStringLiteral("playing")).toBool();
            loadState = deck.value(QStringLiteral("load")).toString();
            if (loadState.isEmpty()) {
                loadState = deck.value(QStringLiteral("load")).toMap().firstKey();
            }
        }
        if (stage == Stopping) {
            if (deckAPlaying && !stopAsked) {
                stopAsked = true;
                bridge.play(QStringLiteral("A"), false);
                return;
            }
            if (!deckAPlaying) {
                stage = Loading;
            }
            return;
        }
        if (stage == Loading) {
            const QString reference = track;
            if (!reference.isEmpty()) {
                pending = bridge.load(QStringLiteral("A"), reference);
                stage = Playing;
                std::fprintf(stdout, "dj-ui: loading A, then playing it\n");
            }
            return;
        }
        if (stage == Playing && !deckAPlaying) {
            pending = bridge.play(QStringLiteral("A"), true);
            return;
        }
        if (stage == Playing && deckAPlaying) {
            stage = AskingBusy;
            pending = bridge.load(QStringLiteral("A"), other.isEmpty() ? track : other);
            std::fprintf(stdout, "dj-ui: deck A is playing, asking it to take another track\n");
            return;
        }
        if (stage == AskingBusy && !deckAPlaying) {
            stage = AskingAgain;
            pending = bridge.load(QStringLiteral("A"), other.isEmpty() ? track : other);
            std::fprintf(stdout, "dj-ui: deck A is stopped, asking again\n");
        }
    });

    QObject::connect(&bridge, &BridgeClient::connectedChanged, &loop, [&]() {
        if (!bridge.isConnected()) {
            return;
        }
        std::fprintf(stdout, "dj-ui: connected to %s for the busy check\n", qPrintable(socketPath));
    });

    bridge.connectToEngine(socketPath);
    loop.exec();

    if (!refusedBusy) {
        std::fprintf(stderr, "dj-ui: a load into a playing deck was not refused as Busy\n");
        return 1;
    }
    if (!secondAccepted) {
        std::fprintf(stderr, "dj-ui: the stopped deck did not accept the load\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: a playing deck refuses a load, a stopped one takes it\n");
    return 0;
}

// End to end: the UI's gain command, the engine applying it, and the number coming back in the
// snapshot. The unit test proves the routing; this proves the wire.
static int runGainCheck(BridgeClient &bridge, const QString &socketPath, int timeoutMs)
{
    QEventLoop loop;
    QTimer watchdog;
    watchdog.setSingleShot(true);
    QObject::connect(&watchdog, &QTimer::timeout, &loop, &QEventLoop::quit);
    watchdog.start(timeoutMs);

    // The two level controls of a channel, end to end and apart: the gain (trim) is the knob before
    // the EQ, the fader is the line on the meter after it. Each is sent in turn, and the other one has
    // to stay where it was — one number shared by both controls is the defect this check exists for.
    enum Phase { AwaitSnapshot, GainSent, FaderSent, Finished } phase = AwaitSnapshot;
    double gainC = 1.0;
    double faderC = 1.0;
    double gainB = 1.0;
    double faderB = 1.0;
    // Deck B is the control hand: it must not follow either command. Its baseline is whatever it holds
    // when the check starts — auto-gain may already have trimmed it to the track it is carrying, and a
    // check that expects unity there measures the loader, not the command.
    double gainBStart = -1.0;
    double faderBStart = -1.0;
    const char *failure = nullptr;

    const auto read = [&]() {
        const QVariantList decks = bridge.snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant &entry : decks) {
            const QVariantMap deck = entry.toMap();
            const QString name = deck.value(QStringLiteral("deck")).toString();
            if (name == QStringLiteral("C")) {
                gainC = deck.value(QStringLiteral("linear_gain")).toDouble();
                faderC = deck.value(QStringLiteral("fader")).toDouble();
            } else if (name == QStringLiteral("B")) {
                gainB = deck.value(QStringLiteral("linear_gain")).toDouble();
                faderB = deck.value(QStringLiteral("fader")).toDouble();
            }
        }
    };

    QObject::connect(&bridge, &BridgeClient::snapshotChanged, &loop, [&]() {
        read();
        switch (phase) {
        case AwaitSnapshot:
            gainBStart = gainB;
            faderBStart = faderB;
            std::fprintf(stdout, "dj-ui: sent gain 0.25 to deck C (deck B starts at %.3f)\n",
                         gainBStart);
            phase = GainSent;
            bridge.setChannelGain(QStringLiteral("C"), 0.25);
            break;
        case GainSent:
            if (std::abs(gainC - 0.25) < 0.01) {
                if (std::abs(faderC - 1.0) > 0.01) {
                    failure = "the gain command moved the channel fader";
                    phase = Finished;
                    loop.quit();
                    return;
                }
                std::fprintf(stdout, "dj-ui: sent fader 0.5 to deck C\n");
                phase = FaderSent;
                bridge.setChannelFader(QStringLiteral("C"), 0.5);
            }
            break;
        case FaderSent:
            if (std::abs(faderC - 0.5) < 0.01) {
                if (std::abs(gainC - 0.25) > 0.01) {
                    failure = "the fader command moved the gain";
                }
                phase = Finished;
                loop.quit();
            }
            break;
        case Finished:
            break;
        }
    });

    QObject::connect(&bridge, &BridgeClient::connectedChanged, &loop, [&]() {
        if (!bridge.isConnected()) {
            return;
        }
        std::fprintf(stdout, "dj-ui: connected to %s for the gain check\n", qPrintable(socketPath));
    });

    bridge.connectToEngine(socketPath);
    loop.exec();
    read();

    if (failure != nullptr) {
        std::fprintf(stderr, "dj-ui: %s (gain %.3f, fader %.3f)\n", failure, gainC, faderC);
        return 1;
    }
    if (phase != Finished || std::abs(gainC - 0.25) > 0.01 || std::abs(faderC - 0.5) > 0.01) {
        std::fprintf(stderr, "dj-ui: deck C did not take both controls (gain %.3f, fader %.3f)\n",
                     gainC, faderC);
        return 1;
    }
    if (gainBStart < 0.0 || std::abs(gainB - gainBStart) > 0.01
        || std::abs(faderB - faderBStart) > 0.01) {
        std::fprintf(stderr, "dj-ui: deck B moved too (gain %.3f -> %.3f, fader %.3f -> %.3f); the "
                             "command must touch only C\n", gainBStart, gainB, faderBStart, faderB);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: gain 0.25 and fader 0.5 reached deck C and stayed apart; deck B untouched "
                 "(gain %.2f, fader %.2f)\n", gainB, faderB);
    bridge.setChannelGain(QStringLiteral("C"), 1.0);
    bridge.setChannelFader(QStringLiteral("C"), 1.0);
    return 0;
}

int runSelfCheck(BridgeClient &bridge, Waveform &waveform, const QString &socketPath,
                 int timeoutMs, const QString &loadPath, const QString &loadSecondPath,
                 bool expectSpread)
{
    QElapsedTimer elapsed;
    elapsed.start();

    bool sawAck = false;
    bool ackApplied = false;
    bool ackRejected = false;
    QString failure;
    bool sawPlaying = false;
    qulonglong appliedAt = 0;
    int commandId = 0;

    QEventLoop loop;
    QTimer watchdog;
    watchdog.setSingleShot(true);
    QObject::connect(&watchdog, &QTimer::timeout, &loop, &QEventLoop::quit);
    watchdog.start(timeoutMs);

    QObject::connect(&bridge, &BridgeClient::connectedChanged, &loop, [&]() {
        if (!bridge.isConnected() || commandId != 0) {
            return;
        }
        std::fprintf(stdout, "dj-ui: connected to %s (epoch %llu)\n", qPrintable(socketPath),
                     static_cast<unsigned long long>(bridge.epoch()));
        if (!loadPath.isEmpty()) {
            // The load stage asks for the file; Play would hit an empty deck and be refused.
            commandId = bridge.load(QStringLiteral("A"), loadPath);
            std::fprintf(stdout, "dj-ui: sent Load(A) as command %d\n", commandId);
            return;
        }
        commandId = bridge.play(QStringLiteral("A"), true);
        std::fprintf(stdout, "dj-ui: sent Play(A) as command %d\n", commandId);
    });

    QObject::connect(&bridge, &BridgeClient::acked, &loop,
                     [&](int id, bool applied, bool rejected, QString reason) {
                         if (id != commandId) {
                             return;
                         }
                         sawAck = true;
                         ackApplied = applied;
                         ackRejected = rejected;
                         if (rejected) {
                             std::fprintf(stderr, "dj-ui: the engine refused the command: %s\n",
                                          qPrintable(reason));
                         }
                     });

    // A stage machine, because the acceptance names two controls: Play and Cue, plus the load
    // that makes a deck worth playing.
    enum Stage { SendingLoad, SendingLoadSecond, SendingPlay, SendingSetCue, SendingJumpCue, Done };
    auto stage = loadPath.isEmpty() ? SendingPlay : SendingLoad;
    QString loadTitle;
    bool sawCue = false;

    QObject::connect(&bridge, &BridgeClient::snapshotChanged, &loop, [&]() {
        const QVariantMap snapshot = bridge.snapshot();
        appliedAt = static_cast<qulonglong>(
            snapshot.value(QStringLiteral("audio_clock_samples")).toDouble());
        // `Accepted` is a valid ack for a load; every stage still demands the effect itself below.
        if (!sawAck || ackRejected) {
            return;
        }
        switch (stage) {
        case SendingLoad: {
            // The state is an enum serialised as a one-key map: {"Loading":{"progress":0}},
            // {"Ready"} or {"Failed":{"message":"..."}}. Reading it with toString() gave an empty
            // string, so the check treated the very first "Loading" snapshot as "ready" and failed
            // as soon as the loader needed longer than one snapshot — which it did once the
            // envelope resolution went up.
            const QVariantMap deckState = deckById(snapshot, QStringLiteral("A"));
            // Unit variants ("Ready") serialise as a string, variants with data as a one-key map
            // ({"Loading":{"progress":0}}). Both shapes have to be understood, or "Ready" reads as
            // "not loaded yet" and the check waits until the watchdog gives up.
            const QVariant loadValue = deckState.value(QStringLiteral("load"));
            const QVariantMap loadState = loadValue.toMap();
            const QString state = loadValue.typeId() == QMetaType::QString
                ? loadValue.toString()
                : (loadState.isEmpty() ? QString() : loadState.firstKey());
            // Empty is not an answer either: the load was sent, and a deck that has not taken it yet
            // reports Empty for a snapshot or two. Waiting while it is Empty is the difference between
            // catching a slow loader and blaming the deck for a picture it never had a track for.
            if (state.isEmpty() || state == QStringLiteral("Loading")
                || state == QStringLiteral("Empty")) {
                return; // still working; the loader is off the audio path by design
            }
            if (state == QStringLiteral("Failed")) {
                failure = QStringLiteral("the deck failed to load: %1")
                              .arg(loadState.value(state).toMap()
                                       .value(QStringLiteral("message"))
                                       .toString());
                loop.quit();
                return;
            }
            const QVariantMap track = deckState.value(QStringLiteral("track")).toMap();
            std::fprintf(stdout, "dj-ui: deck A is ready: %s\n",
                         qPrintable(track.value(QStringLiteral("title")).toString()));
            // The engine publishes a reference to the envelope; the UI must be able to read it, or
            // the deck has no picture however good the snapshot is.
            const QVariantMap reference = deckState.value(QStringLiteral("waveform")).toMap();
            if (reference.isEmpty()) {
                std::fprintf(stderr, "dj-ui: deck state: %s\n",
                             QJsonDocument::fromVariant(deckState).toJson(QJsonDocument::Compact)
                                 .constData());
                failure = QStringLiteral("the deck published no waveform");
                loop.quit();
                return;
            }
            const QString envelope = reference.value(QStringLiteral("path")).toString();
            if (!waveform.load(envelope) || waveform.buckets() <= 0) {
                failure = waveform.error();
                loop.quit();
                return;
            }
            // Coverage is the property the renderer relies on: every frame of the track has a
            // column, so a marker or a playhead near the end cannot fall off the picture.
            const double covered = double(waveform.buckets()) * waveform.framesPerBucket();
            const double duration = track.value(QStringLiteral("duration_frames")).toDouble();
            if (duration > 0 && covered + waveform.framesPerBucket() < duration) {
                failure = QStringLiteral("the envelope covers %1 frames of %2")
                              .arg(covered)
                              .arg(duration);
                loop.quit();
                return;
            }
            // The coloured style needs the band levels; a reader that silently dropped them would
            // still draw, in one colour, and nobody would notice until they looked closely.
            double loudestBass = 0.0;
            double loudestMids = 0.0;
            double loudestTreble = 0.0;
            const QVariantList columnList = waveform.columns();
            for (const QVariant &entry : columnList) {
                const QVariantMap column = entry.toMap();
                loudestBass = qMax(loudestBass, column.value(QStringLiteral("low")).toDouble());
                loudestMids = qMax(loudestMids, column.value(QStringLiteral("mid")).toDouble());
                loudestTreble = qMax(loudestTreble, column.value(QStringLiteral("high")).toDouble());
            }
            if (loudestBass <= 0.0 && loudestMids <= 0.0 && loudestTreble <= 0.0) {
                failure = QStringLiteral("the envelope carries no band levels");
                loop.quit();
                return;
            }
            // The default fixture is a 440 Hz tone, which lives in the mid band: if the three bands
            // were really one number copied three times, this would not hold. Material whose bands
            // genuinely differ (drums) is checked by the spread instead, below.
            if (!expectSpread && (loudestMids <= loudestBass || loudestMids <= loudestTreble)) {
                failure = QStringLiteral("the mid band is not dominant for a 440 Hz tone: "
                                         "low %1 mid %2 high %3")
                              .arg(loudestBass)
                              .arg(loudestMids)
                              .arg(loudestTreble);
                loop.quit();
                return;
            }
            // The anti-moiré rule, checked as a property rather than by eye: for any screen width
            // the profile has exactly one entry per pixel and the pixel ranges tile the window, so
            // no bucket is drawn twice and none is skipped.
            for (const int probeWidth : {640, 901, 1280}) {
                const QVariantList profile = waveform.profile(probeWidth, 0.0, 1.0);
                if (profile.size() != probeWidth) {
                    failure = QStringLiteral("profile at %1 px has %2 entries")
                                  .arg(probeWidth)
                                  .arg(profile.size());
                    loop.quit();
                    return;
                }
                if (waveform.lastCoveredBuckets() != waveform.buckets()) {
                    failure = QStringLiteral("profile at %1 px covers %2 of %3 buckets")
                                  .arg(probeWidth)
                                  .arg(waveform.lastCoveredBuckets())
                                  .arg(waveform.buckets());
                    loop.quit();
                    return;
                }
            }
            std::fprintf(stdout, "dj-ui: profile tiles the track at 640, 901 and 1280 px\n");
            // The overview must be smooth. Without the horizontal blur the bars beat against the
            // envelope grid and the strip reads as a pixel moiré; the mean distance of a bar from
            // its neighbours is the number that catches it.
            {
                const QVariantList overview = waveform.profile(1200, 0.0, 1.0);
                double total = 0.0;
                int compared = 0;
                for (int index = 1; index + 1 < overview.size(); ++index) {
                    if (overview.at(index).toMap().value(QStringLiteral("outside")).toBool()) {
                        continue;
                    }
                    const double left = overview.at(index - 1).toMap()
                                            .value(QStringLiteral("amplitude")).toDouble();
                    const double right = overview.at(index + 1).toMap()
                                             .value(QStringLiteral("amplitude")).toDouble();
                    const double here = overview.at(index).toMap()
                                            .value(QStringLiteral("amplitude")).toDouble();
                    total += qAbs(here - 0.5 * (left + right)) * 80.0;
                    ++compared;
                }
                const double roughness = compared > 0 ? total / compared : 0.0;
                std::fprintf(stdout, "dj-ui: overview roughness %.2f px\n", roughness);

                // A zoomed view must keep its detail: if the smoothed outline were used there too,
                // the bars would be as flat as the overview and a beat would have no edge.
                const QVariantList zoomed = waveform.profile(1200, 0.3, 0.1);
                double zoomTotal = 0.0;
                int zoomCompared = 0;
                for (int index = 1; index + 1 < zoomed.size(); ++index) {
                    if (zoomed.at(index).toMap().value(QStringLiteral("outside")).toBool()) {
                        continue;
                    }
                    const double left = zoomed.at(index - 1).toMap()
                                            .value(QStringLiteral("amplitude")).toDouble();
                    const double right = zoomed.at(index + 1).toMap()
                                             .value(QStringLiteral("amplitude")).toDouble();
                    const double here = zoomed.at(index).toMap()
                                            .value(QStringLiteral("amplitude")).toDouble();
                    zoomTotal += qAbs(here - 0.5 * (left + right)) * 80.0;
                    ++zoomCompared;
                }
                const double zoomRoughness = zoomCompared > 0 ? zoomTotal / zoomCompared : 0.0;
                std::fprintf(stdout, "dj-ui: zoomed roughness %.2f px (overview %.2f)\n",
                             zoomRoughness, roughness);
                if (expectSpread && !(zoomRoughness > roughness)) {
                    failure = QStringLiteral("the zoom is as flat as the overview: %.2f against %.2f px")
                                  .arg(zoomRoughness, 0, 'f', 2)
                                  .arg(roughness, 0, 'f', 2);
                    loop.quit();
                    return;
                }
                if (expectSpread && roughness > 1.0) {
                    failure = QStringLiteral("the overview is jagged: %.2f px between neighbours")
                                  .arg(roughness);
                    loop.quit();
                    return;
                }
            }


            // The colours must not depend on where the window happens to sit. They did while the
            // smoothing was recomputed per paint: as the waveform scrolled, every frame re-smoothed
            // a different set of neighbours and the strip shimmered in time with the music.
            {
                const QVariantList left = waveform.profile(400, 0.0, 0.5);
                const QVariantList right = waveform.profile(400, 0.1, 0.5);
                // Both windows span 0.5 of the track over 400 bars, so one bar covers 0.00125 and
                // the 0.1 offset is exactly 80 bars. Bar i of the left window is bar i-80 of the
                // right one, and that is the whole point: the same moment must keep its colour.
                int compared = 0;
                double worstHue = 0.0;
                double worstHeight = 0.0;
                for (int index = 0; index < 320; ++index) {
                    const int leftIndex = 80 + index;
                    const int rightIndex = index;
                    if (leftIndex >= left.size() || rightIndex >= right.size()) {
                        break;
                    }
                    const QColor a(left.at(leftIndex).toMap().value(QStringLiteral("rgb")).toString());
                    const QColor b(right.at(rightIndex).toMap().value(QStringLiteral("rgb")).toString());
                    // Only bars that carry colour on both sides: on sparse material a bar can pick
                    // up a silent column in one window and not in the other, and comparing black
                    // with a hue says nothing about shimmering.
                    if (qMax(a.red(), qMax(a.green(), a.blue())) < 40
                        || qMax(b.red(), qMax(b.green(), b.blue())) < 40) {
                        continue;
                    }
                    double difference = qAbs(a.hsvHueF() - b.hsvHueF()) * 360.0;
                    difference = qMin(difference, 360.0 - difference);
                    worstHue = qMax(worstHue, difference);
                    // Height must not depend on the window either: a kernel that resizes with the
                    // view makes the whole strip pump as it scrolls.
                    const double heightA = left.at(leftIndex).toMap()
                                               .value(QStringLiteral("amplitude")).toDouble();
                    const double heightB = right.at(rightIndex).toMap()
                                                .value(QStringLiteral("amplitude")).toDouble();
                    worstHeight = qMax(worstHeight, qAbs(heightA - heightB));
                    ++compared;
                }
                if (compared < 20) {
                    std::fprintf(stdout,
                                 "dj-ui: window comparison skipped, only %d comparable bars\n",
                                 compared);
                } else if (worstHeight > 0.04) {
                    failure = QStringLiteral("height depends on the window: up to %1 of full scale")
                                  .arg(worstHeight, 0, 'f', 3);
                    loop.quit();
                    return;
                } else if (worstHue > 25.0) {
                    failure = QStringLiteral("colour depends on the window: hue moves up to %1 "
                                             "degrees")
                                  .arg(worstHue, 0, 'f', 1);
                    loop.quit();
                    return;
                } else {
                    std::fprintf(stdout,
                                 "dj-ui: the picture does not depend on the window (max %.1f degrees, "
                                 "%.3f of height, over %d bars)\n",
                                 worstHue, worstHeight, compared);
                }
            }


            // Which band wins each column of the whole track. Printed, not asserted, because a
            // real track has its own balance; but if one band takes everything on music too, the
            // colouring is broken and this line says so before anyone looks at the screen.
            {
                const QVariantList whole = waveform.columns();
                int counts[3] = {0, 0, 0};
                for (const QVariant &entry : whole) {
                    const QVariantMap pixel = entry.toMap();
                    const double weights[3] = {pixel.value(QStringLiteral("low")).toDouble(),
                                               pixel.value(QStringLiteral("mid")).toDouble(),
                                               pixel.value(QStringLiteral("high")).toDouble()};
                    int best = 0;
                    for (int band = 1; band < 3; ++band) {
                        if (weights[band] > weights[best]) {
                            best = band;
                        }
                    }
                    ++counts[best];
                }
                const double shown = whole.isEmpty() ? 1.0 : double(whole.size());
                std::fprintf(stdout,
                             "dj-ui: colour spread over the track: bass %.0f%% mid %.0f%% high %.0f%%\n",
                             100.0 * counts[0] / shown, 100.0 * counts[1] / shown,
                             100.0 * counts[2] / shown);
                // A single tone legitimately has one band everywhere, so this is only a defect
                // when the material has balance to show — which is what --waveform-expect-spread
                // declares. The shares are printed either way.
                if (expectSpread) {
                    // Drum-like material must not paint one colour: at least two bands have to own
                    // a tenth of the columns each. Two rather than three, because a kick-and-hat
                    // loop legitimately has almost no mid content, and demanding it would be a
                    // threshold chosen to fit the fixture instead of the requirement.
                    int covered = 0;
                    int largest = 0;
                    for (int band = 0; band < 3; ++band) {
                        if (double(counts[band]) / shown >= 0.05) {
                            ++covered;
                        }
                        largest = qMax(largest, counts[band]);
                    }
                    // Not monochrome: at least two bands have to be visible somewhere, and no single
                    // band may own almost everything. The share itself is the track's business — a
                    // kick-and-hat loop is mostly kick — but one colour everywhere is a defect.
                    if (covered < 2 || double(largest) / shown > 0.95) {
                        failure = QStringLiteral("the picture is one colour: %1 band(s) above 5%%, "
                                                 "largest %2%")
                                      .arg(covered)
                                      .arg(100.0 * largest / shown, 0, 'f', 1);
                        loop.quit();
                        return;
                    }
                    std::fprintf(stdout, "dj-ui: %d bands are visible, largest takes %.0f%%\n",
                                 covered, 100.0 * largest / shown);
                }
            }
            std::fprintf(stdout,
                         "dj-ui: waveform loaded: %d columns of %d frames at %d Hz, covering %.0f "
                         "of %.0f frames; bands low %.3f mid %.3f high %.3f\n",
                         waveform.buckets(), waveform.framesPerBucket(), waveform.sampleRate(),
                         covered, duration, loudestBass, loudestMids, loudestTreble);
            if (!loadSecondPath.isEmpty()) {
                stage = SendingLoadSecond;
                commandId = bridge.load(QStringLiteral("B"), loadSecondPath);
                std::fprintf(stdout, "dj-ui: sent Load(B) as command %d\n", commandId);
                sawAck = false;
                ackApplied = false;
                ackRejected = false;
                return;
            }
            stage = SendingPlay;
            commandId = bridge.play(QStringLiteral("A"), true);
            std::fprintf(stdout, "dj-ui: sent Play(A) as command %d\n", commandId);
            sawAck = false;
            ackApplied = false;
            ackRejected = false;
            return;
        }
        case SendingLoadSecond: {
            // Two decks, two envelopes. A shared reader made deck B paint deck A's track; this is
            // the check that would have caught it.
            const QVariantMap deckB = deckById(snapshot, QStringLiteral("B"));
            if (deckB.value(QStringLiteral("load")).toString() != QStringLiteral("Ready")) {
                return;
            }
            const QVariantMap reference = deckB.value(QStringLiteral("waveform")).toMap();
            if (reference.isEmpty()) {
                return;
            }
            Waveform second;
            if (!second.load(reference.value(QStringLiteral("path")).toString())) {
                failure = second.error();
                loop.quit();
                return;
            }
            const int columnsA = waveform.buckets();
            const int columnsB = second.buckets();
            if (columnsA <= 0 || columnsB <= 0 || columnsA == columnsB) {
                failure = QStringLiteral("envelopes are not independent: A=%1 B=%2")
                              .arg(columnsA)
                              .arg(columnsB);
                loop.quit();
                return;
            }
            std::fprintf(stdout,
                         "dj-ui: two envelopes, kept apart: A %d columns, B %d columns\n",
                         columnsA, columnsB);
            stage = SendingPlay;
            commandId = bridge.play(QStringLiteral("A"), true);
            std::fprintf(stdout, "dj-ui: sent Play(A) as command %d\n", commandId);
            sawAck = false;
            ackApplied = false;
            ackRejected = false;
            return;
        }
        case SendingPlay:
            if (!deckIsPlaying(snapshot, QStringLiteral("A"))) {
                return;
            }
            sawPlaying = true;
            stage = SendingSetCue;
            commandId = bridge.cue(QStringLiteral("A"), QStringLiteral("Set"), 0);
            std::fprintf(stdout, "dj-ui: sent Cue(Set 0) as command %d\n", commandId);
            sawAck = false;
            ackApplied = false;
            ackRejected = false;
            return;
        case SendingSetCue:
            if (!deckHasHotcue(snapshot, QStringLiteral("A"), 0)) {
                return;
            }
            sawCue = true;
            stage = SendingJumpCue;
            commandId = bridge.cue(QStringLiteral("A"), QStringLiteral("Jump"), 0);
            std::fprintf(stdout, "dj-ui: sent Cue(Jump 0) as command %d\n", commandId);
            sawAck = false;
            ackApplied = false;
            ackRejected = false;
            return;
        case SendingJumpCue:
            stage = Done;
            loop.quit();
            return;
        case Done:
            return;
        }
    });

    QObject::connect(&bridge, &BridgeClient::failed, &loop, [&](const QString &message) {
        std::fprintf(stderr, "dj-ui: %s\n", qPrintable(message));
        loop.quit();
    });

    bridge.connectToEngine(socketPath);
    loop.exec();

    if (!failure.isEmpty()) {
        std::fprintf(stderr, "dj-ui: self-check failed: %s\n", qPrintable(failure));
        return 1;
    }
    if (!bridge.isConnected()) {
        std::fprintf(stderr, "dj-ui: self-check failed: no engine at %s\n", qPrintable(socketPath));
        return 1;
    }
    if (!sawAck) {
        std::fprintf(stderr, "dj-ui: self-check failed: the engine never acknowledged\n");
        return 1;
    }
    if (!ackApplied) {
        std::fprintf(stderr, "dj-ui: self-check failed: the command was not applied\n");
        return 1;
    }
    if (!sawPlaying) {
        std::fprintf(stderr, "dj-ui: self-check failed: no snapshot reported deck A playing\n");
        return 1;
    }
    if (!sawCue) {
        std::fprintf(stderr, "dj-ui: self-check failed: the cue never appeared in a snapshot\n");
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: self-check passed: Play and Cue acknowledged and visible in the snapshot "
                 "(sample %llu, rev %u) after %lld ms\n",
                 static_cast<unsigned long long>(appliedAt),
                 bridge.snapshot().value(QStringLiteral("revision")).toUInt(),
                 static_cast<long long>(elapsed.elapsed()));
    return 0;
}

} // namespace

int main(int argc, char *argv[])
{
    qInstallMessageHandler(collectMessages);

    QGuiApplication app(argc, argv);

    // Controls follow the application palette, and the palette is the measured Traktor scheme:
    // neutral dark greys with one blue accent. Without this the shell's own background is dark
    // while every control stays light, which is exactly wrong for a booth.
    // The Controls style reads the palette; Fusion is the one that honours it on every platform.
    qputenv("QT_QUICK_CONTROLS_STYLE", "Fusion");
    QPalette palette;
    palette.setColor(QPalette::Window, QColor(QStringLiteral("#181818")));
    palette.setColor(QPalette::WindowText, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::Base, QColor(QStringLiteral("#101010")));
    palette.setColor(QPalette::AlternateBase, QColor(QStringLiteral("#202020")));
    palette.setColor(QPalette::Text, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::Button, QColor(QStringLiteral("#282828")));
    palette.setColor(QPalette::ButtonText, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::BrightText, QColor(QStringLiteral("#f0f3f6")));
    palette.setColor(QPalette::Highlight, QColor(QStringLiteral("#5090f0")));
    palette.setColor(QPalette::HighlightedText, QColor(QStringLiteral("#0b0b0b")));
    palette.setColor(QPalette::ToolTipBase, QColor(QStringLiteral("#202020")));
    palette.setColor(QPalette::ToolTipText, QColor(QStringLiteral("#c8c8c8")));
    palette.setColor(QPalette::PlaceholderText, QColor(QStringLiteral("#8a8a8a")));
    palette.setColor(QPalette::Disabled, QPalette::Text, QColor(QStringLiteral("#6a6a6a")));
    palette.setColor(QPalette::Disabled, QPalette::ButtonText, QColor(QStringLiteral("#6a6a6a")));
    QGuiApplication::setPalette(palette);

    QGuiApplication::setPalette(palette);

    QCoreApplication::setApplicationName(QStringLiteral("dj-platform"));
    QCoreApplication::setApplicationVersion(QStringLiteral("0.1.0"));
    QCoreApplication::setOrganizationName(QStringLiteral("DJ Platform"));

    QCommandLineParser parser;
    parser.setApplicationDescription(
        QStringLiteral("DJ Platform UI shell: renders the engine snapshot, forwards commands."));
    parser.addHelpOption();
    parser.addVersionOption();
    const QCommandLineOption smokeOption(
        QStringLiteral("smoke"),
        QStringLiteral("Load the QML scene, report Qt warnings, then exit without a window."));
    const QCommandLineOption connectOption(
        QStringLiteral("connect"),
        QStringLiteral("Engine socket to connect to (default: %1).").arg(defaultSocketPath()),
        QStringLiteral("path"));
    const QCommandLineOption stripCheckOption(
        QStringLiteral("strip-check"),
        QStringLiteral("With --connect and --load: load the track into deck A and require the deck's "
                       "strip to receive the envelope the engine published."));
    const QCommandLineOption exitAfterOption(
        QStringLiteral("exit-after"),
        QStringLiteral("Close the window after this many milliseconds: the shutdown path, exercised "
                       "instead of described."),
        QStringLiteral("ms"));
    const QCommandLineOption gridReconnectOption(
        QStringLiteral("grid-reconnect-check"),
        QStringLiteral("With --connect only: the deck is already loaded, as it is for a client that "
                       "reconnects to a running engine, and every grid button must still work."));
    const QCommandLineOption metronomeOption(
        QStringLiteral("metronome-check"),
        QStringLiteral("With --connect and --load: require the deck's metronome button to reach the "
                       "engine, the state to come back, and the click to be played on the beats."));
    const QCommandLineOption tapOption(
        QStringLiteral("tap-check"),
        QStringLiteral("With --connect and --load: require the tapped grid to be put on the taps — the "
                       "tempo from their slope and a beat on the line through them."));
    const QCommandLineOption gridLinesOption(
        QStringLiteral("grid-lines-check"),
        QStringLiteral("With --connect and --load: require the strip to draw the beat lines of the grid, "
                       "with a bar line every fourth beat, and the spacing to follow the tempo."));
    const QCommandLineOption gridCheckOption(
        QStringLiteral("grid-check"),
        QStringLiteral("With --connect and --load: move the grid, halve it, double it and type a tempo, "
                       "requiring the deck and the file to follow every edit."));
    const QCommandLineOption loopBeatsOption(
        QStringLiteral("loop-beats-check"),
        QStringLiteral("Check that a loop of four beats is four beats of the grid, and that the arrows "
                       "move it by a beat without shortening it (DJP-DECK-005)."));
    const QCommandLineOption syncCheckOption(
        QStringLiteral("sync-check"),
        QStringLiteral("Load two decks, make one the master through its button, put the other on its "
                       "beat, and require the state and the phase meter to agree with the engine."));
    const QCommandLineOption transportExtrasOption(
        QStringLiteral("transport-extras-check"),
        QStringLiteral("With --connect and --load: press a jump and require the play head on a beat of "
                       "the grid, hold and release the nudge, and switch slip on and off "
                       "(DJP-DECK-006/007)."));
    const QCommandLineOption gridHereOption(
        QStringLiteral("grid-here-check"),
        QStringLiteral("With --connect and --load: seek off the grid, press the button between the arrows, "
                       "and require a beat of the grid to land on the play head — and to survive a reload."));
    const QCommandLineOption tempoRecheckOption(
        QStringLiteral("tempo-recheck"),
        QStringLiteral("With --connect and --load: require the deck's re-measure button to ask before "
                       "it runs, then confirm it and require the new grid on disk."));
    const QCommandLineOption cueWriteOption(
        QStringLiteral("cue-write-check"),
        QStringLiteral("With --connect and --load: seek, store hotcue 2 and set a loop, then require the "
                       "snapshot to show them. The pair of checks proves a reload brings them back."));
    const QCommandLineOption cueReadOption(
        QStringLiteral("cue-read-check"),
        QStringLiteral("With --connect and --load: expect hotcue 2 at 48000 and a loop 96000..144000 "
                       "from the file alone, sending no cue commands."));
    const QCommandLineOption masterCheckOption(
        QStringLiteral("master-check"),
        QStringLiteral("With --connect: the master knob must change the published gain, the master "
                       "meter must see the signal, and the knob must sit above a shorter meter."));
    const QCommandLineOption curveCheckOption(
        QStringLiteral("curve-check"),
        QStringLiteral("With --connect: send each crossfader law and require the snapshot to report "
                       "it back."));
    const QCommandLineOption windowCheckOption(
        QStringLiteral("window-check"),
        QStringLiteral("With --session-file: restore the remembered window shape, toggle it with F11 "
                       "and require the file to follow."));
    const QCommandLineOption gainCurveOption(
        QStringLiteral("gain-curve"),
        QStringLiteral("Law of the level knobs: linear (straight through) or cubic (soft around unity, "
                       "the default)."),
        QStringLiteral("kind"));
    const QCommandLineOption gainCurveMiddleOption(
        QStringLiteral("gain-curve-middle"),
        QStringLiteral("How much of the straight rate a level knob keeps at unity, 0.05 to 1 "
                       "(1 is straight through; the default is 0.1)."),
        QStringLiteral("share"));
    const QCommandLineOption faderCurveOption(
        QStringLiteral("fader-curve"),
        QStringLiteral("Law of the channel faders: linear, cubic (soft around the top) or logarithmic "
                       "(B-taper, the default: a quarter of the throw is about ten decibels)."),
        QStringLiteral("kind"));
    const QCommandLineOption faderCurveMiddleOption(
        QStringLiteral("fader-curve-middle"),
        QStringLiteral("The same setting for the channel faders, which are ridden rather than set, "
                       "0.05 to 1 (1 is straight through; the default is 0.5)."),
        QStringLiteral("share"));
    const QCommandLineOption gainCurveCheckOption(
        QStringLiteral("gain-curve-check"),
        QStringLiteral("Measure the level knobs' law at unity and at the end of their travel and "
                       "require the configured curve to be in force."));
    const QCommandLineOption tempoCheckOption(
        QStringLiteral("tempo-check"),
        QStringLiteral("With --connect and --load: require the engine to measure a tempo and the "
                       "deck to show it."));
    const QCommandLineOption eqCheckOption(
        QStringLiteral("eq-check"),
        QStringLiteral("With --connect: send three band gains to one deck and require the snapshot "
                       "to apply them and the strip's knobs to show those numbers."));
    const QCommandLineOption loopOption(
        QStringLiteral("loop-check"),
        QStringLiteral("With --connect and --load: press Loop twice and require the engine to hold a "
                       "loop between the two playhead positions, then require shift on the second "
                       "press to cancel the pair."));
    const QCommandLineOption scenarioOption(
        QStringLiteral("scenario-check"),
        QStringLiteral("With --connect and --load: drive the loops and cue points through the widgets "
                       "and require every user story to hold. Prints one line per case."));
    const QCommandLineOption padsOption(
        QStringLiteral("pads-check"),
        QStringLiteral("With --connect and --load: click an empty pad to set it, name and colour it, "
                       "jump to it by clicking again and clear it with shift, requiring the engine to "
                       "report each step."));
    const QCommandLineOption transportOption(
        QStringLiteral("transport-check"),
        QStringLiteral("With --connect and --load: play the deck, stop it and require the playhead to "
                       "return to the cue point, scrub the overview with the hand, and require the "
                       "clock to show the time left rather than the length."));
    const QCommandLineOption pitchOption(
        QStringLiteral("pitch-check"),
        QStringLiteral("With --connect: drag a deck's pitch fader and require the engine to play at "
                       "the rate the hand asked for, then double click it back to the file's own "
                       "speed."));
    const QCommandLineOption neutralOption(
        QStringLiteral("neutral-check"),
        QStringLiteral("With --connect: seat every mouse-driven control away from its neutral "
                       "position, double click it, and require the engine to report the neutral "
                       "value."));
    const QCommandLineOption wheelCheckOption(
        QStringLiteral("wheel-check"),
        QStringLiteral("With --connect: scroll over a channel knob and require the deck's gain to "
                       "change in the engine."));
    const QCommandLineOption spaceCheckOption(
        QStringLiteral("space-check"),
        QStringLiteral("With --connect and --load: prove Space no longer touches playback and folds "
                       "the browser instead."));
    const QCommandLineOption refusalCheckOption(
        QStringLiteral("refusal-check"),
        QStringLiteral("With --connect and --load: play deck A, ask the shell to load into it, and "
                       "require the deck's buttons to signal the refusal."));
    const QCommandLineOption busyCheckOption(
        QStringLiteral("load-busy-check"),
        QStringLiteral("With --connect and --load: prove a playing deck refuses a load (Busy) and "
                       "accepts it once stopped."));
    const QCommandLineOption gainCheckOption(
        QStringLiteral("gain-check"),
        QStringLiteral("With --connect: send a channel gain and require the snapshot to carry it "
                       "for that deck and no other."));
    const QCommandLineOption selfCheckOption(
        QStringLiteral("self-check"),
        QStringLiteral("Connect, press Play, require the ack and the snapshot, then exit."));
    const QCommandLineOption dialogCheckOption(
        QStringLiteral("dialog-check"),
        QStringLiteral("Check the in-app file browser without a display: the QML module loads and "
                       "lists a directory."));
    const QCommandLineOption outsideColorOption(
        QStringLiteral("waveform-outside-color"),
        QStringLiteral("Colour of the regions before the start and after the end of a track "
                       "(default: #101010, the window background)."),
        QStringLiteral("colour"));
    const QCommandLineOption styleOption(
        QStringLiteral("waveform-style"),
        QStringLiteral("coloured (default: bass warm, treble cold) or plain."),
        QStringLiteral("style"));
    const QCommandLineOption barWidthOption(
        QStringLiteral("waveform-bar-width"),
        QStringLiteral("Width of one waveform bar in pixels (default 1: one bar per pixel)."),
        QStringLiteral("px"));
    const QCommandLineOption barGapOption(
        QStringLiteral("waveform-bar-gap"),
        QStringLiteral("Space between bars in pixels (default 0: a continuous waveform)."),
        QStringLiteral("px"));
    const QCommandLineOption bandGainOption(
        QStringLiteral("waveform-band-gain"),
        QStringLiteral("Trim the three bands before colouring, comma separated "
                       "(default 1,1,1). Raise the low gain to see more bass colour."),
        QStringLiteral("low,mid,high"));
    const QCommandLineOption layoutOption(
        QStringLiteral("layout"),
        QStringLiteral("Arrange the shell like a preset: traktor (default, measured from the "
                       "reference) or two-deck."),
        QStringLiteral("preset"));
    const QCommandLineOption layoutFileOption(
        QStringLiteral("layout-file"),
        QStringLiteral("Where the layout is remembered (default $XDG_CONFIG_HOME/hauler-dj/"
                       "layout.json). A preset on the command line overrides the file."),
        QStringLiteral("path"));
    const QCommandLineOption sessionFileOption(
        QStringLiteral("session-file"),
        QStringLiteral("Where the shell remembers the last folder it loaded from (default "
                       "$XDG_CONFIG_HOME/hauler-dj/session.json)."),
        QStringLiteral("path"));
    const QCommandLineOption channelOrderOption(
        QStringLiteral("channel-order"),
        QStringLiteral("Order of the mixer strips, left to right (default CABD). A permutation of "
                       "A, B, C and D."),
        QStringLiteral("ABCD"));
    const QCommandLineOption layoutCheckOption(
        QStringLiteral("layout-check"),
        QStringLiteral("Load the shell, print each region's share of the window and fail when it "
                       "differs from the layout by more than two points."));
    const QCommandLineOption contrastOption(
        QStringLiteral("waveform-contrast"),
        QStringLiteral("How much the strip separates loud from quiet: the height is raised to this "
                       "power (default 1.4; 0.5..3). 1.0 draws the amplitudes as they are."),
        QStringLiteral("power"));
    const QCommandLineOption zoomOption(
        QStringLiteral("waveform-zoom"),
        QStringLiteral("Zoom the strip starts at, as a factor (default 10; 1..100). A double click "
                       "returns to it."),
        QStringLiteral("factor"));
    const QCommandLineOption frameRateOption(
        QStringLiteral("fps"),
        QStringLiteral("Frames drawn per second (default 20, from display.frame_hz in the settings; "
                       "5..240). The strip is projected between engine snapshots, so a higher rate "
                       "smooths a fast scroll without asking the engine for more state."),
        QStringLiteral("hz"));
    const QCommandLineOption benchOption(
        QStringLiteral("waveform-bench"),
        QStringLiteral("Redraw the strip N times and report the cost of a frame, in milliseconds."),
        QStringLiteral("frames"));
    const QCommandLineOption themeCheckOption(
        QStringLiteral("ui-theme-check"),
        QStringLiteral("Grab the shell and fail unless it is dark: a light panel means the palette "
                       "was lost."));
    const QCommandLineOption browserStartOption(
        QStringLiteral("browser-start-check"),
        QStringLiteral("Prove the browser opens the folder remembered from the last load, and that a "
                       "new load moves it."));
    const QCommandLineOption browserCheckOption(
        QStringLiteral("browser-check"),
        QStringLiteral("Load the shell, pick a track in the browser and act on it, the way a click "
                       "does, then print what happened."));
    const QCommandLineOption browserRestoreOption(
        QStringLiteral("browser-restore-check"),
        QStringLiteral("Start the shell on a layout file written by an earlier run and require the "
                       "browser's rail, the browser's text and the deck's zoom to be what that file "
                       "says."));
    const QCommandLineOption screenshotOption(
        QStringLiteral("ui-screenshot"),
        QStringLiteral("Load the shell, wait, grab it into OUT.png and exit. Verifies the theme "
                       "without a display."),
        QStringLiteral("OUT.png"));
    const QCommandLineOption spanOption(
        QStringLiteral("waveform-span"),
        QStringLiteral("Window to render for --waveform-png: START,SPAN as fractions of the track "
                       "(default 0,1). Use a small span to check a zoomed view."),
        QStringLiteral("START,SPAN"));
    const QCommandLineOption waveformPngOption(
        QStringLiteral("waveform-png"),
        QStringLiteral("Render a waveform envelope to an image and exit: PATH OUT.png"),
        QStringLiteral("PATH,OUT"));
    const QCommandLineOption expectSpreadOption(
        QStringLiteral("waveform-expect-spread"),
        QStringLiteral("Fail unless every band claims at least 10% of the loaded track's columns."));
    const QCommandLineOption bandColorOption(
        QStringLiteral("waveform-band-colors"),
        QStringLiteral("Three colours for bass, mids and treble, comma separated "
                       "(default #ff2f1f,#35d13a,#2f6bff)."),
        QStringLiteral("low,mid,high"));
    const QCommandLineOption loadSecondOption(
        QStringLiteral("load-b"),
        QStringLiteral("With --self-check: load this file into deck B as well."),
        QStringLiteral("path"));
    const QCommandLineOption loadOption(
        QStringLiteral("load"),
        QStringLiteral("With --self-check: load this file into deck A first."),
        QStringLiteral("path"));
    parser.addOption(smokeOption);
    parser.addOption(connectOption);
    parser.addOption(selfCheckOption);
    parser.addOption(gainCheckOption);
    parser.addOption(gainCurveOption);
    parser.addOption(gainCurveMiddleOption);
    parser.addOption(faderCurveOption);
    parser.addOption(faderCurveMiddleOption);
    parser.addOption(gainCurveCheckOption);
    parser.addOption(eqCheckOption);
    parser.addOption(tempoCheckOption);
    parser.addOption(neutralOption);
    parser.addOption(pitchOption);
    parser.addOption(transportOption);
    parser.addOption(scenarioOption);
    parser.addOption(padsOption);
    parser.addOption(loopOption);
    parser.addOption(busyCheckOption);
    parser.addOption(refusalCheckOption);
    parser.addOption(spaceCheckOption);
    parser.addOption(wheelCheckOption);
    parser.addOption(windowCheckOption);
    parser.addOption(curveCheckOption);
    parser.addOption(masterCheckOption);
    parser.addOption(gridReconnectOption);
    parser.addOption(metronomeOption);
    parser.addOption(tapOption);
    parser.addOption(gridLinesOption);
    parser.addOption(gridCheckOption);
    parser.addOption(loopBeatsOption);
    parser.addOption(syncCheckOption);
    parser.addOption(transportExtrasOption);
    parser.addOption(gridHereOption);
    parser.addOption(tempoRecheckOption);
    parser.addOption(cueWriteOption);
    parser.addOption(cueReadOption);
    parser.addOption(exitAfterOption);
    parser.addOption(stripCheckOption);
    parser.addOption(loadOption);
    parser.addOption(loadSecondOption);
    parser.addOption(outsideColorOption);
    parser.addOption(styleOption);
    parser.addOption(bandColorOption);
    parser.addOption(expectSpreadOption);
    parser.addOption(waveformPngOption);
    parser.addOption(spanOption);
    parser.addOption(screenshotOption);
    parser.addOption(browserCheckOption);
    parser.addOption(browserRestoreOption);
    parser.addOption(browserStartOption);
    parser.addOption(themeCheckOption);
    parser.addOption(benchOption);
    parser.addOption(frameRateOption);
    parser.addOption(zoomOption);
    parser.addOption(contrastOption);
    parser.addOption(layoutOption);
    parser.addOption(layoutFileOption);
    parser.addOption(layoutCheckOption);
    parser.addOption(channelOrderOption);
    parser.addOption(sessionFileOption);
    parser.addOption(bandGainOption);
    parser.addOption(barWidthOption);
    parser.addOption(barGapOption);
    parser.addOption(dialogCheckOption);
    parser.process(app);

    auto bridge = std::make_unique<BridgeClient>();
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "Bridge", bridge.get());
    FileBrowser browser;
    SessionState session;
    if (parser.isSet(sessionFileOption)) {
        session.setFile(parser.value(sessionFileOption));
    }
    session.load();
    // The browser opens where the last track came from; an empty state falls back to the music
    // folder, which is what FileBrowser does when given no path.
    browser.open(session.folder());
    // Every load, from the browser or from a deck's dialog, moves that folder.
    bridge->setLoadFolderHandler([&session, &browser](const QString &folder) {
        if (session.rememberFolder(folder)) {
            session.save();
            browser.open(folder);
        }
    });
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "Browser", &browser);
    // A type, not a singleton: two decks hold two envelopes, and a singleton made the second deck
    // paint the first deck's waveform.
    qmlRegisterType<Waveform>("DjPlatform.Ui", 1, 0, "WaveformEnvelope");
    qmlRegisterType<WaveformItem>("DjPlatform.Ui", 1, 0, "WaveformStrip");
    Waveform waveform; // used by the headless self-check
    UiSettings settings;
    LayoutSettings layout;
    if (parser.isSet(outsideColorOption)
        && !settings.setWaveformOutsideColor(parser.value(outsideColorOption))) {
        std::fprintf(stderr, "dj-ui: %s is not a colour\n",
                     qPrintable(parser.value(outsideColorOption)));
        return 2;
    }
    if (parser.isSet(styleOption) && !settings.setWaveformStyle(parser.value(styleOption))) {
        std::fprintf(stderr, "dj-ui: %s is not a waveform style (coloured or plain)\n",
                     qPrintable(parser.value(styleOption)));
        return 2;
    }
    if (parser.isSet(bandColorOption)) {
        const QStringList parts = parser.value(bandColorOption).split(',');
        const QStringList bands = {QStringLiteral("low"), QStringLiteral("mid"), QStringLiteral("high")};
        if (parts.size() != 3) {
            std::fprintf(stderr, "dj-ui: --waveform-band-colors needs three colours\n");
            return 2;
        }
        for (int index = 0; index < 3; ++index) {
            if (!settings.setWaveformBandColor(bands.at(index), parts.at(index).trimmed())) {
                std::fprintf(stderr, "dj-ui: %s is not a colour for band %s\n",
                             qPrintable(parts.at(index)), qPrintable(bands.at(index)));
                return 2;
            }
        }
    }
    if (parser.isSet(barWidthOption)) {
        bool ok = false;
        const int width = parser.value(barWidthOption).toInt(&ok);
        if (!ok || !settings.setWaveformBarWidth(width)) {
            std::fprintf(stderr, "dj-ui: --waveform-bar-width needs 1..32\n");
            return 2;
        }
    }
    if (parser.isSet(barGapOption)) {
        bool ok = false;
        const int gap = parser.value(barGapOption).toInt(&ok);
        if (!ok || !settings.setWaveformBarGap(gap)) {
            std::fprintf(stderr, "dj-ui: --waveform-bar-gap needs 0..32\n");
            return 2;
        }
    }
    if (parser.isSet(contrastOption)) {
        bool ok = false;
        const double power = parser.value(contrastOption).toDouble(&ok);
        if (!ok || power < 0.5 || power > 3.0) {
            std::fprintf(stderr, "dj-ui: --waveform-contrast needs 0.5..3\n");
            return 2;
        }
        Waveform::setDefaultContrast(power);
    }
    if (parser.isSet(zoomOption)) {
        bool ok = false;
        const double factor = parser.value(zoomOption).toDouble(&ok);
        if (!ok || !settings.setWaveformZoom(factor)) {
            std::fprintf(stderr, "dj-ui: --waveform-zoom needs 1..400\n");
            return 2;
        }
    }
    if (parser.isSet(frameRateOption)) {
        bool ok = false;
        const int hz = parser.value(frameRateOption).toInt(&ok);
        if (!ok || !settings.setFrameHz(hz)) {
            std::fprintf(stderr, "dj-ui: --fps needs 5..240\n");
            return 2;
        }
    }
    double bandGain[3] = {1.0, 1.0, 1.0};
    if (parser.isSet(bandGainOption)) {
        const QStringList parts = parser.value(bandGainOption).split(',');
        bool ok = parts.size() == 3;
        double gains[3] = {1.0, 1.0, 1.0};
        for (int index = 0; ok && index < 3; ++index) {
            gains[index] = parts.at(index).trimmed().toDouble(&ok);
            ok = ok && gains[index] > 0.0;
        }
        if (!ok) {
            std::fprintf(stderr, "dj-ui: --waveform-band-gain needs three positive numbers\n");
            return 2;
        }
        bandGain[0] = gains[0];
        bandGain[1] = gains[1];
        bandGain[2] = gains[2];
        waveform.setBandGain(bandGain[0], bandGain[1], bandGain[2]);
    }
    waveform.setPalette(QColor(settings.waveformLowColor()), QColor(settings.waveformMidColor()),
                        QColor(settings.waveformHighColor()));
    if (parser.isSet(layoutFileOption)) {
        layout.setFile(parser.value(layoutFileOption));
    }
    // The file is the user's last arrangement; a preset named on the command line wins over it, so a
    // single run can be compared against the measured default without editing anything.
    layout.load();
    if (parser.isSet(layoutOption) && !layout.applyPreset(parser.value(layoutOption))) {
        std::fprintf(stderr, "dj-ui: --layout expects traktor or two-deck\n");
        return 2;
    }
    if (parser.isSet(channelOrderOption)
        && !layout.setChannelOrder(parser.value(channelOrderOption))) {
        std::fprintf(stderr, "dj-ui: --channel-order needs each of A, B, C and D once\n");
        return 2;
    }

    if (parser.isSet(gainCurveOption)
        && !settings.setGainCurve(parser.value(gainCurveOption).trimmed())) {
        std::fprintf(stderr, "dj-ui: %s is not a gain curve (linear or cubic)\n",
                     qPrintable(parser.value(gainCurveOption)));
        return 2;
    }
    if (parser.isSet(gainCurveMiddleOption)) {
        bool ok = false;
        const double middle = parser.value(gainCurveMiddleOption).toDouble(&ok);
        if (!ok || !settings.setGainCurveMiddle(middle)) {
            std::fprintf(stderr, "dj-ui: --gain-curve-middle wants a number from 0.05 to 1\n");
            return 2;
        }
    }
    if (parser.isSet(faderCurveOption)
        && !settings.setFaderCurve(parser.value(faderCurveOption).trimmed())) {
        std::fprintf(stderr, "dj-ui: %s is not a fader curve (linear, cubic or logarithmic)\n",
                     qPrintable(parser.value(faderCurveOption)));
        return 2;
    }
    if (parser.isSet(faderCurveMiddleOption)) {
        bool ok = false;
        const double middle = parser.value(faderCurveMiddleOption).toDouble(&ok);
        if (!ok || !settings.setFaderCurveMiddle(middle)) {
            std::fprintf(stderr, "dj-ui: --fader-curve-middle wants a number from 0.05 to 1\n");
            return 2;
        }
    }
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "UiSettings", &settings);
    // Not "Layout": that name belongs to QtQuick.Layouts' attached property.
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "LayoutState", &layout);

    if (parser.isSet(waveformPngOption)) {
        // The envelope drawn exactly as the deck draws it, but into a file: no display, no engine,
        // and something that can be measured afterwards instead of described.
        const QStringList parts = parser.value(waveformPngOption).split(',');
        if (parts.size() != 2) {
            std::fprintf(stderr, "dj-ui: --waveform-png needs PATH and OUT.png\n");
            return 2;
        }
        Waveform envelope;
        envelope.setPalette(QColor(settings.waveformLowColor()),
                            QColor(settings.waveformMidColor()),
                            QColor(settings.waveformHighColor()));
        envelope.setBandGain(bandGain[0], bandGain[1], bandGain[2]);
        if (!envelope.load(parts.at(0))) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(envelope.error()));
            return 1;
        }
        const int width = 1200;
        const int height = 160;
        const int barWidth = settings.waveformBarWidth();
        const int barGap = settings.waveformBarGap();
        const int barCount = qMax(1, width / (barWidth + barGap));
        double windowStart = 0.0;
        double windowSpan = 1.0;
        if (parser.isSet(spanOption)) {
            const QStringList span = parser.value(spanOption).split(',');
            bool ok = span.size() == 2;
            if (ok) {
                windowStart = span.at(0).toDouble(&ok);
                windowSpan = span.at(1).toDouble(&ok);
            }
            if (!ok || windowSpan <= 0.0) {
                std::fprintf(stderr, "dj-ui: --waveform-span needs START,SPAN within 0..1\n");
                return 2;
            }
        }
        const QVariantList pixels = envelope.profile(barCount, windowStart, windowSpan);
        QImage image(width, height, QImage::Format_RGB32);
        image.fill(QColor(settings.waveformStyle() == QStringLiteral("coloured")
                              ? QStringLiteral("#141414")
                              : QStringLiteral("#101010")));
        QPainter painter(&image);
        const double middle = height / 2.0;
        int painted = 0;
        for (int index = 0; index < pixels.size(); ++index) {
            const QVariantMap pixel = pixels.at(index).toMap();
            if (pixel.value(QStringLiteral("outside")).toBool()) {
                continue;
            }
            const double half = qMax(0.5, pixel.value(QStringLiteral("amplitude")).toDouble() * middle);
            // Full brightness at the centre axis, about half at the tips — the glow the deck draws.
            const QColor colour(pixel.value(QStringLiteral("rgb")).toString());
            static const double glow[3] = {0.42, 1.0, 0.42};
            const double x = index * (barWidth + barGap);
            for (int slice = 0; slice < 3; ++slice) {
                const double from = middle - half + (2 * half) * slice / 3.0;
                const double to = middle - half + (2 * half) * (slice + 1) / 3.0;
                QColor shaded = colour;
                shaded.setRedF(qBound(0.0, colour.redF() * glow[slice], 1.0));
                shaded.setGreenF(qBound(0.0, colour.greenF() * glow[slice], 1.0));
                shaded.setBlueF(qBound(0.0, colour.blueF() * glow[slice], 1.0));
                painter.fillRect(QRectF(x, from, barWidth, qMax(1.0, to - from)), shaded);
            }
            ++painted;
        }
        painter.end();
        if (!image.save(parts.at(1))) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(parts.at(1)));
            return 1;
        }
        // The numbers behind the picture: what a colour does globally is not visible by eye when
        // one band leads everywhere, and that is exactly the complaint to check for.
        // Nine buckets, and "white" first: with the bands driving RGB directly, dense material
        // legitimately comes out pale, and calling that "blue" because the blue channel is a shade
        // higher would misreport the picture.
        int hues[8] = {0, 0, 0, 0, 0, 0, 0, 0};
        const char *names[8] = {"white", "red", "orange", "yellow",
                                "green", "cyan", "blue", "violet"};
        for (int index = 0; index < pixels.size(); ++index) {
            const QVariantMap pixel = pixels.at(index).toMap();
            if (pixel.value(QStringLiteral("outside")).toBool()
                || pixel.value(QStringLiteral("amplitude")).toDouble() * middle < 0.5) {
                continue;
            }
            const QColor colour(pixel.value(QStringLiteral("rgb")).toString());
            int bucket = 0;
            if (colour.saturationF() > 0.15) {
                const int hue = int(colour.hsvHueF() * 360.0);
                if (hue < 20 || hue >= 340) {
                    bucket = 1;
                } else if (hue < 45) {
                    bucket = 2;
                } else if (hue < 70) {
                    bucket = 3;
                } else if (hue < 160) {
                    bucket = 4;
                } else if (hue < 210) {
                    bucket = 5;
                } else if (hue < 265) {
                    bucket = 6;
                } else {
                    bucket = 7;
                }
            }
            ++hues[bucket];
        }
        std::fprintf(stdout, "dj-ui: wrote %s, %d of %d columns painted\n",
                     qPrintable(parts.at(1)), painted, width);
        // How jagged the outline is: the mean distance of a bar from the average of its neighbours,
        // in pixels. A pixel moiré shows up here as a number around two; a smooth outline is under
        // one. Printed and asserted, because "looks grainy" is not something a gate can read.
        double roughness = 0.0;
        int compared = 0;
        for (int index = 1; index + 1 < pixels.size(); ++index) {
            const QVariantMap bar = pixels.at(index).toMap();
            if (bar.value(QStringLiteral("outside")).toBool()) {
                continue;
            }
            const double left = pixels.at(index - 1)
                                    .toMap()
                                    .value(QStringLiteral("amplitude"))
                                    .toDouble();
            const double right = pixels.at(index + 1)
                                     .toMap()
                                     .value(QStringLiteral("amplitude"))
                                     .toDouble();
            const double here = bar.value(QStringLiteral("amplitude")).toDouble();
            roughness += qAbs(here - 0.5 * (left + right)) * middle;
            ++compared;
        }
        if (compared > 0) {
            std::fprintf(stdout, "dj-ui: roughness %.2f px\n", roughness / compared);
        }
        std::fprintf(stdout, "dj-ui: hues");
        for (int index = 0; index < 8; ++index) {
            if (hues[index] > 0) {
                std::fprintf(stdout, " %s %d", names[index], hues[index]);
            }
        }
        std::fprintf(stdout, "\n");
        return 0;
    }

    if (parser.isSet(busyCheckOption)) {
        BridgeClient busyBridge;
        return runBusyCheck(busyBridge, parser.value(connectOption), parser.value(loadOption),
                            parser.value(loadSecondOption), 15000);
    }

    if (parser.isSet(gainCheckOption)) {
        BridgeClient gainBridge;
        return runGainCheck(gainBridge, parser.value(connectOption), 5000);
    }

    if (parser.isSet(selfCheckOption)) {
        const QString path = parser.isSet(connectOption) ? parser.value(connectOption)
                                                         : defaultSocketPath();
        return runSelfCheck(
            *bridge, waveform, path, 20000,
            parser.isSet(loadOption) ? parser.value(loadOption) : QString(),
            parser.isSet(loadSecondOption) ? parser.value(loadSecondOption) : QString(),
            parser.isSet(expectSpreadOption));
    }

    if (parser.isSet(dialogCheckOption)) {
        // The load dialog must list a directory without any desktop package: no GTK, no portal and
        // no QML extras, because a missing schema or module otherwise aborts the whole UI.
        browser.open(QString());
        const QVariantList entries = browser.entries();
        if (!browser.error().isEmpty()) {
            std::fprintf(stderr, "dj-ui: the file browser failed: %s\n", qPrintable(browser.error()));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: file browser lists %d entries in %s\n",
                     static_cast<int>(entries.size()), qPrintable(browser.folder()));
        return 0;
    }

    QQmlApplicationEngine engine;
    engine.loadFromModule(QStringLiteral("DjPlatform.Ui"), QStringLiteral("Main"));

    if (engine.rootObjects().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the QML scene did not load\n");
        return 1;
    }

    // The shape the window was left in: window, maximized or full screen. Restored before any check
    // runs, so a check sees the same window a person would.
    auto *mainWindow = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    if (mainWindow != nullptr) {
        // What full screen returns to. It lives in the session file too: a restart that lands in full
        // screen must still know whether it came from a maximized window or a plain one.
        auto restoreTo = std::make_shared<QString>(
            session.restoreTo().isEmpty() ? QStringLiteral("window") : session.restoreTo());
        // The scene shows its window as a plain one while loading. That first transition is not a
        // choice of the person, and writing it down would overwrite the shape the file remembers —
        // which is exactly what happened before this guard: every start forgot a maximized window.
        auto settled = std::make_shared<bool>(false);
        // Where the window is on its way to, when a transition takes two steps: leaving full screen goes
        // through a plain window before it becomes a maximized one, and that first step must not be
        // recorded as the shape the person chose.
        auto pending = std::make_shared<QString>();

        // F11 belongs to the window, not to whichever control happens to hold the focus: a filter sees
        // the key whatever the focus is, and a shortcut on the QML item would only fire when the shell
        // itself had it.
        class FullScreenKey : public QObject
        {
        public:
            FullScreenKey(QQuickWindow *window, std::shared_ptr<QString> restoreTo,
                          std::shared_ptr<QString> pending,
                          std::function<void(const QString &)> remember)
                : QObject(window)
                , m_window(window)
                , m_restoreTo(std::move(restoreTo))
                , m_pending(std::move(pending))
                , m_remember(std::move(remember))
            {
            }

        protected:
            bool eventFilter(QObject *watched, QEvent *event) override
            {
                if (event->type() != QEvent::KeyPress) {
                    return QObject::eventFilter(watched, event);
                }
                auto *key = static_cast<QKeyEvent *>(event);
                if (key->key() != Qt::Key_F11 || key->isAutoRepeat()) {
                    return QObject::eventFilter(watched, event);
                }
                if (m_window->visibility() == QQuickWindow::FullScreen) {
                    // Back to the shape it came from. showNormal() first: a window manager may ignore
                    // a request to become maximized while the window is still full screen.
                    const bool toMaximized = *m_restoreTo == QLatin1String("maximized");
                    *m_pending = toMaximized ? QStringLiteral("maximized") : QStringLiteral("window");
                    m_window->showNormal();
                    if (toMaximized) {
                        m_window->showMaximized();
                    }
                } else {
                    *m_restoreTo = m_window->visibility() == QQuickWindow::Maximized
                        ? QStringLiteral("maximized")
                        : QStringLiteral("window");
                    m_remember(*m_restoreTo);
                    m_window->showFullScreen();
                }
                return true;
            }

        private:
            QQuickWindow *m_window = nullptr;
            std::shared_ptr<QString> m_restoreTo;
            std::shared_ptr<QString> m_pending;
            std::function<void(const QString &)> m_remember;
        };

        mainWindow->installEventFilter(new FullScreenKey(
            mainWindow, restoreTo, pending, [&session](const QString &shape) {
                if (session.rememberRestoreTo(shape)) {
                    session.save();
                }
            }));

        QObject::connect(
            mainWindow, &QQuickWindow::visibilityChanged, mainWindow,
            [&session, mainWindow, settled, restoreTo, pending]() {
                if (!*settled) {
                    return;
                }
                QString shape;
                switch (mainWindow->visibility()) {
                case QQuickWindow::FullScreen:
                    shape = QStringLiteral("fullscreen");
                    break;
                case QQuickWindow::Maximized:
                    shape = QStringLiteral("maximized");
                    break;
                case QQuickWindow::Windowed:
                    shape = QStringLiteral("window");
                    break;
                default:
                    return;
                }
                if (!pending->isEmpty()) {
                    // A step on the way to the target: not the person's choice yet.
                    if (*pending != shape) {
                        return;
                    }
                    pending->clear();
                }
                bool changed = false;
                if (shape == QLatin1String("fullscreen")) {
                    changed = session.rememberWindow(shape);
                } else {
                    const bool wasFull = session.window() == QLatin1String("fullscreen");
                    changed = session.rememberWindow(shape);
                    if (wasFull) {
                        *restoreTo = shape;
                        changed = session.rememberRestoreTo(shape) || changed;
                    }
                }
                if (changed) {
                    session.save();
                }
            });

        // Applied once the window exists: on a real display a mode set before the surface is mapped is
        // dropped, and the window opens plain.
        const QString wanted = session.window();
        QTimer::singleShot(0, mainWindow, [mainWindow, wanted, settled]() {
            if (wanted == QLatin1String("maximized")) {
                mainWindow->showMaximized();
            } else if (wanted == QLatin1String("fullscreen")) {
                mainWindow->showFullScreen();
            }
            *settled = true;
        });
    }

    if (parser.isSet(cueWriteOption) || parser.isSet(cueReadOption)) {
        // A13: cues and loops are decisions about a file, so they must outlive the process. The write
        // check makes them, the read check finds them with no command of its own.
        const bool writing = parser.isSet(cueWriteOption);
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the cue check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto sourceFrame = [&]() {
            return deckA().value(QStringLiteral("source_frame")).toULongLong();
        };
        const auto waitFor = [&](const std::function<bool()> &ready, int milliseconds) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < milliseconds) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };

        // Earlier checks in the same engine leave a deck playing or loaded; start from an empty one,
        // because this check is about metadata and not about what ran before it.
        bridge->play(QStringLiteral("A"), false);
        bridge->eject(QStringLiteral("A"));
        waitFor([&]() {
            const QVariant load = deckA().value(QStringLiteral("load"));
            return load.typeId() == QMetaType::QString && load.toString() == QLatin1String("Empty");
        }, 2000);
        bridge->load(QStringLiteral("A"), fixture);

        const bool loaded = waitFor([&]() {
            const QVariant load = deckA().value(QStringLiteral("load"));
            if (load.typeId() == QMetaType::QString) {
                // A plain string is a state with no data: Empty, Ready.
                return load.toString() == QLatin1String("Ready");
            }
            // An object is Loading { progress } or Failed { message }; only the failure is final.
            return load.toMap().keys().value(0) == QLatin1String("Failed");
        }, 8000);
        if (!loaded) {
            std::fprintf(stderr, "dj-ui: the cue check could not load the track\n");
            return 1;
        }

        if (writing) {
            // A deck left playing by an earlier check would run past the frame while we look at it;
            // the cue has to land on a known frame, so stop first.
            bridge->play(QStringLiteral("A"), false);
            bridge->seek(QStringLiteral("A"), 48000);
            if (!waitFor([&]() { return sourceFrame() >= 48000; }, 3000)) {
                std::fprintf(stderr, "dj-ui: the deck did not reach frame 48000\n");
                return 1;
            }
            bridge->cue(QStringLiteral("A"), QStringLiteral("Set"), 2);
            // A loop is only armed around the playhead, which is how a hand makes one; park inside the
            // span first, then arm it, then save it into a pad.
            bridge->seek(QStringLiteral("A"), 120000);
            if (!waitFor([&]() { return sourceFrame() >= 120000; }, 3000)) {
                std::fprintf(stderr, "dj-ui: the deck did not reach frame 120000\n");
                return 1;
            }
            bridge->setLoop(QStringLiteral("A"), true, 96000, 144000);
            // A loop that is only active lives in memory; what a restart must bring back is the pad it was
            // saved into (DJP-DECK-004), so the check stores it there too.
            bridge->setPadLoop(QStringLiteral("A"), 6);
            // The check is about a file, not about memory: the engine writes queued edits on a thread
            // of its own, and this process is taken down the moment the check returns. So wait for the
            // file to hold the edit — otherwise a restart proves nothing and the last edit can die
            // with the process, which is exactly how this check used to fail now and then.
            const QString sidecar =
                QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
                QFileInfo(fixture).fileName() + QStringLiteral(".json");
            const bool stored = waitFor(
                [&]() {
                    QFile file(sidecar);
                    if (!file.open(QIODevice::ReadOnly)) {
                        return false;
                    }
                    const QJsonObject meta = QJsonDocument::fromJson(file.readAll()).object();
                    bool cueStored = false;
                    const QJsonArray cues = meta.value(QStringLiteral("cues")).toArray();
                    for (const QJsonValue &value : cues) {
                        const QJsonObject storedCue = value.toObject();
                        cueStored = cueStored ||
                                    (storedCue.value(QStringLiteral("slot")).toInt() == 2 &&
                                     storedCue.value(QStringLiteral("frame")).toVariant().toULongLong() ==
                                         48000);
                    }
                    bool loopStored = false;
                    const QJsonArray loops = meta.value(QStringLiteral("loops")).toArray();
                    for (const QJsonValue &value : loops) {
                        const QJsonObject storedLoop = value.toObject();
                        loopStored = loopStored ||
                                     (storedLoop.value(QStringLiteral("slot")).toInt() == 6 &&
                                      storedLoop.value(QStringLiteral("start")).toVariant().toULongLong() ==
                                          96000 &&
                                      storedLoop.value(QStringLiteral("end")).toVariant().toULongLong() ==
                                          144000);
                    }
                    return cueStored && loopStored;
                },
                3000);
            if (!stored) {
                std::fprintf(stderr, "dj-ui: the cue never reached %s\n", qPrintable(sidecar));
                return 1;
            }
            std::fprintf(stdout, "dj-ui: the cue and the pad loop are in %s\n", qPrintable(sidecar));
        }

        quint64 cue = 0;
        quint64 loopStart = 0;
        quint64 loopEnd = 0;
        const bool seen = waitFor(
            [&]() {
                const QVariantMap deck = deckA();
                const QVariantList hotcues = deck.value(QStringLiteral("hotcues")).toList();
                cue = hotcues.size() > 2 ? hotcues.at(2).toULongLong() : 0;
                loopStart = 0;
                loopEnd = 0;
                // pad_loops is positional: entry i belongs to pad i.
                const QVariantList loops = deck.value(QStringLiteral("pad_loops")).toList();
                if (loops.size() > 6) {
                    const QVariantMap loop = loops.at(6).toMap();
                    loopStart = loop.value(QStringLiteral("start")).toULongLong();
                    loopEnd = loop.value(QStringLiteral("end")).toULongLong();
                }
                return cue == 48000 && loopStart == 96000 && loopEnd == 144000;
            },
            6000);
        std::fprintf(stdout, "dj-ui: %s hotcue 2 at %llu, pad loop %llu..%llu\n",
                     writing ? "stored" : "restored", static_cast<unsigned long long>(cue),
                     static_cast<unsigned long long>(loopStart),
                     static_cast<unsigned long long>(loopEnd));
        if (!seen) {
            std::fprintf(stderr, "dj-ui: the cue or the pad loop did not survive\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: %s\n", writing ? "the cue and loop reached the engine"
                                                    : "the reload brought the cue and the pad loop back");
        return 0;
    }

    if (parser.isSet(masterCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QQuickItem *content = window->contentItem();

        // The window is sized by the session and is not shown in a headless run: give it the real
        // size, or every measurement below is taken against a 0x0 window and lies small.
        window->resize(1366, 768);
        window->show();
        QCoreApplication::processEvents(QEventLoop::AllEvents, 100);

        const auto item = [&](const QString &name) { return findItem(content, name); };
        auto *knob = item(QStringLiteral("masterKnob"));
        auto *meter = item(QStringLiteral("masterMeter"));
        auto *column = item(QStringLiteral("masterColumn"));
        auto *stripA = item(QStringLiteral("strip_A"));
        auto *stripB = item(QStringLiteral("strip_B"));
        auto *meterA = item(QStringLiteral("meter_A"));
        if (knob == nullptr || meter == nullptr || column == nullptr || stripA == nullptr
            || stripB == nullptr || meterA == nullptr) {
            std::fprintf(stderr, "dj-ui: the master column is not in the mixer\n");
            return 1;
        }
        // Between A and B, not at an end.
        const double columnMid = column->mapToItem(content, QPointF(column->width() / 2.0, 0)).x();
        const double aMid = stripA->mapToItem(content, QPointF(stripA->width() / 2.0, 0)).x();
        const double bMid = stripB->mapToItem(content, QPointF(stripB->width() / 2.0, 0)).x();
        std::fprintf(stdout, "dj-ui: the master column sits at %.0f px, between A at %.0f and B at %.0f\n",
                     columnMid, aMid, bMid);
        if (!(aMid < columnMid && columnMid < bMid)) {
            std::fprintf(stderr, "dj-ui: the master column is not between the A and B strips\n");
            return 1;
        }
        // Round, and above the meter.
        std::fprintf(stdout, "dj-ui: the master knob is %.0f x %.0f px\n", knob->width(),
                     knob->height());
        if (std::abs(knob->width() - knob->height()) > 1.0) {
            std::fprintf(stderr, "dj-ui: the master knob is not round\n");
            return 1;
        }
        // Every meter in the mixer is the same height as the master's, and they end on one line: the
        // meter band is defined by the master column, where the knob takes the room above it.
        const double knobBottom = knob->mapToItem(content, QPointF(0, knob->height())).y();
        const double meterTop = meter->mapToItem(content, QPointF(0, 0)).y();
        const double meterBottom = meter->mapToItem(content, QPointF(0, meter->height())).y();
        const double deckMeterTop = meterA->mapToItem(content, QPointF(0, 0)).y();
        const double deckMeterBottom = meterA->mapToItem(content, QPointF(0, meterA->height())).y();
        std::fprintf(stdout, "dj-ui: the master meter starts %.0f px below the knob and ends %.0f px "
                             "from the deck meters; heights %.0f and %.0f px\n",
                     meterTop - knobBottom, meterBottom - deckMeterBottom, meter->height(),
                     meterA->height());
        auto *row = item(QStringLiteral("stripRow"));
        const auto place = [&](QQuickItem *what, const char *name) {
            if (what == nullptr) {
                std::fprintf(stdout, "dj-ui:   %s missing\n", name);
                return;
            }
            const QPointF at = what->mapToItem(content, QPointF(0, 0));
            std::fprintf(stdout, "dj-ui:   %s y %.0f h %.0f\n", name, at.y(), what->height());
        };
        if (meterTop < knobBottom) {
            std::fprintf(stderr, "dj-ui: the master knob overlaps the meter\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the meter band is %.0f px inside a %.0f px strip row\n",
                     meter->height(), row != nullptr ? row->height() : -1.0);
        if (std::abs(meter->height() - meterA->height()) > 1.0) {
            std::fprintf(stderr, "dj-ui: the deck meters are not the master's height\n");
            return 1;
        }
        if (std::abs(meterBottom - deckMeterBottom) > 2.0) {
            std::fprintf(stderr, "dj-ui: the meters do not line up at the bottom\n");
            return 1;
        }

        // Three colours, and none of them the accent: the master must be findable by colour alone.
        const QString green = settings.themeMeterGreen();
        const QString yellow = settings.themeMeterYellow();
        const QString red = settings.themeMeterRed();
        std::fprintf(stdout, "dj-ui: master meter colours %s %s %s\n", qPrintable(green),
                     qPrintable(yellow), qPrintable(red));
        if (green == yellow || yellow == red || green == red || green == settings.themeAccent()) {
            std::fprintf(stderr, "dj-ui: the master meter colours do not separate from each other\n");
            return 1;
        }



        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        // The knob's command must land in the published gain, which is what the label and the pointer
        // read: a knob that moves a value nobody publishes is a knob that springs back.
        const auto gainNow = [&]() {
            return bridge->snapshot().value(QStringLiteral("master_gain")).toDouble();
        };
        bridge->setMasterGain(0.5);
        QElapsedTimer waiting;
        waiting.start();
        double seen = gainNow();
        while (waiting.elapsed() < 3000 && std::abs(seen - 0.5) > 0.01) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            seen = gainNow();
            QThread::msleep(10);
        }
        std::fprintf(stdout, "dj-ui: master gain 0.5 -> snapshot says %.2f\n", seen);
        if (std::abs(seen - 0.5) > 0.01) {
            std::fprintf(stderr, "dj-ui: the master gain did not reach the engine\n");
            return 1;
        }

        // And the meter must see the signal, not just exist: load a track, play it, watch the peak.
        const QString fixture = parser.value(loadOption);
        if (!fixture.isEmpty()) {
            // Another check may have left deck A playing to its end, where a play command produces
            // silence. Empty the deck first: this check is about the master meter, not about what ran
            // before it in the same engine.
            bridge->play(QStringLiteral("A"), false);
            bridge->eject(QStringLiteral("A"));
            QElapsedTimer clearing;
            clearing.start();
            // A snapshot still saying Ready describes the track that was there a moment ago: wait for
            // the deck to report itself empty, or the load below is believed before it happened and a
            // play command lands on nothing.
            while (clearing.elapsed() < 2000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                const QVariantList decksNow =
                    bridge->snapshot().value(QStringLiteral("decks")).toList();
                bool ready = false;
                for (const QVariant &entry : decksNow) {
                    const QVariantMap deck = entry.toMap();
                    if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                        const QVariant load = deck.value(QStringLiteral("load"));
                        const QString name = load.typeId() == QMetaType::QString
                            ? load.toString()
                            : load.toMap().keys().value(0);
                        ready = name == QLatin1String("Ready");
                    }
                }
                if (!ready) {
                    break;
                }
                QThread::msleep(10);
            }
            // A load can be refused for a moment after the eject: try, wait for Ready, try again.
            const auto loadState = [&]() {
                const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
                for (const QVariant &entry : decks) {
                    const QVariantMap deck = entry.toMap();
                    if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                        const QVariant load = deck.value(QStringLiteral("load"));
                        // serde writes a variant with data as an object with one key, and one without
                        // as a plain string.
                        return load.typeId() == QMetaType::QString
                            ? load.toString()
                            : load.toMap().keys().value(0);
                    }
                }
                return QString();
            };
            QString load;
            for (int attempt = 0; attempt < 3 && load != QLatin1String("Ready"); ++attempt) {
                bridge->load(QStringLiteral("A"), fixture);
                QElapsedTimer settling;
                settling.start();
                while (settling.elapsed() < 4000) {
                    QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    load = loadState();
                    if (load == QLatin1String("Ready") || load == QLatin1String("Failed")) {
                        break;
                    }
                    QThread::msleep(20);
                }
            }
            if (load != QLatin1String("Ready")) {
                std::fprintf(stderr, "dj-ui: the master check could not load a track\n");
                return 1;
            }
            bridge->play(QStringLiteral("A"), true);
            QElapsedTimer playing;
            playing.start();
            double peak = 0.0;
            while (playing.elapsed() < 3000 && peak <= 0.0) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                peak = bridge->snapshot().value(QStringLiteral("master")).toMap()
                           .value(QStringLiteral("peak_left")).toDouble();
                QThread::msleep(20);
            }
            // Where the signal stops, if it stops: the deck's own meter and the transport state say
            // whether the master is silent because nothing plays or because nothing is routed.
            bool playingA = false;
            double deckPeak = 0.0;
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    playingA = deck.value(QStringLiteral("playing")).toBool();
                    deckPeak = deck.value(QStringLiteral("levels")).toMap()
                                   .value(QStringLiteral("peak_left")).toDouble();
                }
            }
            std::fprintf(stdout, "dj-ui: deck A playing %s, its own peak %.3f, master peak %.3f\n",
                         playingA ? "yes" : "no", deckPeak, peak);
            // The live meter, still bound to the snapshot: -9 dB is well inside the green.
            std::fprintf(stdout, "dj-ui: the master meter reads %s at %.3f\n",
                         qPrintable(meter->property("zone").toString()), peak);
            if (meter->property("zone").toString() != QLatin1String("green")) {
                std::fprintf(stderr, "dj-ui: the master meter colours the live level wrongly\n");
                return 1;
            }

            if (peak <= 0.0) {
                std::fprintf(stderr, "dj-ui: the master meter stays at zero with a deck playing\n");
                return 1;
            }
            bridge->play(QStringLiteral("A"), false);
        }
        // The colour rules, asked of the meter itself: green means room, yellow means close to
        // 0 dB, red means at or past it. A meter whose colours were squeezed into its own bar showed
        // red at the top of every level.
        const auto zoneAt = [&](double linear) {
            // The zone is a property of the meter, so ask it: feeding it a level and reading the
            // answer tests the rule, not a copy of it.
            meter->setProperty("peak", QVariant(linear));
            return meter->property("zone").toString();
        };
        const QString zoneLow = zoneAt(0.1);    // -20 dB
        const QString zoneAt6 = zoneAt(0.45);   // -6.9 dB
        const QString zoneNear = zoneAt(0.6);   // -4.4 dB
        const QString zoneFull = zoneAt(1.0);   // 0 dB, the top edge
        std::fprintf(stdout, "dj-ui: master meter zones: -20 dB %s, -6.9 dB %s, -4.4 dB %s, 0 dB %s\n",
                     qPrintable(zoneLow), qPrintable(zoneAt6), qPrintable(zoneNear),
                     qPrintable(zoneFull));
        if (zoneLow != QLatin1String("green") || zoneAt6 != QLatin1String("green")
            || zoneNear != QLatin1String("yellow") || zoneFull != QLatin1String("red")) {
            std::fprintf(stderr, "dj-ui: the master meter colours do not follow the level\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: master meter yellow from %.0f dB, red from %.0f dB, top %.0f dB\n",
                     meter->property("yellowFromDb").toDouble(),
                     meter->property("redFromDb").toDouble(),
                     meter->property("ceilingDb").toDouble());

        // And the picture itself: a gradient runs top-down while the thresholds are measured from the
        // bottom, so the colours can be logical in the properties and upside down on screen. Drive the
        // meter past 0 dB — that is the only place red exists — and read the pixels: red at the top,
        // yellow under it, green at the foot.
        meter->setProperty("peak", QVariant(1.0));
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
        const QImage frame = window->grabWindow();
        if (frame.isNull()) {
            std::fprintf(stdout, "dj-ui: the window would not render, so the meter colours are unread\n");
        } else {
            const QPointF origin = meter->mapToItem(content, QPointF(0, 0));
            const double ratio = content->width() > 0
                ? static_cast<double>(frame.width()) / content->width() : 1.0;
            const auto sample = [&](double downFromTop) {
                const int x = static_cast<int>((origin.x() + meter->width() / 2.0) * ratio);
                const int y = static_cast<int>((origin.y() + downFromTop) * ratio);
                return frame.pixelColor(x, y);
            };
            const double height = meter->height();
            const QColor head = sample(height * 0.03);
            const QColor middle = sample(height * 0.12);
            const QColor foot = sample(height * 0.90);
            std::fprintf(stdout, "dj-ui: at full scale the master meter reads %s at the top, %s under "
                                 "it and %s at the foot\n",
                         qPrintable(head.name()), qPrintable(middle.name()), qPrintable(foot.name()));
            if (!(head.red() > head.green() + 40 && head.red() > head.blue() + 40)) {
                std::fprintf(stderr, "dj-ui: the master meter is not red at the top\n");
                return 1;
            }
            if (!(middle.red() > 150 && middle.green() > 120 && middle.blue() < 100)) {
                std::fprintf(stderr, "dj-ui: the master meter is not yellow under the red\n");
                return 1;
            }
            if (!(foot.green() > foot.red() + 20)) {
                std::fprintf(stderr, "dj-ui: the master meter is not green at the bottom\n");
                return 1;
            }
        }

        std::fprintf(stdout, "dj-ui: the master knob and meter work\n");
        return 0;
    }

    if (parser.isSet(curveCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const QStringList laws = {QStringLiteral("constant_power"), QStringLiteral("linear"),
                                  QStringLiteral("smooth"), QStringLiteral("sharp")};
        const auto published = [&]() {
            return bridge->snapshot().value(QStringLiteral("crossfader_curve")).toString();
        };
        for (const QString &law : laws) {
            bridge->setCrossfaderCurve(law);
            QElapsedTimer waiting;
            waiting.start();
            QString seen = published();
            while (waiting.elapsed() < 3000 && seen != law) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                seen = published();
                QThread::msleep(10);
            }
            std::fprintf(stdout, "dj-ui: curve %s -> snapshot says %s\n", qPrintable(law),
                         qPrintable(seen.isEmpty() ? QStringLiteral("nothing") : seen));
            // The buttons must show the same law the engine published: exactly one highlighted.
            QCoreApplication::processEvents();
            QStringList lit;
            QStringList dark;
            for (const QString &candidate : laws) {
                auto *button = findItem(window->contentItem(),
                                        QStringLiteral("curveButton_") + candidate);
                if (button == nullptr) {
                    std::fprintf(stderr, "dj-ui: no button for %s\n", qPrintable(candidate));
                    return 1;
                }
                const QColor fill = button->property("color").value<QColor>();
                if (fill == QColor(QStringLiteral("#181818"))) {
                    lit.append(candidate);
                } else {
                    dark.append(candidate);
                }
            }
            std::fprintf(stdout, "dj-ui:   highlighted %s, plain %s\n",
                         qPrintable(lit.join(QStringLiteral(","))),
                         qPrintable(dark.join(QStringLiteral(","))));
            if (lit.size() != 1 || lit.first() != law) {
                std::fprintf(stderr, "dj-ui: the selector does not show the law in force\n");
                return 1;
            }
            if (seen != law) {
                std::fprintf(stderr, "dj-ui: the engine did not take the %s law\n", qPrintable(law));
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: every crossfader law travels to the engine and back\n");
        return 0;
    }

    if (parser.isSet(windowCheckOption)) {
        if (!parser.isSet(sessionFileOption) || session.file().isEmpty()) {
            std::fprintf(stderr, "dj-ui: --window-check needs --session-file\n");
            return 1;
        }
        if (mainWindow == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        // The restore is applied from the event loop, once the window exists — that is how it behaves
        // on a real display. So the check waits for it instead of reading the shape a microsecond
        // after the scene was built.
        QElapsedTimer settling;
        settling.start();
        while (settling.elapsed() < 2000 && !mainWindow->isVisible()) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(5);
        }
        QCoreApplication::processEvents();

        const auto stateOf = [&]() {
            switch (mainWindow->visibility()) {
            case QQuickWindow::FullScreen:
                return QStringLiteral("fullscreen");
            case QQuickWindow::Maximized:
                return QStringLiteral("maximized");
            default:
                return QStringLiteral("window");
            }
        };
        const auto remembers = [&]() {
            QFile file(session.file());
            if (!file.open(QIODevice::ReadOnly)) {
                return QString();
            }
            const QJsonObject root = QJsonDocument::fromJson(file.readAll()).object();
            return root.value(QStringLiteral("window")).toString();
        };
        const auto pressF11 = [&]() {
            QKeyEvent press(QEvent::KeyPress, Qt::Key_F11, Qt::NoModifier);
            QCoreApplication::sendEvent(mainWindow, &press);
            QKeyEvent release(QEvent::KeyRelease, Qt::Key_F11, Qt::NoModifier);
            QCoreApplication::sendEvent(mainWindow, &release);
            QCoreApplication::processEvents();
        };

        const QString wanted = session.window();
        const QString opened = stateOf();
        const QString writtenAtStart = remembers();
        std::fprintf(stdout, "dj-ui: the window opened %s (wanted %s, file says %s)\n",
                     qPrintable(opened), qPrintable(wanted), qPrintable(writtenAtStart));
        if (opened != wanted) {
            std::fprintf(stderr, "dj-ui: the window shape was not restored\n");
            return 1;
        }
        // The scene shows a plain window while it loads; that must not be written over the memory.
        if (writtenAtStart != wanted) {
            std::fprintf(stderr, "dj-ui: the start overwrote the remembered shape\n");
            return 1;
        }
        // F11 toggles: from full screen it leaves — back to the shape remembered as its origin — and
        // from anything else it fills.
        const QString expected = opened == QLatin1String("fullscreen")
            ? (session.restoreTo().isEmpty() ? QStringLiteral("window") : session.restoreTo())
            : QStringLiteral("fullscreen");
        pressF11();
        const QString afterFirst = stateOf();
        const QString afterFirstFile = remembers();
        std::fprintf(stdout, "dj-ui: F11 -> %s, file says %s\n", qPrintable(afterFirst),
                     qPrintable(afterFirstFile));
        if (afterFirst != expected || afterFirstFile != expected) {
            std::fprintf(stderr, "dj-ui: F11 gave %s, wanted %s\n", qPrintable(afterFirst),
                         qPrintable(expected));
            return 1;
        }
        pressF11();
        const QString back = stateOf();
        const QString backFile = remembers();
        std::fprintf(stdout, "dj-ui: F11 again -> %s, file says %s\n", qPrintable(back),
                     qPrintable(backFile));
        if (back != opened || backFile != opened) {
            std::fprintf(stderr, "dj-ui: leaving full screen did not return to %s\n",
                         qPrintable(opened));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the window keeps its shape across F11 and a restart\n");
        return 0;
    }

    // A frame of the strip, timed: the same code the deck runs, painted into an image instead of a
    // window, so the number means something on a machine without a display.
    if (parser.isSet(benchOption)) {
        const QString path = parser.value(loadOption);
        Waveform envelope;
        if (path.isEmpty()) {
            // A five-minute track at one hundred columns per second: the size a real envelope has,
            // so the figure does not flatter a tiny fixture.
            envelope.loadSynthetic(30000);
        } else if (!envelope.load(path)) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(envelope.error()));
            return 1;
        }
        bool ok = false;
        const int frames = parser.value(benchOption).toInt(&ok);
        const int count = ok && frames > 0 ? frames : 200;
        QImage image(1200, 160, QImage::Format_RGB32);
        WaveformStyle style;
        style.background = QColor(QStringLiteral("#141414"));
        style.outside = QColor(QStringLiteral("#101010"));
        style.flat = QColor(QStringLiteral("#5b636d"));
        style.border = QColor(QStringLiteral("#383838"));
        style.accent = QColor(QStringLiteral("#5090f0"));
        style.marker = QColor(QStringLiteral("#e2c07f"));
        style.playhead = QColor(QStringLiteral("#f0f3f6"));
        style.barWidth = settings.waveformBarWidth();
        style.barGap = settings.waveformBarGap();

        std::vector<double> costs;
        costs.reserve(std::size_t(count));
        for (int frame = 0; frame < count; ++frame) {
            // Scroll across the track, the way a second of playback at fiftyfold zoom does.
            const double start = 0.5 * double(frame) / double(count);
            QElapsedTimer timer;
            timer.start();
            QPainter painter(&image);
            paintWaveform(painter, envelope, style, image.rect(), start, 0.1, start + 0.05, -1.0,
                          {}, {}, false, 0.0, 0.0, false, -1.0, {}, {});
            painter.end();
            costs.push_back(double(timer.nsecsElapsed()) / 1.0e6);
        }
        std::sort(costs.begin(), costs.end());
        const auto at = [&costs](double share) {
            const std::size_t index = std::size_t(qBound(0.0, share, 1.0) * double(costs.size() - 1));
            return costs[index];
        };
        std::fprintf(stdout,
                     "dj-ui: frame cost p50 %.2f ms, p99 %.2f ms, max %.2f ms over %d frames "
                     "at %d Hz (%d ms apart)\n",
                     at(0.50), at(0.99), costs.back(), count, settings.frameHz(),
                     settings.frameInterval());
        // A frame has a 16.7 ms budget at 60 Hz; taking more than half of it to paint the strip
        // means the strip, not the display, sets the frame rate.
        if (at(0.99) > 8.0) {
            std::fprintf(stderr, "dj-ui: a frame costs %.2f ms, too much to scroll smoothly\n",
                         at(0.99));
            return 1;
        }
        return 0;
    }

    // The arrangement, measured from the objects that were actually built: a layout setting that the
    // shell quietly ignores is worse than no setting at all.
    if (parser.isSet(layoutCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();
        auto *content = window->contentItem();
        auto *globalBar = findItem(content, QStringLiteral("globalBar"));
        auto *deckArea = findItem(content, QStringLiteral("deckArea"));
        auto *browser = findItem(content, QStringLiteral("browserPanel"));
        auto *mixer = findItem(content, QStringLiteral("mixer"));
        if (globalBar == nullptr || deckArea == nullptr || browser == nullptr || mixer == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no regions by name\n");
            return 1;
        }
        auto *shell = globalBar->parentItem();
        if (shell == nullptr || shell->height() <= 0.0 || deckArea->width() <= 0.0) {
            std::fprintf(stderr, "dj-ui: the shell has no size yet\n");
            return 1;
        }
        const double height = shell->height();
        const double width = deckArea->width();
        const double globalShare = globalBar->height() / height * 100.0;
        const double deckShare = deckArea->height() / height * 100.0;
        const double browserShare = browser->height() / height * 100.0;
        const double mixerShare = mixer->width() / width * 100.0;

        const auto report = [&](const char *name, double actual, double wanted) {
            std::fprintf(stdout, "dj-ui: layout %-8s %5.1f%% (wanted %4.1f%%)\n", name, actual,
                         wanted);
        };
        report("global", globalShare, layout.globalPercent());
        report("decks", deckShare, layout.deckRowsPercent());
        report("browser", browserShare, layout.browserPercent());
        report("mixer", mixerShare, layout.mixerPercent());

        const auto off = [](double actual, double wanted) {
            return std::abs(actual - wanted) > 2.0;
        };
        if (off(globalShare, layout.globalPercent()) || off(mixerShare, layout.mixerPercent())) {
            std::fprintf(stderr, "dj-ui: the layout does not match its settings\n");
            return 1;
        }
        if (off(deckShare, layout.deckRowsPercent()) || off(browserShare, layout.browserPercent())) {
            std::fprintf(stderr, "dj-ui: the layout does not match its settings\n");
            return 1;
        }
        // Each deck, measured: a deck that grows tall enough to swallow the window is what "too
        // puffy" means, so the numbers are printed next to the layout's.
        for (const char *id : {"A", "B", "C", "D"}) {
            auto *deck = findItem(content, QStringLiteral("deckPanel_%1").arg(QString::fromLatin1(id)));
            auto *detail = findItem(content, QStringLiteral("deckDetail_%1").arg(QString::fromLatin1(id)));
            auto *overview = findItem(content, QStringLiteral("deckOverview_%1").arg(QString::fromLatin1(id)));
            if (deck == nullptr || detail == nullptr || overview == nullptr) {
                std::fprintf(stderr,
                             "dj-ui: deck %s has no strips (panel %s, strip %s, overview %s)\n", id,
                             deck != nullptr ? "found" : "missing",
                             detail != nullptr ? "found" : "missing",
                             overview != nullptr ? "found" : "missing");
                return 1;
            }
            std::fprintf(stdout, "dj-ui: deck %s %.0f px tall, strip %.0f px, overview %.0f px\n", id,
                         deck->height(), detail->height(), overview->height());
        }
        // At rest nothing plays, so every deck button in the browser must look the same: a button
        // that wears a selection colour just because it is the first in the row says nothing true.
        {
            QColor first;
            for (const char *id : {"A", "B", "C", "D"}) {
                auto *button = findItem(content,
                                        QStringLiteral("deckButton_%1").arg(QString::fromLatin1(id)));
                if (button == nullptr) {
                    std::fprintf(stderr, "dj-ui: the browser has no button for deck %s\n", id);
                    return 1;
                }
                const QColor face = button->property("faceColor").value<QColor>();
                if (first.isValid() && face != first) {
                    std::fprintf(stderr, "dj-ui: deck %s looks different at rest (%s against %s)\n",
                                 id, qPrintable(face.name()), qPrintable(first.name()));
                    return 1;
                }
                first = face;
            }
            std::fprintf(stdout, "dj-ui: deck buttons at rest are all %s\n",
                         qPrintable(first.name()));
            if (first != QColor(settings.themeRaised())) {
                std::fprintf(stderr, "dj-ui: deck buttons do not wear the resting colour\n");
                return 1;
            }
        }

        // The zoom a deck opens at, measured from the item itself.
        if (auto *stripView = findItem(content, QStringLiteral("deckDetail_A")); stripView != nullptr) {
            const double span = stripView->property("zoomSpan").toDouble();
            const double factor = span > 0.0 ? 1.0 / span : 0.0;
            std::fprintf(stdout, "dj-ui: deck A opens at %.1fx zoom (wanted %.1fx)\n", factor,
                         settings.waveformZoom());
            if (std::abs(factor - settings.waveformZoom()) > 0.05) {
                std::fprintf(stderr, "dj-ui: the deck does not open at the zoom setting\n");
                return 1;
            }
        }

        // The mixer, measured: the strips in the order the setting names, and the crossfader handle
        // resting in the middle at 0.5 rather than against the left stop.
        QStringList placed;
        for (const QChar letter : layout.channelOrder()) {
            auto *strip = findItem(content, QStringLiteral("strip_%1").arg(letter));
            if (strip == nullptr) {
                std::fprintf(stderr, "dj-ui: the mixer has no strip %s\n", qPrintable(letter));
                return 1;
            }
            placed.append(QStringLiteral("%1@%2").arg(letter).arg(int(strip->x())));
        }
        std::fprintf(stdout, "dj-ui: mixer order %s (wanted %s)\n", qPrintable(placed.join(QStringLiteral(" "))),
                     qPrintable(layout.channelOrder()));

        auto *handle = findItem(content, QStringLiteral("crossfaderHandle"));
        if (handle == nullptr || handle->parentItem() == nullptr) {
            std::fprintf(stderr, "dj-ui: the mixer has no crossfader handle\n");
            return 1;
        }
        const double centre = handle->x() + handle->width() / 2.0;
        const double travel = handle->parentItem()->width();
        const double offset = centre - travel / 2.0;
        std::fprintf(stdout, "dj-ui: crossfader handle %.1f px from the middle of %.0f px\n", offset,
                     travel);
        if (std::abs(offset) > 2.0) {
            std::fprintf(stderr, "dj-ui: the crossfader does not sit in the middle at rest\n");
            return 1;
        }

        // The crossfader belongs under the strips, not above them.
        if (auto *stripA = findItem(content, QStringLiteral("strip_A")); stripA != nullptr) {
            const double handleTop = handle->mapToScene(QPointF(0.0, 0.0)).y();
            const double stripBottom = stripA->mapToScene(QPointF(0.0, stripA->height())).y();
            std::fprintf(stdout, "dj-ui: crossfader top %.0f px, strips bottom %.0f px\n", handleTop,
                         stripBottom);
            if (handleTop < stripBottom) {
                std::fprintf(stderr, "dj-ui: the crossfader is above the strips\n");
                return 1;
            }
        }

        // The fader and the meter are one rectangle, so they must agree on their size.
        auto *knob = findItem(content, QStringLiteral("fader_A"));
        auto *meter = findItem(content, QStringLiteral("meter_A"));
        if (knob != nullptr && meter != nullptr) {
            std::fprintf(stdout, "dj-ui: knob %.0f px, meter %.0f px, difference %.0f px\n",
                         knob->height(), meter->height(), std::abs(knob->height() - meter->height()));
            if (std::abs(knob->height() - meter->height()) > 2.0) {
                std::fprintf(stderr, "dj-ui: the knob is not the height of its meter\n");
                return 1;
            }
        }
        // At rest nothing is playing, so no deck frame may wear the accent: a highlight that sits on
        // one deck for ever is the bug the owner saw.
        const QColor accent = QColor(settings.themeAccent());
        QStringList framed;
        for (const char *id : {"A", "B", "C", "D"}) {
            auto *frame = findItem(content, QStringLiteral("deckFrame_%1").arg(QString::fromLatin1(id)));
            if (frame == nullptr) {
                std::fprintf(stderr, "dj-ui: deck %s has no frame\n", id);
                return 1;
            }
            auto *pen = frame->property("border").value<QObject *>();
            const QColor colour = pen != nullptr ? pen->property("color").value<QColor>() : QColor();
            if (colour == accent) {
                std::fprintf(stderr, "dj-ui: deck %s is highlighted while nothing plays\n", id);
                return 1;
            }
            framed.append(QStringLiteral("%1 %2").arg(QString::fromLatin1(id), colour.name()));
        }
        std::fprintf(stdout, "dj-ui: deck frames at rest: %s\n",
                     qPrintable(framed.join(QStringLiteral(", "))));
        // The gain pointer rides on the meter: same rectangle, so the strip gives none of its width
        // away to a separate column.
        for (const char *id : {"A", "B", "C", "D"}) {
            auto *stripKnob = findItem(content, QStringLiteral("fader_%1").arg(QString::fromLatin1(id)));
            auto *stripMeter = findItem(content, QStringLiteral("meter_%1").arg(QString::fromLatin1(id)));
            if (stripKnob == nullptr || stripMeter == nullptr) {
                std::fprintf(stderr, "dj-ui: deck %s has no knob or meter\n", id);
                return 1;
            }
            const QPointF knobAt = stripKnob->mapToScene(QPointF(0, 0));
            const QPointF meterAt = stripMeter->mapToScene(QPointF(0, 0));
            const double shift = std::abs(knobAt.x() - meterAt.x())
                + std::abs(knobAt.y() - meterAt.y())
                + std::abs(stripKnob->width() - stripMeter->width())
                + std::abs(stripKnob->height() - stripMeter->height());
            std::fprintf(stdout, "dj-ui: deck %s pointer on the meter, %.0f px apart\n", id, shift);
            if (shift > 2.0) {
                std::fprintf(stderr, "dj-ui: deck %s has its pointer beside the meter, not on it\n", id);
                return 1;
            }
        }

        // And the fader has to be painted, not merely placed: at rest it stands at unity, which for a
        // channel fader is the top of the scale, so a bright line must cross the meter there over a
        // dark background.
        if (auto *pointer = findItem(content, QStringLiteral("fader_A"));
            pointer != nullptr) {
            const QImage frame = window->grabWindow();
            if (!frame.isNull()) {
                const QPointF origin = pointer->mapToScene(QPointF(0, 0));
                const double ratio = content->width() > 0
                    ? static_cast<double>(frame.width()) / content->width() : 1.0;
                // The fader at unity sits on the first line of the scale, three pixels in from the top
                // edge — the same inset the knob draws its travel with.
                const double unityY = origin.y() + 3.0;
                // Away from the middle: the grip sits there, and this is a check about the line.
                const double sampleX = origin.x() + 4.0;
                const QColor onLine = frame.pixelColor(static_cast<int>(sampleX * ratio),
                                                       static_cast<int>(unityY * ratio));
                const QColor below = frame.pixelColor(
                    static_cast<int>(sampleX * ratio),
                    static_cast<int>((unityY + pointer->height() * 0.08) * ratio));
                std::fprintf(stdout, "dj-ui: the gain pointer draws %s at unity over %s\n",
                             qPrintable(onLine.name()), qPrintable(below.name()));
                if (!(onLine.lightness() > below.lightness() + 60)) {
                    std::fprintf(stderr, "dj-ui: the gain pointer is not drawn on the meter\n");
                    return 1;
                }
            }
        }

        // The four laws are one row of buttons a hand can hit, each with its law drawn inside. One
        // row, because vertical room belongs to the strips: the check measures that they really share
        // a row, that the row fits the column, and that each button is still big enough to read.
        QStringList laws = {QStringLiteral("constant_power"), QStringLiteral("linear"),
                            QStringLiteral("smooth"), QStringLiteral("sharp")};
        double rowTop = -1.0;
        double rowBottom = 0.0;
        double narrowest = 1e9;
        QString narrowestLaw;
        QRectF row;
        for (const QString &law : laws) {
            auto *button = findItem(content, QStringLiteral("curveButton_") + law);
            if (button == nullptr) {
                std::fprintf(stderr, "dj-ui: no button for %s\n", qPrintable(law));
                return 1;
            }
            // Position in the mixer panel's own coordinate system, so the comparison is about layout
            // and not about where the panel happens to sit.
            const QPointF top = button->mapToItem(content, QPointF(0, 0));
            const QRectF box(top, QSizeF(button->width(), button->height()));
            row = row.isNull() ? box : row.united(box);
            if (button->height() < 30.0) {
                std::fprintf(stderr, "dj-ui: the %s button is %.0f px tall, too small\n",
                             qPrintable(law), button->height());
                return 1;
            }
            if (button->width() < narrowest) {
                narrowest = button->width();
                narrowestLaw = law;
            }
            if (rowTop < 0.0) {
                rowTop = top.y();
            } else if (std::abs(top.y() - rowTop) > 0.5) {
                std::fprintf(stderr, "dj-ui: the %s button is on another row\n", qPrintable(law));
                return 1;
            }
            rowBottom = qMax(rowBottom, top.y() + button->height());
        }
        std::fprintf(stdout, "dj-ui: the four laws sit in one row %.0f px wide, %.0f px tall, "
                             "narrowest %s at %.0f px\n",
                     row.width(), rowBottom - rowTop, qPrintable(narrowestLaw), narrowest);
        if (auto *last = findItem(content, QStringLiteral("strip_D")); last != nullptr) {
            const double room = last->x() + last->width();
            if (row.width() > room + 0.5) {
                std::fprintf(stderr, "dj-ui: the law row is wider than the column\n");
                return 1;
            }
        }
        if (narrowest < 28.0) {
            std::fprintf(stderr, "dj-ui: a law button is only %.0f px wide\n", narrowest);
            return 1;
        }

        // The picture has to be there: a blank button says nothing. And the four pictures have to
        // differ — one drawing repeated four times would be a lie about there being four laws.
        QMap<QString, QImage> glyphs;
        for (const QString &law : laws) {
            auto *button = findItem(content, QStringLiteral("curveButton_") + law);
            bool ready = false;
            QImage image;
            const QSharedPointer<QQuickItemGrabResult> grab = button->grabToImage();
            QObject::connect(grab.data(), &QQuickItemGrabResult::ready, [&]() {
                image = grab->image();
                ready = true;
            });
            QElapsedTimer waiting;
            waiting.start();
            while (!ready && waiting.elapsed() < 3000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                QThread::msleep(5);
            }
            if (!ready || image.isNull()) {
                std::fprintf(stderr, "dj-ui: cannot grab the %s button\n", qPrintable(law));
                return 1;
            }
            int inked = 0;
            for (int y = 0; y < image.height(); ++y) {
                for (int x = 0; x < image.width(); ++x) {
                    if (image.pixelColor(x, y).lightness() > 90) {
                        ++inked;
                    }
                }
            }
            std::fprintf(stdout, "dj-ui: the %s button draws %d px\n", qPrintable(law), inked);
            if (inked < 30) {
                std::fprintf(stderr, "dj-ui: the %s button is blank\n", qPrintable(law));
                return 1;
            }
            glyphs.insert(law, image);
        }
        const QImage reference = glyphs.value(QStringLiteral("constant_power"));
        for (auto it = glyphs.cbegin(); it != glyphs.cend(); ++it) {
            if (it.key() != QStringLiteral("constant_power") && it.value() == reference) {
                std::fprintf(stderr, "dj-ui: the %s picture is identical to constant power\n",
                             qPrintable(it.key()));
                return 1;
            }
        }

        // The wheel has to reach 100x, not 50x: the deepest step is the one a person tries when they
        // want a single beat across the strip.
        if (auto *viewA = findItem(content, QStringLiteral("deckDetail_A")); viewA != nullptr) {
            for (int step = 0; step < 16; ++step) {
                QMetaObject::invokeMethod(viewA, "zoomBy", Q_ARG(QVariant, QVariant(2.0)));
            }
            const double deepest = viewA->property("zoomSpan").toDouble();
            std::fprintf(stdout, "dj-ui: the wheel reaches %.1fx zoom\n",
                         deepest > 0.0 ? 1.0 / deepest : 0.0);
            if (std::abs(deepest - 0.0025) > 1e-9) {
                std::fprintf(stderr, "dj-ui: the deepest zoom is %.5f, wanted 0.0025 (400x)\n", deepest);
                return 1;
            }
            QMetaObject::invokeMethod(viewA, "zoomBy", Q_ARG(QVariant, QVariant(0.000001)));
        }

        // The wheel trims a channel — over the knob and over the meter, because that is where the
        // pointer actually is when a person scrolls. A handler on the strip missed both.
        if (auto *stripA = findItem(content, QStringLiteral("strip_A")); stripA != nullptr) {
            const auto wheelAt = [&](QQuickItem *target, int notches) {
                const QPointF centre =
                    target->mapToScene(QPointF(target->width() / 2.0, target->height() / 2.0));
                QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(),
                                  QPoint(0, 120 * notches), Qt::NoButton, Qt::NoModifier,
                                  Qt::NoScrollPhase, false);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
                return stripA->property("lastWheelFader").toDouble();
            };
            auto *knobA = findItem(content, QStringLiteral("fader_A"));
            auto *meterA = findItem(content, QStringLiteral("meter_A"));
            if (knobA == nullptr || meterA == nullptr) {
                std::fprintf(stderr, "dj-ui: strip A has no fader or meter\n");
                return 1;
            }

            // The pointer rides on the meter, so the wheel has one surface. A channel fader stands at
            // unity when it is open, and unity is the top of its scale — so the test goes down first
            // and comes back up, otherwise "up" has nowhere to go and a working wheel reads as dead.
            const double downOnce = wheelAt(meterA, -1);
            const double upAgain = wheelAt(meterA, 1);
            std::fprintf(stdout, "dj-ui: wheel over the meter %.3f down, %.3f back up\n", downOnce,
                         upAgain);
            if (!(downOnce < 1.0)) {
                std::fprintf(stderr, "dj-ui: the wheel does nothing over the meter\n");
                return 1;
            }
            if (!(upAgain > downOnce)) {
                std::fprintf(stderr, "dj-ui: the wheel up does not take the fader back\n");
                return 1;
            }
            Q_UNUSED(knobA);
        }

        // A knob the mouse cannot reach is not a knob. Send a real press at three quarters of its
        // height and read back what it decided to send.
        if (knob != nullptr) {
            const QPointF point = knob->mapToScene(QPointF(knob->width() / 2.0, knob->height() * 0.25));
            for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
                QMouseEvent event(type, point, window->mapToGlobal(point.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
            }
            QCoreApplication::processEvents();
            const double lastY = knob->property("lastY").toDouble();
            std::fprintf(stdout, "dj-ui: fader A press at %.0f px -> y %.1f, position %.3f\n",
                         point.y(), lastY, knob->property("lastPosition").toDouble());
            if (lastY < 0.0) {
                std::fprintf(stderr, "dj-ui: the knob did not receive the mouse\n");
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: layout %s applied as set\n", qPrintable(layout.preset()));
        return 0;
    }

    if (parser.isSet(wheelCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();

        // Two controls, two numbers: the wheel over the gain knob moves the trim and must leave the
        // fader alone. A single number shared by both was the defect this check exists for.
        auto *trimA = findItem(window->contentItem(), QStringLiteral("trim_A"));
        if (trimA == nullptr) {
            std::fprintf(stderr, "dj-ui: no gain knob for deck A\n");
            return 1;
        }
        const auto fieldOfA = [&](const QString &field) {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck.value(field).toDouble();
                }
            }
            return 0.0;
        };
        const double gainBefore = fieldOfA(QStringLiteral("linear_gain"));
        const double faderBefore = fieldOfA(QStringLiteral("fader"));
        const QPointF centre = trimA->mapToScene(QPointF(trimA->width() / 2.0,
                                                          trimA->height() / 2.0));
        for (int notch = 0; notch < 3; ++notch) {
            QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(),
                              QPoint(0, 120), Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(60);
        }
        QElapsedTimer waiting;
        waiting.start();
        double gainAfter = gainBefore;
        while (waiting.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            gainAfter = fieldOfA(QStringLiteral("linear_gain"));
            if (std::abs(gainAfter - gainBefore) > 0.01) {
                break;
            }
            QThread::msleep(20);
        }
        const double faderAfter = fieldOfA(QStringLiteral("fader"));
        std::fprintf(stdout, "dj-ui: wheel over the gain knob: gain %.3f -> %.3f, fader %.3f\n",
                     gainBefore, gainAfter, faderAfter);
        if (std::abs(gainAfter - gainBefore) <= 0.01) {
            std::fprintf(stderr, "dj-ui: the wheel did not reach the engine\n");
            return 1;
        }
        if (std::abs(faderAfter - faderBefore) > 0.01) {
            std::fprintf(stderr, "dj-ui: the gain wheel moved the fader\n");
            return 1;
        }
        bridge->setChannelGain(QStringLiteral("A"), 1.0);

        // A notch is a fixed turn, not a fixed number of decibels: scroll one notch up and one back down
        // and the pointer must land where it started. The decibels need not come back — the two halves of
        // the law are asymmetric on purpose — which is why the angle is what is measured here.
        const auto pump = [&]() {
            for (int step = 0; step < 40; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        const auto wheelAngle = [&](int notches) {
            QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(),
                              QPoint(0, 120 * notches), Qt::NoButton, Qt::NoModifier,
                              Qt::NoScrollPhase, false);
            QCoreApplication::sendEvent(window, &event);
            pump();
            return trimA->property("angle").toDouble();
        };
        pump();
        const double angleAtRest = trimA->property("angle").toDouble();
        const double angleUp = wheelAngle(1);
        const double angleDown = wheelAngle(-1);
        const double angleBack = wheelAngle(-1);
        const double angleUpAgain = wheelAngle(1);
        std::fprintf(stdout,
                     "dj-ui: one notch turns the gain knob %+.1f deg up and %+.1f deg down (back at "
                     "%.1f and %.1f deg)\n",
                     angleUp - angleAtRest, angleBack - angleDown, angleDown, angleUpAgain);
        if (std::abs((angleUp - angleAtRest) - 13.5) > 1.0
            || std::abs((angleBack - angleDown) + 13.5) > 1.0) {
            std::fprintf(stderr, "dj-ui: the wheel turns the gain knob by different steps each way\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the wheel trims the gain and leaves the fader where it was\n");
        return 0;
    }

    if (parser.isSet(gainCurveCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        // The knobs are drawn at the size a person sees: the law is measured in pixels of hand.
        window->resize(1366, 768);
        window->show();
        QCoreApplication::processEvents(QEventLoop::AllEvents, 100);
        QEventLoop settle;
        QTimer::singleShot(200, &settle, &QEventLoop::quit);
        settle.exec();
        QQuickItem *content = window->contentItem();
        auto *trim = findItem(content, QStringLiteral("trim_A"));
        auto *master = findItem(content, QStringLiteral("masterKnob"));
        if (trim == nullptr || master == nullptr) {
            std::fprintf(stderr, "dj-ui: no level knobs in the mixer\n");
            return 1;
        }
        // Both knobs answer the same questions, which is what makes one check enough.
        // A QML function takes and returns `QVariant`, not `double`, whatever it is written as.
        const auto ask = [&](QQuickItem *knob, const char *name, double argument, double *out) {
            QVariant answer;
            if (!QMetaObject::invokeMethod(knob, name, Q_RETURN_ARG(QVariant, answer),
                                           Q_ARG(QVariant, QVariant(argument)))) {
                std::fprintf(stderr, "dj-ui: the knob does not answer %s\n", name);
                return false;
            }
            *out = answer.toDouble();
            return true;
        };
        const double capSide = trim->property("capSide").toDouble();
        const double degreesPerPixel = 270.0 / capSide;
        // A rate, not an average: a hair of a degree and a hundredth of a pixel, so the numbers below are
        // the slope at a spot rather than a chord across a fifth of the travel. The chord is what made an
        // earlier version of this check pass for the wrong shape.
        const double hair = 0.01;
        const double trimRange = 40.0;
        const double masterRange = 60.0;
        const double masterTravel = master->property("travelPixels").toDouble();
        // The trim: ends exact, then the slope at unity and at the very end of the cut.
        double unity = 0.0;
        double bottom = 0.0;
        double top = 0.0;
        double firstHair = 0.0;
        double lastHairBefore = 0.0;
        double lastHair = 0.0;
        if (!ask(trim, "dbOf", 0.0, &unity) || !ask(trim, "dbOf", -135.0, &bottom)
            || !ask(trim, "dbOf", 135.0, &top) || !ask(trim, "dbOf", -hair, &firstHair)
            || !ask(trim, "dbOf", -135.0 + hair, &lastHairBefore)
            || !ask(trim, "dbOf", -135.0, &lastHair)) {
            return 1;
        }
        const double trimAtUnity = std::abs(firstHair) / hair * degreesPerPixel;
        const double trimAtEnd = std::abs(lastHair - lastHairBefore) / hair * degreesPerPixel;
        // The master drags in pixels, so its law takes two arguments.
        const auto drag = [&](double pixels, double fromDb, double *out) {
            QVariant answer;
            if (!QMetaObject::invokeMethod(master, "dbForPixels", Q_RETURN_ARG(QVariant, answer),
                                           Q_ARG(QVariant, QVariant(pixels)),
                                           Q_ARG(QVariant, QVariant(fromDb)))) {
                std::fprintf(stderr, "dj-ui: the master does not answer dbForPixels\n");
                return false;
            }
            *out = answer.toDouble();
            return true;
        };
        double masterFirst = 0.0;
        double masterLastBefore = 0.0;
        double masterLast = 0.0;
        if (!drag(0.01, 0.0, &masterFirst) || !drag(masterTravel - 0.01, 0.0, &masterLastBefore)
            || !drag(masterTravel, 0.0, &masterLast)) {
            return 1;
        }
        const double masterAtUnity = std::abs(masterFirst) / 0.01;
        const double masterAtEnd = std::abs(masterLast - masterLastBefore) / 0.01;
        std::fprintf(stdout,
                     "dj-ui: the gain curve is %s at %.2f; the trim moves %.3f dB per pixel at unity "
                     "and %.3f at the end of the cut; the master moves %.3f dB per pixel at unity and "
                     "%.3f at the bottom\n",
                     qPrintable(settings.gainCurve()), settings.gainCurveMiddle(), trimAtUnity,
                     trimAtEnd, masterAtUnity, masterAtEnd);
        if (std::abs(unity) > 0.01 || std::abs(bottom + 40.0) > 0.01 || std::abs(top - 6.0) > 0.01) {
            std::fprintf(stderr,
                         "dj-ui: the ends of the trim are not where they were: %.2f / %.2f / %.2f\n",
                         unity, bottom, top);
            return 1;
        }
        // What the law promises, whatever the configured shape: the straight rate scaled by the
        // configured share at unity, and by the rest of the average slope at the end.
        const double middle = settings.gainCurveMiddle();
        const bool curved = settings.gainCurve() == QStringLiteral("cubic");
        const double promisedTrim = trimRange * (curved ? middle : 1.0) / 135.0 * degreesPerPixel;
        const double promisedTrimEnd =
            trimRange * (curved ? 2.0 - middle : 1.0) / 135.0 * degreesPerPixel;
        const double promisedMaster =
            masterRange * (curved ? middle : 1.0) / masterTravel;
        const auto near = [](double measured, double promised) {
            return std::abs(measured - promised) <= std::max(1e-4, promised * 0.05);
        };
        if (!near(trimAtUnity, promisedTrim)) {
            std::fprintf(stderr,
                         "dj-ui: the trim moves %.3f dB per pixel at unity, and the curve promises "
                         "%.3f\n",
                         trimAtUnity, promisedTrim);
            return 1;
        }
        if (!near(trimAtEnd, promisedTrimEnd)) {
            std::fprintf(stderr,
                         "dj-ui: the trim moves %.3f dB per pixel at the end of the cut, and the "
                         "curve promises %.3f\n",
                         trimAtEnd, promisedTrimEnd);
            return 1;
        }
        if (!near(masterAtUnity, promisedMaster)) {
            std::fprintf(stderr,
                         "dj-ui: the master moves %.3f dB per pixel at unity, and the curve promises "
                         "%.3f\n",
                         masterAtUnity, promisedMaster);
            return 1;
        }
        // The channel faders: same family, their own middle, and the ends fixed at unity and silence.
        auto *fader = findItem(content, QStringLiteral("fader_A"));
        if (fader == nullptr) {
            std::fprintf(stderr, "dj-ui: no channel fader in the mixer\n");
            return 1;
        }
        double faderTop = 0.0;
        double faderBottom = 0.0;
        double faderFirst = 0.0;
        double faderLastBefore = 0.0;
        if (!ask(fader, "valueAtHandle", 1.0, &faderTop) || !ask(fader, "valueAtHandle", 0.0, &faderBottom)
            || !ask(fader, "valueAtHandle", 1.0 - hair, &faderFirst)
            || !ask(fader, "valueAtHandle", hair, &faderLastBefore)) {
            return 1;
        }
        const double faderAtTop = (faderTop - faderFirst) / hair;
        const double faderAtBottom = faderLastBefore / hair;
        // The fader's own shape, not the knobs': the two are configured apart, and a check that reads the
        // wrong one passes for the wrong reason.
        const bool faderCubic = settings.faderCurve() == QStringLiteral("cubic");
        const double promisedFader = faderCubic ? settings.faderCurveMiddle() : 1.0;
        const double promisedFaderBottom = faderCubic ? 2.0 - settings.faderCurveMiddle() : 1.0;
        std::fprintf(stdout,
                     "dj-ui: the channel fader moves %.3f of its range per unit of travel at the top "
                     "(curve %s at %.2f) and %.3f at the bottom\n",
                     faderAtTop, qPrintable(settings.faderCurve()), settings.faderCurveMiddle(),
                     faderAtBottom);
        if (std::abs(faderTop - 1.0) > 0.001 || std::abs(faderBottom) > 0.001) {
            std::fprintf(stderr,
                         "dj-ui: the fader's ends are not unity and silence: %.3f / %.3f\n",
                         faderTop, faderBottom);
            return 1;
        }
        const bool logarithmic = settings.faderCurve() == QStringLiteral("logarithmic");
        if (logarithmic) {
            // A B-taper is a promise about decibels, not about slopes: a quarter of the throw takes the
            // level down about ten decibels and half of it about eighteen, which is what makes the top
            // third of a fader the fine one.
            double quarter = 0.0;
            double half = 0.0;
            if (!ask(fader, "valueAtHandle", 0.75, &quarter) || !ask(fader, "valueAtHandle", 0.5, &half)) {
                return 1;
            }
            const double quarterDb = 20.0 * std::log10(std::max(quarter, 1e-9));
            const double halfDb = 20.0 * std::log10(std::max(half, 1e-9));
            std::fprintf(stdout,
                         "dj-ui: the fader at a quarter of the throw is %.1f dB, at half %.1f dB, and "
                         "it moves %.2f times as fast at the top as at the bottom\n",
                         quarterDb, halfDb, faderAtBottom > 0.0 ? faderAtTop / faderAtBottom : 0.0);
            if (quarterDb > -8.0 || quarterDb < -14.0) {
                std::fprintf(stderr,
                             "dj-ui: a quarter of the fader's throw takes only %.1f dB, so the top is not "
                             "the fine part\n",
                             quarterDb);
                return 1;
            }
            if (halfDb > -14.0 || halfDb < -24.0) {
                std::fprintf(stderr, "dj-ui: half the throw takes %.1f dB\n", halfDb);
                return 1;
            }
            if (!(faderAtTop > faderAtBottom * 3.0)) {
                std::fprintf(stderr,
                             "dj-ui: the fader is not steeper at the top (%.3f against %.3f per unit of "
                             "travel)\n",
                             faderAtTop, faderAtBottom);
                return 1;
            }
        } else if (!near(faderAtTop, promisedFader) || !near(faderAtBottom, promisedFaderBottom)) {
            std::fprintf(stderr,
                         "dj-ui: the fader moves %.3f at the top and %.3f at the bottom, and the curve "
                         "promises %.3f and %.3f\n",
                         faderAtTop, faderAtBottom, promisedFader, promisedFaderBottom);
            return 1;
        }
        if (curved && !(trimAtEnd > trimAtUnity * 1.3)) {
            std::fprintf(stderr,
                         "dj-ui: the end of the trim is not faster than its middle (%.3f against "
                         "%.3f dB per pixel)\n",
                         trimAtEnd, trimAtUnity);
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the level knobs follow the %s curve\n",
                     qPrintable(settings.gainCurve()));
        return 0;
    }

    if (parser.isSet(tempoCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --tempo-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        bridge->load(QStringLiteral("A"), fixture);
        double bpm = 0.0;
        quint64 firstBeat = 0;
        // The loader decodes the file twice (level and tempo) before it says Ready, so the wait is
        // generous on purpose: a slow first load is not a defect.
        bool measured = false;
        QElapsedTimer waiting;
        waiting.start();
        while (waiting.elapsed() < 20000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            const QVariantMap track = deckA().value(QStringLiteral("track")).toMap();
            const QVariant tempo = track.value(QStringLiteral("bpm"));
            if (tempo.isValid() && !tempo.isNull()) {
                bpm = tempo.toDouble();
                firstBeat = track.value(QStringLiteral("first_beat_frame")).toULongLong();
                measured = true;
                break;
            }
            QThread::msleep(10);
        }
        if (!measured) {
            std::fprintf(stderr, "dj-ui: the engine measured no tempo\n");
            return 1;
        }
        // What the DJ sees, not only what the snapshot holds: the label is part of the feature.
        // The label is a QQuickItem, not a plain QObject child: the scene is a tree of items, and a
        // `findChild` on the window finds the C++ wrappers only by accident.
        auto *label = findItem(window->contentItem(), QStringLiteral("bpm_A"));
        const QString shown = label != nullptr ? label->property("text").toString() : QString();
        std::fprintf(stdout, "dj-ui: tempo %.2f BPM, first beat %llu, label \"%s\"\n", bpm,
                     static_cast<unsigned long long>(firstBeat), qPrintable(shown));
        if (bpm <= 0.0 || firstBeat == 0) {
            std::fprintf(stderr, "dj-ui: the deck published no usable grid\n");
            return 1;
        }
        if (!shown.contains(QStringLiteral("BPM"))) {
            std::fprintf(stderr, "dj-ui: the deck shows no tempo (label \"%s\")\n",
                         qPrintable(shown));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the measured tempo reached the deck\n");
        return 0;
    }

    if (parser.isSet(loopBeatsOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --loop-beats-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto gridFirstBeat = [&]() -> qint64 {
            const QVariantList segments = deckA()
                                              .value(QStringLiteral("track"))
                                              .toMap()
                                              .value(QStringLiteral("grid"))
                                              .toMap()
                                              .value(QStringLiteral("segments"))
                                              .toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto loopRegion = [&]() {
            return deckA().value(QStringLiteral("loop_region")).toMap();
        };
        // A Rectangle with a MouseArea has no `clicked` signal: the press is a mouse event at its centre.
        const auto clickAt = [](QQuickItem *item) {
            const QPointF centre = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            QMouseEvent press(QEvent::MouseButtonPress, centre, centre, Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QMouseEvent release(QEvent::MouseButtonRelease, centre, centre, Qt::LeftButton,
                                Qt::NoButton, Qt::NoModifier);
            QCoreApplication::sendEvent(item->window(), &press);
            QCoreApplication::sendEvent(item->window(), &release);
            QCoreApplication::processEvents();
        };
        const auto click = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName) && gridFirstBeat() >= 0;
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to loop by\n");
            return 1;
        }
        // The grid's own numbers, not the deck's headline tempo: the loop is built from the segment the
        // play head is in, and the check has to measure the same thing the deck did.
        const QVariantList segments = deckA()
                                          .value(QStringLiteral("track"))
                                          .toMap()
                                          .value(QStringLiteral("grid"))
                                          .toMap()
                                          .value(QStringLiteral("segments"))
                                          .toList();
        if (segments.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the deck published no grid segments\n");
            return 1;
        }
        const QVariantMap first = segments.first().toMap();
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        const double bpm = first.value(QStringLiteral("bpm")).toDouble();
        if (rate <= 0 || !(bpm > 0.0)) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or tempo\n");
            return 1;
        }
        const double period = rate * 60.0 / bpm;
        const qint64 anchor = first.value(QStringLiteral("frame")).toLongLong();
        const auto onGrid = [&](qint64 frame) {
            const double lines = (static_cast<double>(frame) - static_cast<double>(anchor)) / period;
            const double off = std::fabs(lines - std::round(lines)) * period;
            return off <= 2.0;
        };
        // Snap and quantize, on a pair of presses made off the grid on purpose: the start goes to the
        // nearest line and the end is a whole number of beats after it (DJP-DECK-005).
        const auto seekTo = [&](qint64 frame) {
            bridge->seek(QStringLiteral("A"), static_cast<qulonglong>(frame));
            return waitFor([&]() {
                return std::llabs(deckA().value(QStringLiteral("source_frame")).toLongLong() - frame)
                    <= 2;
            }, 5000);
        };
        const auto loopStart = [&]() -> qint64 {
            const QVariantMap region = loopRegion();
            return region.isEmpty() ? -1 : region.value(QStringLiteral("start")).toLongLong();
        };
        const qint64 markFrom = anchor + static_cast<qint64>(period * 3.0) + 137;
        const qint64 markTo = anchor + static_cast<qint64>(period * 6.4) - 211;
        if (!seekTo(markFrom)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach the snap mark\n");
            return 1;
        }
        auto *panelItem = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
        if (panelItem == nullptr) {
            std::fprintf(stderr, "dj-ui: no deck panel to press Loop on\n");
            return 1;
        }
        const auto pressLoop = [&]() {
            QMetaObject::invokeMethod(panelItem, "pressLoop", Q_ARG(QVariant, QVariant(false)));
            QCoreApplication::processEvents();
            return true;
        };
        if (!pressLoop()) {
            return 1;
        }
        if (!seekTo(markTo)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach the quantize mark\n");
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!waitFor([&]() { return loopStart() >= 0; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the two presses made no loop\n");
            return 1;
        }
        const qint64 snappedStart = loopStart();
        const qint64 snappedEnd = loopRegion().value(QStringLiteral("end")).toLongLong();
        if (!onGrid(snappedStart) || !onGrid(snappedEnd)) {
            std::fprintf(stderr, "dj-ui: the loop edges are off the grid: %lld..%lld (anchor %lld, "
                                 "period %.1f)\n",
                         snappedStart, snappedEnd, anchor, period);
            return 1;
        }
        const double beats = static_cast<double>(snappedEnd - snappedStart) / period;
        if (std::fabs(beats - std::round(beats)) > 0.02 || std::round(beats) < 1.0) {
            std::fprintf(stderr, "dj-ui: the loop is %.3f beats long, not a whole number\n", beats);
            return 1;
        }
        const auto snappedBeats = static_cast<qint64>(std::llround(beats));
        // The switches are the DJ's: turned off, the very same pair must land where the hand was and not
        // on the grid. A switch that only looks lit is worse than none.
        auto *snapSwitch = findItem(window->contentItem(), QStringLiteral("snap_A"));
        auto *quantSwitch = findItem(window->contentItem(), QStringLiteral("quantize_A"));
        if (snapSwitch == nullptr || quantSwitch == nullptr) {
            std::fprintf(stderr, "dj-ui: no snap or quantize switch\n");
            return 1;
        }
        if (!snapSwitch->property("on").toBool() || !quantSwitch->property("on").toBool()) {
            std::fprintf(stderr, "dj-ui: the switches do not start on\n");
            return 1;
        }
        clickAt(snapSwitch);
        clickAt(quantSwitch);
        if (snapSwitch->property("on").toBool() || quantSwitch->property("on").toBool()) {
            std::fprintf(stderr, "dj-ui: the switches did not turn off\n");
            return 1;
        }
        // Clear it, so the size loop below is not refused for crossing this one.
        bridge->setLoop(QStringLiteral("A"), false, 0, 0);
        if (!waitFor([&]() { return loopStart() < 0; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the loop outlived the clear\n");
            return 1;
        }
        const qint64 rawFrom = anchor + static_cast<qint64>(period * 2.3) + 211;
        const qint64 rawTo = anchor + static_cast<qint64>(period * 5.7) - 173;
        if (!seekTo(rawFrom)) {
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!seekTo(rawTo)) {
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!waitFor([&]() { return loopStart() >= 0; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the two presses with the switches off made no loop\n");
            return 1;
        }
        const qint64 rawStart = loopStart();
        if (onGrid(rawStart)) {
            std::fprintf(stderr,
                         "dj-ui: the loop snapped to %lld with snap and quantize off — the switches do "
                         "nothing\n",
                         rawStart);
            return 1;
        }
        const qint64 rawEnd = loopRegion().value(QStringLiteral("end")).toLongLong();
        bridge->setLoop(QStringLiteral("A"), false, 0, 0);
        if (!waitFor([&]() { return loopStart() < 0; }, 5000)) {
            return 1;
        }
        // Back on for the rest of the check, as a deck starts.
        clickAt(snapSwitch);
        clickAt(quantSwitch);
        const auto four = static_cast<qint64>(std::llround(4.0 * period));
        if (!click(QStringLiteral("loopSize_A_4"))) {
            return 1;
        }
        const auto loopLength = [&]() -> qint64 {
            const QVariantMap region = loopRegion();
            if (region.isEmpty()) {
                return -1;
            }
            return region.value(QStringLiteral("end")).toLongLong()
                 - region.value(QStringLiteral("start")).toLongLong();
        };
        if (!waitFor([&]() { return std::llabs(loopLength() - four) <= 2; }, 5000)) {
            std::fprintf(stderr, "dj-ui: a four beat loop came back %lld frames long, wanted %lld (start %s)\n",
                         loopLength(), four, qPrintable(loopRegion().value(QStringLiteral("start")).toString()));
            return 1;
        }
        const qint64 start = loopRegion().value(QStringLiteral("start")).toLongLong();
        // The start has to be on a beat of the grid, not on the play head: that is the whole point.
        const double where = std::fmod(static_cast<double>(start - gridFirstBeat()), period);
        if (where > 2.0 && period - where > 2.0) {
            std::fprintf(stderr, "dj-ui: the loop starts %.1f frames off the beat\n", where);
            return 1;
        }
        if (!click(QStringLiteral("loopOn_A"))) {
            return 1;
        }
        const auto beatStep = static_cast<qint64>(std::llround(period));
        if (!waitFor(
                [&]() {
                    const qint64 moved = loopRegion().value(QStringLiteral("start")).toLongLong()
                                       - start;
                    return std::llabs(moved - beatStep) <= 2
                        && std::llabs(loopLength() - four) <= 2;
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: the arrow moved the loop to frame %s, and it is %lld long\n",
                         qPrintable(loopRegion().value(QStringLiteral("start")).toString()),
                         loopLength());
            return 1;
        }
        // Every loop is the same loop: the one the size button made has to light the Loop button, and a
        // press inside it has to put the mode out — that is how a DJ leaves a loop.
        auto *loopButton = findItem(window->contentItem(), QStringLiteral("loop_A"));
        if (loopButton == nullptr) {
            std::fprintf(stderr, "dj-ui: no Loop button to read\n");
            return 1;
        }
        if (!waitFor([&]() { return deckA().value(QStringLiteral("loop_mode")).toBool(); }, 5000)) {
            std::fprintf(stderr, "dj-ui: the loop did not put the mode on\n");
            return 1;
        }
        if (!loopButton->property("active").toBool()) {
            std::fprintf(stderr, "dj-ui: the Loop button is not lit over an active loop\n");
            return 1;
        }
        if (!pressLoop()) {
            return 1;
        }
        if (!waitFor(
                [&]() {
                    return !deckA().value(QStringLiteral("loop_mode")).toBool()
                        && !loopButton->property("active").toBool();
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: a press on the lit Loop button left the mode on\n");
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: a four beat loop is %lld frames of the grid (a beat is %.1f), it starts on a "
                     "beat at frame %lld, and one press of the arrow moved it %.1f frames without "
                     "changing its length; two presses off the grid snapped to %lld..%lld (%lld whole "
                     "beats), with the switches off the same pair landed on %lld (%s the grid), the size "
                     "loop lit the Loop button, and a press inside it put the mode out\n",
                     four, period, start, static_cast<double>(beatStep), snappedStart, snappedEnd,
                     snappedBeats, rawStart, onGrid(rawStart) ? "on" : "off");
        return 0;
    }

    if (parser.isSet(transportExtrasOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --transport-extras-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = deckA()
                                              .value(QStringLiteral("track"))
                                              .toMap()
                                              .value(QStringLiteral("grid"))
                                              .toMap()
                                              .value(QStringLiteral("segments"))
                                              .toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto playHead = [&]() -> qint64 {
            return deckA().value(QStringLiteral("source_frame")).toLongLong();
        };
        const auto press = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };
        const auto hold = [&](const QString &name, const char *signal) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to hold\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, signal);
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName) && firstBeat() >= 0;
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to jump on\n");
            return 1;
        }
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        const double bpm = deckA().value(QStringLiteral("local_bpm")).toDouble();
        if (rate <= 0 || !(bpm > 0.0)) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or tempo\n");
            return 1;
        }
        const double period = rate * 60.0 / bpm;
        const qint64 first = firstBeat();
        // Between two beats and not on a round frame: a jump that only looks right from a beat proves
        // nothing about landing on one.
        const qint64 start = first + static_cast<qint64>(period * 1.5) + 311;
        bridge->seek(QStringLiteral("A"), start);
        if (!waitFor([&]() { return std::llabs(playHead() - start) <= 2; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach frame %lld (%lld)\n", start,
                         playHead());
            return 1;
        }
        // Every length the panel promises is a button: twelve of them, and a missing one is a jump a DJ
        // cannot make.
        for (const QString &name : {QStringLiteral("m64"), QStringLiteral("m32"), QStringLiteral("m16"),
                                    QStringLiteral("m8"), QStringLiteral("m4"), QStringLiteral("m1"),
                                    QStringLiteral("p1"), QStringLiteral("p4"), QStringLiteral("p8"),
                                    QStringLiteral("p16"), QStringLiteral("p32"), QStringLiteral("p64")}) {
            if (findItem(window->contentItem(), QStringLiteral("jump_A_") + name) == nullptr) {
                std::fprintf(stderr, "dj-ui: the panel has no jump button for %s\n", qPrintable(name));
                return 1;
            }
        }
        if (!press(QStringLiteral("jump_A_p1"))) {
            return 1;
        }
        const auto onTheGrid = [&]() {
            const double place = static_cast<double>(playHead() - first) / period;
            return std::fabs(place - std::round(place)) * period <= 2.0;
        };
        if (!waitFor([&]() { return playHead() > start && onTheGrid(); }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: the jump did not land on a beat (start %lld, head %lld, period "
                         "%.1f)\n",
                         start, playHead(), period);
            return 1;
        }
        const qint64 jump = playHead() - start;
        // The platter: held, the state says the deck is bent; released, it says the bend is gone.
        if (!hold(QStringLiteral("nudgeOn_A"), "pressed")) {
            return 1;
        }
        if (!waitFor(
                [&]() { return deckA().value(QStringLiteral("nudge")).toDouble() > 0.9; }, 3000)) {
            std::fprintf(stderr, "dj-ui: the nudge never reached the deck (nudge %.3f)\n",
                         deckA().value(QStringLiteral("nudge")).toDouble());
            return 1;
        }
        if (!hold(QStringLiteral("nudgeOn_A"), "released")) {
            return 1;
        }
        if (!waitFor(
                [&]() { return std::fabs(deckA().value(QStringLiteral("nudge")).toDouble()) < 1e-6; },
                3000)) {
            std::fprintf(stderr, "dj-ui: letting go left the deck bent (nudge %.3f)\n",
                         deckA().value(QStringLiteral("nudge")).toDouble());
            return 1;
        }
        // Slip: on, the state carries a virtual head; off, it carries none.
        if (!press(QStringLiteral("slip_A"))) {
            return 1;
        }
        if (!waitFor(
                [&]() {
                    const QVariantMap deck = deckA();
                    return deck.value(QStringLiteral("slip")).toBool()
                           && deck.value(QStringLiteral("slip_frame")).isValid();
                },
                3000)) {
            std::fprintf(stderr, "dj-ui: slip did not reach the state\n");
            return 1;
        }
        // The virtual head has to be on the picture, not only in the state: a jump under slip moves the
        // sound and leaves the virtual head behind, and both strips have to show that difference.
        const qint64 beforeSlipJump = playHead();
        if (!press(QStringLiteral("jump_A_p1"))) {
            return 1;
        }
        if (!waitFor([&]() { return playHead() != beforeSlipJump; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the jump under slip did not move the sound\n");
            return 1;
        }
        auto *overview = findItem(window->contentItem(), QStringLiteral("overviewStrip_A"));
        if (overview == nullptr) {
            std::fprintf(stderr, "dj-ui: the overview strip is not there to draw the virtual head\n");
            return 1;
        }
        if (overview->property("drawnSlip").toInt() != 1) {
            std::fprintf(stderr, "dj-ui: the overview drew no virtual head (slip %.4f)\n",
                         overview->property("slipRatio").toDouble());
            return 1;
        }
        const double slipMark = overview->property("slipRatio").toDouble();
        const double heardMark = overview->property("playheadRatio").toDouble();
        if (std::fabs(slipMark - heardMark) < 1e-4) {
            std::fprintf(stderr,
                         "dj-ui: the virtual head sits on the play head after a jump (both %.4f)\n",
                         slipMark);
            return 1;
        }
        auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        if (strip == nullptr || strip->property("slipRatio").toDouble() < 0.0) {
            std::fprintf(stderr, "dj-ui: the main strip was never told where the virtual head is\n");
            return 1;
        }
        if (!press(QStringLiteral("slip_A"))) {
            return 1;
        }
        if (!waitFor(
                [&]() { return !deckA().value(QStringLiteral("slip")).toBool(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: slip stayed on after the second press\n");
            return 1;
        }
        // Reverse: the button exists and the material turns around under it. A track still streaming
        // from disk refuses — the deck says so in its state and the head keeps going forward — so the
        // press is repeated until the entry is in the cache, and only then is the head expected to
        // walk down. Nothing here reads a timer into the verdict: the state is the verdict.
        if (findItem(window->contentItem(), QStringLiteral("reverse_A")) == nullptr) {
            std::fprintf(stderr, "dj-ui: the panel has no reverse button\n");
            return 1;
        }
        // Reverse needs a moving deck with material on both sides of the head. Whether the deck is
        // paused has nothing to do with reverse, so the check puts it in a state where the question is
        // only about reverse: somewhere in the middle of the file, playing.
        const qint64 target = first + static_cast<qint64>(period * 8.0);
        bridge->seek(QStringLiteral("A"), target);
        waitFor([&]() { return std::llabs(playHead() - target) <= 2; }, 5000);
        if (!deckA().value(QStringLiteral("playing")).toBool() && !press(QStringLiteral("play_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return playHead() > target; }, 4000)) {
            std::fprintf(stderr, "dj-ui: the deck is not moving before the reverse test (%lld)\n",
                         playHead());
            return 1;
        }
        // The cache: the engine holds this track in memory, and the deck says so in its own state.
        // The handover happens a few seconds after the load (the entry is built behind the deck), so
        // the check waits instead of reading a single snapshot.
        if (!waitFor(
                [&]() {
                    return deckA().value(QStringLiteral("cached")).toBool()
                        && deckA().value(QStringLiteral("cached_bytes")).toDouble() > 0.0;
                },
                20000)) {
            std::fprintf(stderr,
                         "dj-ui: the deck never reported its material in memory (cache off?)\n");
            return 1;
        }
        const double cachedMb =
            deckA().value(QStringLiteral("cached_bytes")).toDouble() / (1024.0 * 1024.0);
        const double budgetMb = bridge->snapshot()
                                    .value(QStringLiteral("cache_budget_bytes"))
                                    .toDouble()
            / (1024.0 * 1024.0);
        std::fprintf(stdout,
                     "dj-ui: the track is in memory: %.1f MB of the %.0f MB budget (%.1f%%)\n",
                     cachedMb, budgetMb, budgetMb > 0.0 ? 100.0 * cachedMb / budgetMb : 0.0);
        const qint64 beforeReverse = playHead();
        bool accepted = false;
        QElapsedTimer reverseWait;
        reverseWait.start();
        while (reverseWait.elapsed() < 12'000) {
            if (!press(QStringLiteral("reverse_A"))) {
                return 1;
            }
            if (waitFor(
                    [&]() { return deckA().value(QStringLiteral("reverse")).toBool(); }, 800)) {
                accepted = true;
                break;
            }
            // Refused: the track is not in the cache yet. Wait a moment and ask again.
            waitFor([]() { return false; }, 250);
        }
        if (!accepted) {
            std::fprintf(stderr,
                         "dj-ui: reverse was never accepted (the track never reached the cache)\n");
            return 1;
        }
        if (!waitFor([&]() { return playHead() < beforeReverse; }, 4000)) {
            std::fprintf(stderr,
                         "dj-ui: reverse was on but the play head did not walk down (%lld from "
                         "%lld)\n",
                         playHead(), beforeReverse);
            return 1;
        }
        const qint64 downhill = playHead();
        if (!press(QStringLiteral("reverse_A"))) {
            return 1;
        }
        if (!waitFor(
                [&]() { return !deckA().value(QStringLiteral("reverse")).toBool(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: reverse stayed on after the second press\n");
            return 1;
        }
        if (!waitFor([&]() { return playHead() > downhill; }, 4000)) {
            std::fprintf(stderr,
                         "dj-ui: the deck did not play forward again after reverse was switched off "
                         "(%lld from %lld)\n",
                         playHead(), downhill);
            return 1;
        }
        // Hand the deck back quiet. The next check loads its own file, and a deck that is still
        // playing refuses a load on purpose — leaving it running would make this check's leftovers
        // look like a broken load in someone else's check.
        if (deckA().value(QStringLiteral("playing")).toBool()
            && !press(QStringLiteral("play_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return !deckA().value(QStringLiteral("playing")).toBool(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: the deck would not stop for the next check\n");
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: a jump of one beat moved the play head %lld frames and landed on the "
                     "grid, the nudge bent the deck and let go, slip drew its virtual head on both "
                     "strips (%.4f against the sound at %.4f), and reverse walked the head down to "
                     "%lld and forward again, and the deck was left paused for the next check\n",
                     jump, slipMark, heardMark, downhill);
        return 0;
    }

    if (parser.isSet(gridHereOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-here-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = deckA()
                                              .value(QStringLiteral("track"))
                                              .toMap()
                                              .value(QStringLiteral("grid"))
                                              .toMap()
                                              .value(QStringLiteral("segments"))
                                              .toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto playHead = [&]() -> qint64 {
            return deckA().value(QStringLiteral("source_frame")).toLongLong();
        };
        const auto click = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName) && firstBeat() >= 0;
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to put on the head\n");
            return 1;
        }
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        const double bpm = deckA().value(QStringLiteral("local_bpm")).toDouble();
        if (rate <= 0 || !(bpm > 0.0)) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or tempo\n");
            return 1;
        }
        const double period = rate * 60.0 / bpm;
        const qint64 before = firstBeat();
        // Half a beat and a few frames past it: as far from a beat of the grid as the middle of the beat
        // is, and not on a round number of frames either.
        const qint64 target = before + static_cast<qint64>(period / 2.0) + 137;
        // The track has to reach the frame the head is sent to: what is left of it is the deck's own word.
        const qint64 left = deckA().value(QStringLiteral("remaining_frames")).toLongLong();
        if (left > 0 && target > playHead() + left - 1) {
            std::fprintf(stderr, "dj-ui: the fixture is too short for the check\n");
            return 1;
        }
        bridge->seek(QStringLiteral("A"), target);
        if (!waitFor([&]() { return std::llabs(playHead() - target) <= 2; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the play head did not reach frame %lld (%lld)\n", target,
                         playHead());
            return 1;
        }
        // The head has to be off the grid to begin with, or the check would prove nothing.
        const auto offTheGrid = [&]() {
            const double place = static_cast<double>(playHead() - firstBeat()) / period;
            return std::fabs(place - std::round(place)) * period > 2.0;
        };
        if (!offTheGrid()) {
            std::fprintf(stderr, "dj-ui: the play head started on a beat; nothing to prove\n");
            return 1;
        }
        // The mark on the button has to be a mark: the glyph for a target is drawn as a dot in most fonts,
        // and a dot is a button nobody presses. The ring is the mark, and it is measured, not assumed.
        auto *mark = findItem(window->contentItem(), QStringLiteral("gridHereRing_A"));
        if (mark == nullptr) {
            std::fprintf(stderr, "dj-ui: the button between the arrows has no mark of its own\n");
            return 1;
        }
        if (mark->width() < 8.0 || mark->height() < 8.0) {
            std::fprintf(stderr, "dj-ui: the mark on the button is %.1f by %.1f pixels\n",
                         mark->width(), mark->height());
            return 1;
        }
        if (!click(QStringLiteral("gridHere_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() != before && !offTheGrid(); }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: the grid did not come to the play head (first beat %lld -> %lld, head "
                         "%lld, period %.1f)\n",
                         before, firstBeat(), playHead(), period);
            return 1;
        }
        // Where a beat sits against the head, in frames: the button promises the beat *here*.
        const double landed = static_cast<double>(playHead() - firstBeat()) / period;
        const double error = std::fabs(landed - std::round(landed)) * period;
        if (error > 2.0) {
            std::fprintf(stderr, "dj-ui: a beat landed %.1f frames from the play head\n", error);
            return 1;
        }
        const qint64 placed = firstBeat();
        // The edit is the file's, not the session's: a reload has to bring the same grid back.
        bridge->load(QStringLiteral("A"), fixture);
        if (!waitFor([&]() { return std::llabs(firstBeat() - placed) <= 2; }, 20000)) {
            std::fprintf(stderr, "dj-ui: the reload moved the grid (%lld -> %lld)\n", placed,
                         firstBeat());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the grid came to the play head: a beat landed %.1f frames from frame %lld, "
                     "the mark on the button is %.0f by %.0f pixels, and the file kept the grid across a "
                     "reload\n",
                     error, target, mark->width(), mark->height());
        return 0;
    }

    if (parser.isSet(tempoRecheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --tempo-recheck needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        bridge->load(QStringLiteral("A"), fixture);
        if (!waitFor(
                [&]() {
                    const QVariant tempo = deckA()
                                               .value(QStringLiteral("track"))
                                               .toMap()
                                               .value(QStringLiteral("bpm"));
                    return tempo.isValid() && !tempo.isNull();
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the engine measured no tempo to re-measure\n");
            return 1;
        }
        const QString sidecar =
            QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
            QFileInfo(fixture).fileName() + QStringLiteral(".json");
        const auto revision = [&]() {
            QFile file(sidecar);
            if (!file.open(QIODevice::ReadOnly)) {
                return -1;
            }
            const QJsonObject meta = QJsonDocument::fromJson(file.readAll()).object();
            return meta.value(QStringLiteral("revision")).toInt(-1);
        };
        // The load itself writes the measured grid, on the engine's writer thread: wait for that write
        // before capturing the revision, or the check would credit the button with the loader's work.
        if (!waitFor([&]() { return revision() > 0; }, 8000)) {
            std::fprintf(stderr, "dj-ui: the load wrote no grid to %s\n", qPrintable(sidecar));
            return 1;
        }
        const int before = revision();
        auto *button = findItem(window->contentItem(), QStringLiteral("tempo_A"));
        if (button == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no re-measure button\n");
            return 1;
        }
        auto *confirm = findItem(window->contentItem(), QStringLiteral("tempoGo_A"));
        // Nothing is asked of a person who has clicked nothing yet. The popup's content may not exist
        // until it is first opened, which is also a way of asking nothing.
        if (confirm != nullptr && confirm->isVisible()) {
            std::fprintf(stderr, "dj-ui: the re-measure asks nothing before it runs\n");
            return 1;
        }
        QMetaObject::invokeMethod(button, "clicked");
        QCoreApplication::processEvents();
        if (confirm == nullptr) {
            confirm = findItem(window->contentItem(), QStringLiteral("tempoGo_A"));
        }
        if (confirm == nullptr) {
            std::fprintf(stderr, "dj-ui: the re-measure opened no question at all\n");
            return 1;
        }
        if (!waitFor([&]() { return confirm->isVisible(); }, 2000)) {
            std::fprintf(stderr, "dj-ui: the re-measure button did not ask for confirmation\n");
            return 1;
        }
        // The question alone must change nothing: what is written follows the answer, not the click.
        waitFor([]() { return false; }, 300);
        if (revision() != before) {
            std::fprintf(stderr, "dj-ui: the grid was rewritten before the question was answered\n");
            return 1;
        }
        auto *strip = findItem(window->contentItem(), QStringLiteral("bpm_A"));
        // The question must fit in its window: a sentence that runs off the edge is a sentence nobody
        // finishes reading. Wrapping is what makes the window narrow instead of as wide as the text.
        auto *question = findItem(window->contentItem(), QStringLiteral("tempoText_A"));
        if (question == nullptr) {
            std::fprintf(stderr, "dj-ui: the question has no text to read\n");
            return 1;
        }
        const double questionWidth = question->width();
        const double unwrapped = question->property("implicitWidth").toDouble();
        const double questionHeight = question->height();
        if (questionWidth > 300.0 || unwrapped <= questionWidth || questionHeight < 24.0) {
            std::fprintf(
                stderr,
                "dj-ui: the question does not wrap (%.0f px wide, %.0f px unwrapped, %.0f px tall)\n",
                questionWidth, unwrapped, questionHeight);
            return 1;
        }
        auto *badge = findItem(window->contentItem(), QStringLiteral("analyzing_A"));
        const QString shownBefore = strip != nullptr ? strip->property("text").toString() : QString();
        QMetaObject::invokeMethod(confirm, "clicked");
        // The screen has to say that the work is happening: the engine publishes the flag, the panel
        // draws a small window, and the check reads the panel rather than the snapshot behind it.
        const bool announced = waitFor(
            [&]() {
                return badge != nullptr && badge->isVisible() &&
                       deckA().value(QStringLiteral("analyzing")).toBool();
            },
            5000);
        if (!announced) {
            std::fprintf(stderr, "dj-ui: the re-measure ran without saying so\n");
            return 1;
        }
        if (!waitFor([&]() { return revision() > before; }, 20000)) {
            std::fprintf(stderr, "dj-ui: the confirmed re-measure never reached the metadata\n");
            return 1;
        }
        // ... and the number on the strip is the new one: the file and the screen must not disagree.
        // The publisher assembles the description at display rate, so the new number arrives a frame or
        // two after the file does — waiting for it is the difference between a slow screen and a stale one.
        waitFor(
            [&]() {
                return strip != nullptr && strip->property("text").toString() != shownBefore;
            },
            5000);
        const QVariant tempo =
            deckA().value(QStringLiteral("track")).toMap().value(QStringLiteral("bpm"));
        const QString shownAfter = strip != nullptr ? strip->property("text").toString() : QString();
        if (shownAfter.isEmpty() || shownAfter == shownBefore) {
            std::fprintf(stderr,
                         "dj-ui: the strip still shows \"%s\" after the measurement of %.2f BPM\n",
                         qPrintable(shownAfter), tempo.toDouble());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the re-measure asked, was announced \"%s\", confirmed, and reached the "
                     "strip: \"%s\" -> \"%s\", revision %d -> %d\n",
                     deckA().value(QStringLiteral("analyzing")).toBool() ? "yes" : "seen",
                     qPrintable(shownBefore), qPrintable(shownAfter), before, revision());
        return 0;
    }

    if (parser.isSet(gridReconnectOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        bridge->connectToEngine(parser.value(connectOption));
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        // No load here: this client must find the deck as the previous one left it. That is the whole
        // point — a load is what used to tell the engine its tempo base, and a reconnected client never
        // loads anything.
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto segments = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap()
                .value(QStringLiteral("segments"))
                .toList();
        };
        if (!waitFor([&]() { return !segments().isEmpty(); }, 10000)) {
            std::fprintf(stderr, "dj-ui: the deck holds no grid; load one before this check\n");
            return 1;
        }
        const QVariantMap first = segments().first().toMap();
        const double rate = deckA().value(QStringLiteral("source_rate")).toDouble();
        const double bpm = first.value(QStringLiteral("bpm")).toDouble();
        if (rate <= 0.0 || bpm <= 0.0) {
            std::fprintf(stderr, "dj-ui: the deck published no rate or no tempo\n");
            return 1;
        }
        const qint64 start = std::llround(first.value(QStringLiteral("frame")).toDouble());
        const qint64 beat = std::llround(rate * 60.0 / bpm);
        auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        auto *right = findItem(window->contentItem(), QStringLiteral("gridBeatRight_A"));
        auto *left = findItem(window->contentItem(), QStringLiteral("gridBeatLeft_A"));
        if (strip == nullptr || right == nullptr || left == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no grid buttons to press\n");
            return 1;
        }
        const auto firstFrame = [&]() {
            const QVariantList list = segments();
            return list.isEmpty() ? -1.0 : list.first().toMap().value(QStringLiteral("frame")).toDouble();
        };
        QMetaObject::invokeMethod(right, "clicked");
        if (!waitFor([&]() { return std::llround(firstFrame()) == start + beat; }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: on a reconnected client the grid stayed at %.0f instead of moving to "
                         "%lld\n",
                         firstFrame(), static_cast<long long>(start + beat));
            return 1;
        }
        QMetaObject::invokeMethod(left, "clicked");
        if (!waitFor([&]() { return std::llround(firstFrame()) == start; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the grid did not come back on a reconnected client (%.0f)\n",
                         firstFrame());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the grid moves on a reconnected client: %lld -> %lld and back at %.2f "
                     "BPM\n",
                     static_cast<long long>(start), static_cast<long long>(start + beat), bpm);
        return 0;
    }

    if (parser.isSet(tapOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --tap-check needs --load\n");
            return 1;
        }
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        bridge->connectToEngine(parser.value(connectOption));
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto grid = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap();
        };
        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        const auto loadArrived = [&]() {
            const QString held = deckA().value(QStringLiteral("path")).toString();
            return !held.isEmpty() && held.endsWith(wantName);
        };
        if (!waitFor(
                [&]() {
                    return loadArrived() && !grid().value(QStringLiteral("segments")).toList().isEmpty();
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to tap onto\n");
            return 1;
        }
        auto *panel = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
        if (panel == nullptr) {
            std::fprintf(stderr, "dj-ui: there is no deck panel to tap on\n");
            return 1;
        }
        // Four taps on a 120 BPM line that starts 0.32 s into the file, with the deck's position
        // jittering by a snapshot the way a real hand does: the grid has to come out on the line
        // through the taps, not on the play head and not on their median gap alone.
        constexpr double kRate = 48000.0;
        constexpr double kPeriod = 24000.0;
        // A quarter of a beat after the first frame: a grid put on the play head instead of on the taps
        // is a quarter of a beat away, which the check has to notice.
        constexpr double kFirst = 12000.0;
        // The play head is moved away from the taps' own phase first: a grid put on the play head
        // instead of on the taps then lands a third of a beat off, which the check has to notice.
        bool moved = false;
        for (int attempt = 0; attempt < 3 && !moved; ++attempt) {
            bridge->seek(QStringLiteral("A"), 100'000);
            moved = waitFor(
                [&]() { return deckA().value(QStringLiteral("source_frame")).toDouble() >= 99'999.0; },
                3000);
        }
        if (!moved) {
            std::fprintf(stderr, "dj-ui: the deck never moved to where the taps are not\n");
            return 1;
        }
        QVariantList taps;
        for (int index = 0; index < 4; ++index) {
            QVariantMap tap;
            tap.insert(QStringLiteral("ms"), 10'000 + 500 * index);
            const double jitter = index % 2 == 0 ? 360.0 : -360.0;
            tap.insert(QStringLiteral("frame"), kFirst + kPeriod * index + jitter);
            taps.append(tap);
        }
        QVariant accepted;
        QMetaObject::invokeMethod(panel, "applyTaps", Q_RETURN_ARG(QVariant, accepted),
                                  Q_ARG(QVariant, taps));
        if (!accepted.toBool()) {
            std::fprintf(stderr, "dj-ui: the taps made no grid\n");
            return 1;
        }
        const auto firstBeat = [&]() {
            const QVariantList segments = grid().value(QStringLiteral("segments")).toList();
            if (segments.isEmpty()) {
                return -1.0;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toDouble();
        };
        const auto tempo = [&]() {
            const QVariantList segments = grid().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? 0.0
                       : segments.first().toMap().value(QStringLiteral("bpm")).toDouble();
        };
        const double tempoBefore = tempo();
        const double firstBefore = std::max(0.0, firstBeat());
        // The old grid is a grid too: the wait is for the tapped one, which has a different tempo or a
        // different first beat, and for the taps to sit on it.
        const auto tapsOnBeats = [&]() {
            const double period = kRate * 60.0 / tempo();
            if (!(period > 0.0) || firstBeat() < 0.0) {
                return false;
            }
            for (int index = 0; index < taps.size(); ++index) {
                const double frame = taps.at(index).toMap().value(QStringLiteral("frame")).toDouble();
                const double beat = (frame - firstBeat()) / period;
                if (std::abs(beat - std::round(beat)) > 0.08) {
                    return false;
                }
            }
            return true;
        };
        if (!waitFor(
                [&]() {
                    const bool changed = std::abs(tempo() - tempoBefore) > 0.05 ||
                                         std::abs(firstBeat() - firstBefore) > 1.0;
                    return changed && std::abs(tempo() - 120.0) < 0.2 && tapsOnBeats();
                },
                8000)) {
            std::fprintf(stderr, "dj-ui: the tapped grid reads %.2f BPM with its first beat at %.0f\n",
                         tempo(), firstBeat());
            return 1;
        }
        // Every tap has to land on a beat: the grid passes through the hand, not near it.
        const double period = kRate * 60.0 / tempo();
        for (int index = 0; index < taps.size(); ++index) {
            const double frame = taps.at(index).toMap().value(QStringLiteral("frame")).toDouble();
            const double beat = (frame - firstBeat()) / period;
            const double off = std::abs(beat - std::round(beat));
            if (off > 0.08) {
                std::fprintf(stderr,
                             "dj-ui: tap %d at frame %.0f is %.2f of a beat off the grid (first beat "
                             "%.0f, %.2f BPM)\n",
                             index, frame, off, firstBeat(), tempo());
                return 1;
            }
        }
        std::fprintf(stdout,
                     "dj-ui: the taps made the grid: %.2f BPM, first beat %.0f on the line through "
                     "four taps\n",
                     tempo(), firstBeat());
        return 0;
    }

    if (parser.isSet(metronomeOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --metronome-check needs --load\n");
            return 1;
        }
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        bridge->connectToEngine(parser.value(connectOption));
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        const auto flag = [&]() {
            return deckA().value(QStringLiteral("metronome")).toBool();
        };
        const auto clicks = [&]() {
            return deckA().value(QStringLiteral("clicks")).toULongLong();
        };
        const auto lit = [&]() {
            auto *button = findItem(window->contentItem(), QStringLiteral("metronome_A"));
            return button != nullptr && button->property("highlighted").toBool();
        };
        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckA().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(wantName)
                           && !deckA()
                                   .value(QStringLiteral("track"))
                                   .toMap()
                                   .value(QStringLiteral("grid"))
                                   .toMap()
                                   .value(QStringLiteral("segments"))
                                   .toList()
                                   .isEmpty();
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to click against (path '%s')\n",
                         deckA().value(QStringLiteral("path")).toString().toUtf8().constData());
            return 1;
        }
        auto *button = findItem(window->contentItem(), QStringLiteral("metronome_A"));
        if (button == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no metronome button\n");
            return 1;
        }
        if (flag() || lit()) {
            std::fprintf(stderr, "dj-ui: the metronome is already on before anyone pressed it\n");
            return 1;
        }
        QMetaObject::invokeMethod(button, "clicked");
        if (!waitFor([&]() { return flag() && lit(); }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: the metronome button did not reach the engine (state %d, lit %d)\n",
                         int(flag()), int(lit()));
            return 1;
        }
        // A flag is not a click: the deck has to play, and the engine has to count the clicks it played on
        // the beats it crossed. The count is the one number a check can measure instead of hearing.
        bridge->play(QStringLiteral("A"), true);
        if (!waitFor([&]() { return clicks() >= 2; }, 8000)) {
            std::fprintf(stderr,
                         "dj-ui: the deck played but the metronome clicked %llu times\n",
                         static_cast<unsigned long long>(clicks()));
            return 1;
        }
        const unsigned long long played = clicks();
        bridge->play(QStringLiteral("A"), false);
        QMetaObject::invokeMethod(button, "clicked");
        if (!waitFor([&]() { return !flag() && !lit(); }, 5000)) {
            std::fprintf(stderr, "dj-ui: the metronome stayed on (state %d, lit %d)\n", int(flag()),
                         int(lit()));
            return 1;
        }
        const unsigned long long after = clicks();
        QThread::msleep(400);
        if (clicks() != after) {
            std::fprintf(stderr, "dj-ui: the metronome kept clicking after it was switched off\n");
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the metronome clicks the grid: the button lit, the deck played %llu clicks "
                     "on its beats, and switching it off stopped them\n",
                     played);
        return 0;
    }

    if (parser.isSet(gridLinesOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-lines-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        const auto loadArrived = [&]() {
            const QString held = deckA().value(QStringLiteral("path")).toString();
            return !held.isEmpty() && held.endsWith(wantName);
        };
        const auto gridOf = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap()
                .value(QStringLiteral("segments"))
                .toList();
        };
        if (!waitFor([&]() { return loadArrived() && !gridOf().isEmpty(); }, 20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid\n");
            return 1;
        }
        const QVariantMap segment = gridOf().first().toMap();
        const double bpm = segment.value(QStringLiteral("bpm")).toDouble();
        const double rate = deckA().value(QStringLiteral("source_rate")).toDouble();
        const double total = deckA().value(QStringLiteral("source_frame")).toDouble() +
                             deckA().value(QStringLiteral("remaining_frames")).toDouble();
        if (bpm <= 0.0 || rate <= 0.0 || total <= 0.0) {
            std::fprintf(stderr, "dj-ui: the grid has no usable numbers\n");
            return 1;
        }

        auto *view = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
        auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        if (view == nullptr || strip == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no waveform strip\n");
            return 1;
        }
        // The strip centres on the play head, and the play head is a fraction of the waveform it draws:
        // a deck with no envelope has no window at all, and every beat it counts is a counting of zero.
        if (!waitFor(
                [&]() {
                    return deckA()
                               .value(QStringLiteral("waveform"))
                               .toMap()
                               .value(QStringLiteral("buckets"))
                               .toInt() > 0;
                },
                20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no waveform to draw the grid on\n");
            return 1;
        }
        // Play from the middle and look at the whole track: a strip centred on the start shows half of
        // its window over the lead-in, and the beats there are not the track's.
        bridge->seek(QStringLiteral("A"), static_cast<qulonglong>(total / 2.0));
        // The strip is centred on the play head, and the seek reaches it a snapshot later: without this
        // wait the check reads the paint of the old window and calls a working strip wrong.
        if (!waitFor(
                [&]() {
                    return deckA().value(QStringLiteral("source_frame")).toDouble() >= total / 2.0 - 1.0;
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: the deck never moved to the middle of the track\n");
            return 1;
        }
        // The whole track, whatever the zoom the settings were left at: a strip that shows a quarter of
        // the track has fewer beats than the check counts, and a quarter of a picture is not a verdict.
        for (int press = 0; press < 12 && strip->property("span").toDouble() < 0.99; ++press) {
            QMetaObject::invokeMethod(view, "zoomBy", Q_ARG(QVariant, QVariant(0.1)));
            QCoreApplication::processEvents();
        }
        if (strip->property("span").toDouble() < 0.99) {
            std::fprintf(stderr, "dj-ui: the strip would not zoom out to the whole track (span %.3f)\n",
                         strip->property("span").toDouble());
            return 1;
        }
        // ...and centred on the middle of the track, so the whole window holds track and not lead-in. The
        // strip is centred on the play head, so this is also the proof that the seek arrived — and a
        // command that can be dropped while the deck settles is asked again instead of being believed
        // once.
        // The window has to be the whole track, not merely a window that starts at its beginning: a
        // strip showing the second half passes "starts after zero" and counts half the beats.
        const auto wholeTrack = [&]() {
            const double from = strip->property("windowStart").toDouble();
            const double span = strip->property("span").toDouble();
            return from <= 0.02 && from + span >= 0.98;
        };
        bool centred = wholeTrack();
        for (int attempt = 0; attempt < 3 && !centred; ++attempt) {
            bridge->seek(QStringLiteral("A"), static_cast<qulonglong>(total / 2.0));
            centred = waitFor(wholeTrack, 3000);
        }
        if (!centred) {
            const QVariantMap wave = deckA().value(QStringLiteral("waveform")).toMap();
            std::fprintf(stderr,
                         "dj-ui: the strip did not follow the play head to the middle (from %.3f, head "
                         "%.3f, span %.3f, frame %.0f, buckets %d, per bucket %.1f)\n",
                         strip->property("windowStart").toDouble(),
                         strip->property("playheadRatio").toDouble(),
                         strip->property("span").toDouble(),
                         deckA().value(QStringLiteral("source_frame")).toDouble(),
                         wave.value(QStringLiteral("buckets")).toInt(),
                         wave.value(QStringLiteral("frames_per_bucket")).toDouble());
            return 1;
        }

        const auto drawnBeats = [&]() { return strip->property("drawnBeats").toInt(); };
        const auto drawnBars = [&]() { return strip->property("drawnBars").toInt(); };
        const auto spacing = [&]() { return strip->property("beatSpacing").toDouble(); };
        // The numbers arrive with a paint, and a paint of an older window is a window that no longer
        // exists: the lines counted have to be the lines that tile the strip.
        const auto lines = [&]() {
            return drawnBeats() + drawnBars() + strip->property("drawnSquares").toInt();
        };
        if (!waitFor(
                [&]() {
                    return lines() >= 4 && spacing() > 0.0 && lines() * spacing() >= 0.7 * strip->width();
                },
                8000)) {
            std::fprintf(stderr, "dj-ui: the strip drew no beat lines (beats %d, spacing %.1f)\n",
                         drawnBeats(), spacing());
            return 1;
        }
        // The spacing is the tempo, on screen: one period of the grid, in pixels. Read here, after the
        // paint, because the strip's span and width are what that paint actually used.
        const double span = strip->property("span").toDouble();
        const double width = strip->width();
        const double expected = ((rate * 60.0 / bpm) / total) / span * width;
        if (std::abs(spacing() - expected) > 2.0) {
            std::fprintf(stderr,
                         "dj-ui: beat lines are %.1f px apart, but the tempo puts them %.1f px apart "
                         "(span %.4f, width %.1f)\n",
                         spacing(), expected, span, width);
            return 1;
        }
        const int beats = drawnBeats();
        const int bars = drawnBars();

        if (beats < 6 || bars < 2) {
            std::fprintf(stderr,
                         "dj-ui: the strip drew %d beats and %d bars over the track (window %.3f + "
                         "%.3f, play head %.3f, %d px wide)\n",
                         beats, bars, strip->property("windowStart").toDouble(),
                         span, strip->property("playheadRatio").toDouble(), int(strip->width()));
            return 1;
        }
        // A single tempo has no anchors; a strip that invents one is lying about the track.
        if (strip->property("drawnAnchors").toInt() != 0) {
            std::fprintf(stderr, "dj-ui: anchors appeared on a track with one tempo\n");
            return 1;
        }

        // A shift by one bar leaves every bar line where it was — the lines are periodic — so the strip
        // has to show the start of the four-bar square and the count, or a working shift looks like a
        // dead button.
        const auto squares = [&]() { return strip->property("drawnSquares").toInt(); };
        const auto firstSquare = [&]() { return strip->property("firstSquare").toDouble(); };
        const auto barLabel = [&]() {
            auto *label = findItem(window->contentItem(), QStringLiteral("bar_A"));
            return label != nullptr ? label->property("text").toString() : QString();
        };
        if (!waitFor([&]() { return squares() >= 1 && firstSquare() >= 0.0; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the strip marked no square of four bars (squares %d)\n",
                         squares());
            return 1;
        }
        const double periodRatio = (rate * 60.0 / bpm) / total;
        // The count is read at the play head, so the check puts the play head where it meant to have it.
        // Two reasons, both measured. A playing deck walks into the next bar while the check waits
        // seconds for a shift. And a deck stopped the way a CDJ stops comes back to its cue point, which
        // may be before the grid altogether — a count read there belongs to a bar that is not in the
        // track. One beat past the middle: the strip centres on the play head, and the middle keeps the
        // window on the whole track, while a beat past it keeps the count off a bar boundary.
        bridge->play(QStringLiteral("A"), false);
        const qint64 beatFrames = std::llround(rate * 60.0 / bpm);
        const auto parked = static_cast<qulonglong>(std::llround(total / 2.0) + beatFrames);
        const auto frameNow = [&]() {
            return deckA().value(QStringLiteral("source_frame")).toDouble();
        };
        bool standing = false;
        for (int attempt = 0; attempt < 4 && !standing; ++attempt) {
            bridge->seek(QStringLiteral("A"), parked);
            QElapsedTimer settle;
            settle.start();
            while (settle.elapsed() < 1500) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (std::abs(frameNow() - static_cast<double>(parked)) < 200.0) {
                    standing = true;
                    break;
                }
                QThread::msleep(20);
            }
        }
        if (!standing) {
            std::fprintf(stderr,
                         "dj-ui: the deck would not stand on beat 9,5 (frame %.0f, wanted %llu)\n",
                         frameNow(), static_cast<unsigned long long>(parked));
            return 1;
        }
        const double squareBefore = firstSquare();
        const QString labelBefore = barLabel();
        if (labelBefore.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the deck does not say which bar of the square it is in\n");
            return 1;
        }
        auto *barRight = findItem(window->contentItem(), QStringLiteral("gridBeatRight_A"));
        auto *barLeft = findItem(window->contentItem(), QStringLiteral("gridBeatLeft_A"));
        if (barRight == nullptr || barLeft == nullptr) {
            std::fprintf(stderr, "dj-ui: there is no bar button to press\n");
            return 1;
        }
        QMetaObject::invokeMethod(barRight, "clicked");
        if (!waitFor([&]() { return std::abs(firstSquare() - (squareBefore + periodRatio)) < 0.002; },
                     5000)) {
            std::fprintf(stderr,
                         "dj-ui: a beat shift moved the square start to %.4f, not to %.4f\n",
                         firstSquare(), squareBefore + periodRatio);
            return 1;
        }
        QMetaObject::invokeMethod(barLeft, "clicked");
        if (!waitFor(
                [&]() {
                    return std::abs(firstSquare() - squareBefore) < 0.002 &&
                           barLabel() == labelBefore &&
                           std::abs(frameNow() - static_cast<double>(parked)) < 200.0;
                },
                5000)) {
            std::fprintf(stderr,
                         "dj-ui: the square did not come back (start %.4f -> %.4f, count %s -> %s, "
                         "beat %.3f, play head %.0f, drawn %d beats %d bars %d squares, window %.3f "
                         "+ %.3f, anchor %.0f, metre %d)\n",
                         squareBefore, firstSquare(), qPrintable(labelBefore),
                         qPrintable(barLabel()),
                         deckA().value(QStringLiteral("beat")).toDouble(), frameNow(), drawnBeats(),
                         drawnBars(), strip->property("drawnSquares").toInt(),
                         strip->property("windowStart").toDouble(),
                         strip->property("span").toDouble(),
                         gridOf().isEmpty()
                             ? -1.0
                             : gridOf().first().toMap().value(QStringLiteral("frame")).toDouble(),
                         deckA()
                             .value(QStringLiteral("track"))
                             .toMap()
                             .value(QStringLiteral("grid"))
                             .toMap()
                             .value(QStringLiteral("beats_per_bar"))
                             .toInt());
            return 1;
        }
        // The count follows the grid too: four beats are a bar, so four presses of the beat button must
        // move the count by exactly one bar, and four presses back must return it.
        // One click per block: four clicks inside one block are one shift, not four, and four beats have
        // to be four shifts.
        for (int press = 0; press < 4; ++press) {
            QMetaObject::invokeMethod(barRight, "clicked");
            QCoreApplication::processEvents();
            QThread::msleep(250);
        }
        if (!waitFor([&]() { return barLabel() != labelBefore; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the bar count still says %s after four beats\n",
                         qPrintable(labelBefore));
            return 1;
        }
        const QString labelShifted = barLabel();
        for (int press = 0; press < 4; ++press) {
            QMetaObject::invokeMethod(barLeft, "clicked");
            QCoreApplication::processEvents();
            QThread::msleep(250);
        }
        if (!waitFor([&]() { return barLabel() == labelBefore; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the bar count stuck at %s after coming back\n",
                         qPrintable(barLabel()));
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the strip marks the square: start %.4f -> %.4f on a beat shift, count %s -> "
                     "%s on a bar\n",
                     squareBefore, squareBefore + periodRatio, qPrintable(labelBefore),
                     qPrintable(labelShifted));

        // The shift has no wall at the start of the file. The fixture's first beat is half a second in,
        // so one press of "one beat earlier" already walks the anchor past zero; six of them must leave
        // the picture intact, with only the lines moved.
        const int linesBefore = beats + bars + strip->property("drawnSquares").toInt();
        const double spacingBefore = spacing();
        const double beatBefore = deckA().value(QStringLiteral("beat")).toDouble();
        for (int press = 0; press < 6; ++press) {
            QMetaObject::invokeMethod(barLeft, "clicked");
            QCoreApplication::processEvents();
            QThread::msleep(250);
        }
        if (!waitFor(
                [&]() {
                    return std::abs(deckA().value(QStringLiteral("beat")).toDouble() - beatBefore - 6.0) <
                           0.2;
                },
                5000)) {
            std::fprintf(stderr,
                         "dj-ui: six presses of one beat moved the play head's beat from %.2f to %.2f, "
                         "not by six\n",
                         beatBefore, deckA().value(QStringLiteral("beat")).toDouble());
            return 1;
        }
        const auto firstFrame = [&]() {
            return gridOf().isEmpty() ? -1.0 : gridOf().first().toMap().value(QStringLiteral("frame")).toDouble();
        };
        if (firstFrame() < 0.0) {
            std::fprintf(stderr, "dj-ui: the anchor reads %.0f after walking past the start\n",
                         firstFrame());
            return 1;
        }
        if (std::abs(spacing() - spacingBefore) > 0.5) {
            std::fprintf(stderr, "dj-ui: the beat spacing changed to %.1f px on an unbounded shift\n",
                         spacing());
            return 1;
        }
        const int linesAfter = drawnBeats() + drawnBars() + strip->property("drawnSquares").toInt();
        if (std::abs(linesAfter - linesBefore) > 1) {
            std::fprintf(stderr, "dj-ui: the strip drew %d lines before the walk and %d after\n",
                         linesBefore, linesAfter);
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: six presses of one beat walked the anchor past the start and kept %d lines\n",
                     linesAfter);
        for (int press = 0; press < 6; ++press) {
            QMetaObject::invokeMethod(barRight, "clicked");
            QCoreApplication::processEvents();
            QThread::msleep(150);
        }

        // Half the tempo and the lines move with the grid: this is the proof that they are the grid and
        // not a decoration of the waveform.
        auto *half = findItem(window->contentItem(), QStringLiteral("gridHalf_A"));
        if (half == nullptr) {
            std::fprintf(stderr, "dj-ui: there is no half button to press\n");
            return 1;
        }
        QMetaObject::invokeMethod(half, "clicked");
        if (!waitFor([&]() { return std::abs(spacing() - 2.0 * expected) < 4.0; }, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: after half the beat lines are %.1f px apart, not %.1f\n", spacing(),
                         2.0 * expected);
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: the strip draws the grid: %d beats, %d bars, %.1f px a beat at %.2f BPM, "
                     "%.1f px after half\n",
                     beats, bars, expected, bpm, spacing());
        return 0;
    }

    if (parser.isSet(gridCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-check needs --load\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckA = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        // The grid as the deck publishes it: the same numbers the beat lines are drawn from.
        const auto gridOf = [&]() {
            return deckA()
                .value(QStringLiteral("track"))
                .toMap()
                .value(QStringLiteral("grid"))
                .toMap()
                .value(QStringLiteral("segments"))
                .toList();
        };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = gridOf();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto tempoNow = [&]() {
            return deckA().value(QStringLiteral("local_bpm")).toDouble();
        };
        const auto click = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), fixture);
        // The deck keeps the previous track until the new one is ready, grid and all: waiting for "a
        // grid" means editing the file the previous run left behind.
        const QString wantName = QFileInfo(fixture).fileName();
        const auto loadArrived = [&]() {
            const QString held = deckA().value(QStringLiteral("path")).toString();
            return !held.isEmpty() && held.endsWith(wantName);
        };
        if (!waitFor([&]() { return loadArrived() && firstBeat() >= 0 && tempoNow() > 0.0; }, 20000)) {
            std::fprintf(stderr, "dj-ui: the deck published no grid to edit\n");
            return 1;
        }
        const qint64 start = firstBeat();
        const double tempo = tempoNow();
        // A step of ten milliseconds is the rate over a hundred, whatever the file's own rate is.
        const qint64 rate = deckA().value(QStringLiteral("source_rate")).toLongLong();
        if (rate <= 0) {
            std::fprintf(stderr, "dj-ui: the deck published no source rate\n");
            return 1;
        }

        const qint64 step = std::clamp<qint64>(rate / 100, 1, 100000);
        if (!click(QStringLiteral("gridRight_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start + step; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the shift did not move the grid by ten milliseconds (%lld -> %lld)\n",
                         static_cast<long long>(start), static_cast<long long>(firstBeat()));
            return 1;
        }
        if (!click(QStringLiteral("gridLeft_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the grid did not come back\n");
            return 1;
        }
        // A beat a press: one period of the tempo under the play head, and back again. A grid that starts
        // on the wrong beat needs a beat, not the ten milliseconds of the arrows.
        const qint64 beat = std::llround(double(rate) * 60.0 / tempo);
        if (!click(QStringLiteral("gridBeatRight_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start + beat; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the beat shift moved the grid to %lld instead of %lld\n",
                         static_cast<long long>(firstBeat()),
                         static_cast<long long>(start + beat));
            return 1;
        }
        if (!click(QStringLiteral("gridBeatLeft_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return firstBeat() == start; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the beat shift back did not return the grid\n");
            return 1;
        }

        // Half, then double: the same grid read twice as slow and then back.
        if (!click(QStringLiteral("gridHalf_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return std::abs(tempoNow() - tempo / 2.0) < 0.05; }, 5000)) {
            std::fprintf(stderr, "dj-ui: half did not halve the tempo (%.2f)\n", tempoNow());
            return 1;
        }
        if (!click(QStringLiteral("gridDouble_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return std::abs(tempoNow() - tempo) < 0.05; }, 5000)) {
            std::fprintf(stderr, "dj-ui: double did not bring the tempo back (%.2f)\n", tempoNow());
            return 1;
        }

        // A tempo typed by hand: the number the DJ knows. The question is built when it is first
        // opened, so the field only exists after the button has been pressed.
        if (!click(QStringLiteral("gridTempo_A"))) {
            return 1;
        }
        auto *field = findItem(window->contentItem(), QStringLiteral("tempoField_A"));
        if (field == nullptr) {
            std::fprintf(stderr, "dj-ui: the tempo field was never created\n");
            return 1;
        }
        field->setProperty("text", QStringLiteral("128.0"));
        if (!click(QStringLiteral("tempoApply_A"))) {
            return 1;
        }
        if (!waitFor([&]() { return std::abs(tempoNow() - 128.0) < 0.05; }, 5000)) {
            std::fprintf(stderr, "dj-ui: the typed tempo did not reach the deck (%.2f)\n", tempoNow());
            return 1;
        }

        // Every edit is a decision about the file: it is written beside the music and locked against
        // the next automatic measurement.
        const QString sidecar =
            QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
            QFileInfo(fixture).fileName() + QStringLiteral(".json");
        const auto stored = [&]() {
            QFile file(sidecar);
            if (!file.open(QIODevice::ReadOnly)) {
                return QJsonObject();
            }
            return QJsonDocument::fromJson(file.readAll()).object();
        };
        if (!waitFor(
                [&]() {
                    const QJsonObject grid = stored().value(QStringLiteral("beatgrid")).toObject();
                    return std::abs(grid.value(QStringLiteral("bpm")).toDouble() - 128.0) < 0.05 &&
                           stored().value(QStringLiteral("analysis_lock")).toBool();
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: the edit never reached %s\n", qPrintable(sidecar));
            return 1;
        }
        const QJsonObject grid = stored().value(QStringLiteral("beatgrid")).toObject();
        std::fprintf(stdout,
                     "dj-ui: the grid moved a beat and back, halved, doubled and took a typed tempo: "
                     "%.2f -> 128.00 BPM, "
                     "first beat %lld, file revision %d, locked\n",
                     tempo, static_cast<long long>(grid.value(QStringLiteral("first_beat_frame"))
                                                       .toVariant()
                                                       .toLongLong()),
                     stored().value(QStringLiteral("revision")).toInt());
        return 0;
    }

    if (parser.isSet(eqCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();

        // The knobs have to exist in the loaded scene: a strip without them is a strip with no way
        // to reach the band, whatever the engine supports.
        // The window is sized by the session and is not shown in a headless run: measure against the
        // size a person actually sees, or every number below is about a 0x0 window.
        window->resize(1366, 768);
        window->show();
        QCoreApplication::processEvents(QEventLoop::AllEvents, 100);
        QEventLoop settleAgain;
        QTimer::singleShot(200, &settleAgain, &QEventLoop::quit);
        settleAgain.exec();

        QQuickItem *content = window->contentItem();
        const QString decks[2] = {QStringLiteral("A"), QStringLiteral("B")};
        const QString bands[3] = {QStringLiteral("low"), QStringLiteral("mid"),
                                  QStringLiteral("high")};
        QQuickItem *knobs[2][3] = {{nullptr, nullptr, nullptr}, {nullptr, nullptr, nullptr}};
        for (int deck = 0; deck < 2; ++deck) {
            for (int band = 0; band < 3; ++band) {
                const QString name = QStringLiteral("eq_%1_%2").arg(bands[band], decks[deck]);
                knobs[deck][band] = findItem(content, name);
                if (knobs[deck][band] == nullptr) {
                    std::fprintf(stderr, "dj-ui: no knob %s in the mixer\n", qPrintable(name));
                    return 1;
                }
            }
        }

        // The knobs sit above the meter, not on top of it: a strip has one column, and a knob drawn
        // over the level is a knob nobody can read. The strip's own height test is what keeps the
        // arrangement honest when someone adds another row of controls.
        auto *meterA = findItem(content, QStringLiteral("meter_A"));
        if (meterA == nullptr) {
            std::fprintf(stderr, "dj-ui: no meter for deck A\n");
            return 1;
        }
        // The bottom of the stack is the LO knob, not whichever band happened to be last in the
        // array: the knobs are drawn high-to-low, and comparing the top one let the label under the
        // bottom one overlap the meter while the check stayed green.
        double stackBottom = 0.0;
        for (int band = 0; band < 3; ++band) {
            const double bottom =
                knobs[0][band]->mapToItem(content, QPointF(0, knobs[0][band]->height())).y();
            stackBottom = std::max(stackBottom, bottom);
        }
        const double meterTop = meterA->mapToItem(content, QPointF(0, 0)).y();
        std::fprintf(stdout, "dj-ui: the EQ knobs end at %.0f px, meter A starts at %.0f px\n",
                     stackBottom, meterTop);
        if (stackBottom > meterTop) {
            std::fprintf(stderr, "dj-ui: the EQ knobs overlap the meter\n");
            return 1;
        }

        // Every channel has its own gain knob, and it sits on the master knob's line: the master is the
        // gain of the whole mix, and one line across the mixer is how a strip is read.
        auto *trimA = findItem(content, QStringLiteral("trim_A"));
        auto *trimB = findItem(content, QStringLiteral("trim_B"));
        if (trimA == nullptr || trimB == nullptr) {
            std::fprintf(stderr, "dj-ui: a channel has no gain knob\n");
            return 1;
        }

        // The owner asked for the same cap as the master knob: a band control is the same class of
        // control as the master gain, and a smaller cap reads as a lesser one. Measured, not assumed.
        auto *masterKnob = findItem(content, QStringLiteral("masterKnob"));
        if (masterKnob == nullptr) {
            std::fprintf(stderr, "dj-ui: no master knob in the mixer\n");
            return 1;
        }
        const double masterSide = masterKnob->width();
        for (int deck = 0; deck < 2; ++deck) {
            for (int band = 0; band < 3; ++band) {
                const double side = knobs[deck][band]->property("capSide").toDouble();
                if (std::abs(side - masterSide) > 1.0) {
                    std::fprintf(stderr,
                                 "dj-ui: the %s %s knob is %.0f px against the master's %.0f px\n",
                                 qPrintable(decks[deck]), qPrintable(bands[band]), side, masterSide);
                    return 1;
                }
            }
        }
        std::fprintf(stdout,
                     "dj-ui: every EQ knob is %.0f px across, the same cap as the master knob\n",
                     masterSide);
        // And the master is the same control, not a bigger one: its cap has to measure up to the
        // channel caps, in both directions.
        for (QQuickItem *trim : {trimA, trimB}) {
            const double side = trim->property("capSide").toDouble();
            if (std::abs(side - masterSide) > 1.0) {
                std::fprintf(stderr,
                             "dj-ui: a channel gain knob is %.0f px against the master's %.0f px\n",
                             side, masterSide);
                return 1;
            }
        }
        std::fprintf(stdout,
                     "dj-ui: the master knob is %.0f px, the channel gain knobs are %.0f px, the EQ "
                     "knobs %.0f px\n",
                     masterSide, trimA->property("capSide").toDouble(),
                     knobs[0][0]->property("capSide").toDouble());

        // Where the pointers stand at unity. The owner asked for two different zeroes and both are
        // facts about the drawing, not about the numbers: a trim points at twelve o'clock, a volume
        // knob sits at three.
        //
        // The engine may be anywhere: another check in the same gate leaves the master at half, and an
        // angle read from someone else's setting is not a measurement of the knob. Ask for unity first.
        bridge->setMasterGain(1.0);
        bridge->setChannelGain(QStringLiteral("A"), 1.0);
        // Both knobs, or the angle below measures whatever the previous check left behind: the gate runs
        // these checks one after another on one engine.
        const auto gainOfA = [&]() {
            const QVariantList published =
                bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : published) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck.value(QStringLiteral("linear_gain")).toDouble();
                }
            }
            return 0.0;
        };
        QElapsedTimer settling;
        settling.start();
        while (settling.elapsed() < 1500) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            const double master = bridge->snapshot().value(QStringLiteral("master_gain")).toDouble();
            if (std::abs(master - 1.0) < 0.01 && std::abs(gainOfA() - 1.0) < 0.01) {
                break;
            }
            QThread::msleep(20);
        }
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);

        const double trimAngle = trimA->property("angle").toDouble();
        const double masterAngle = masterKnob->property("angle").toDouble();
        std::fprintf(stdout,
                     "dj-ui: at unity the gain knob points %.0f deg and the master knob %.0f deg\n",
                     trimAngle, masterAngle);
        if (std::abs(trimAngle) > 1.0) {
            std::fprintf(stderr, "dj-ui: the gain knob's zero is not straight up\n");
            return 1;
        }
        if (std::abs(masterAngle - 90.0) > 1.0) {
            std::fprintf(stderr, "dj-ui: the master knob's unity is not to the right\n");
            return 1;
        }

        // The hand turns the pointer at one rate: ten pixels up and ten pixels down are ninety degrees
        // each way, whatever the decibels do — the two halves stay asymmetric, like a channel strip.
        const auto pump = [&]() {
            for (int step = 0; step < 40; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        const auto turnTwice = [&](QQuickItem *knob, const char *name, double &up, double &down,
                                   double &dbUp, double &dbDown) {
            if (knob == nullptr) {
                return false;
            }
            const QPointF grab =
                knob->mapToScene(QPointF(knob->width() / 2.0, knob->height() / 2.0));
            const auto send = [&](QEvent::Type type, const QPointF &at) {
                QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            };
            send(QEvent::MouseButtonPress, grab);
            pump();
            const double rest = knob->property("angle").toDouble();
            const double restDb = knob->property("db").toDouble();
            send(QEvent::MouseMove, grab - QPointF(0, 10));
            pump();
            up = knob->property("angle").toDouble() - rest;
            dbUp = knob->property("db").toDouble() - restDb;
            send(QEvent::MouseMove, grab + QPointF(0, 10));
            pump();
            down = knob->property("angle").toDouble() - rest;
            dbDown = knob->property("db").toDouble() - restDb;
            send(QEvent::MouseButtonRelease, grab);
            std::fprintf(stdout,
                         "dj-ui: ten pixels turn %s %+.0f and %+.0f degrees (%+.1f / %+.1f dB)\n",
                         name, up, down, dbUp, dbDown);
            return true;
        };
        double gainUp = 0.0;
        double gainDown = 0.0;
        double gainDbUp = 0.0;
        double gainDbDown = 0.0;
        if (!turnTwice(trimA, "the gain knob", gainUp, gainDown, gainDbUp, gainDbDown)) {
            std::fprintf(stderr, "dj-ui: no trim knob on deck A\n");
            return 1;
        }
        if (std::abs(gainUp - 90.0) > 2.0 || std::abs(gainDown + 90.0) > 2.0) {
            std::fprintf(stderr, "dj-ui: the gain knob turns at different rates up and down\n");
            return 1;
        }
        QQuickItem *eqLowA = findItem(window->contentItem(), QStringLiteral("eq_low_A"));
        double bandUp = 0.0;
        double bandDown = 0.0;
        double bandDbUp = 0.0;
        double bandDbDown = 0.0;
        if (turnTwice(eqLowA, "the low band", bandUp, bandDown, bandDbUp, bandDbDown)
            && (std::abs(bandUp - 90.0) > 2.0 || std::abs(bandDown + 90.0) > 2.0)) {
            std::fprintf(stderr, "dj-ui: the band knob turns at different rates up and down\n");
            return 1;
        }
        // The foot of the knob is a detent, and it is not another decibel: turn the hand far past the
        // end of the cut and the band has to arrive at the engine killed, not at the deepest cut. A
        // knob that only reached -26 would leave an isolator that nothing can reach.
        if (eqLowA != nullptr) {
            const auto eqOfA = [&](const QString &band) {
                const QVariantList published =
                    bridge->snapshot().value(QStringLiteral("decks")).toList();
                for (const QVariant &entry : published) {
                    const QVariantMap deck = entry.toMap();
                    if (deck.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                        continue;
                    }
                    const QVariantMap eq = deck.value(QStringLiteral("eq")).toMap();
                    return eq.value(band + QStringLiteral("_db")).toDouble();
                }
                return 0.0;
            };
            const auto dragTo = [&](double pixels) {
                const QPointF grab =
                    eqLowA->mapToScene(QPointF(eqLowA->width() / 2.0, eqLowA->height() / 2.0));
                const auto send = [&](QEvent::Type type, const QPointF &at) {
                    QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                      Qt::LeftButton, Qt::NoModifier);
                    QCoreApplication::sendEvent(window, &event);
                    QCoreApplication::processEvents();
                };
                // In steps, like a hand: one jump of two hundred pixels leaves the knob's own area and
                // the move is delivered somewhere else, which reads as a knob that never turned.
                send(QEvent::MouseButtonPress, grab);
                pump();
                const int steps = std::max(1, static_cast<int>(std::ceil(std::abs(pixels) / 10.0)));
                const double step = pixels / static_cast<double>(steps);
                for (int index = 1; index <= steps; ++index) {
                    send(QEvent::MouseMove, grab + QPointF(0, step * index));
                }
                send(QEvent::MouseButtonRelease, grab + QPointF(0, step * steps));
                pump();
            };
            // The round trip through the engine is what makes the kill real: wait for it rather than
            // reading a snapshot that has not caught up.
            const auto waitForEq = [&](double wanted) {
                QElapsedTimer waiting;
                waiting.start();
                while (waiting.elapsed() < 3000) {
                    QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    if (std::abs(eqOfA(QStringLiteral("low")) - wanted) < 0.5) {
                        return true;
                    }
                    QThread::msleep(20);
                }
                return false;
            };
            dragTo(200.0);
            (void)waitForEq(-120.0);
            const double asked = eqLowA->property("db").toDouble();
            const bool killed = eqLowA->property("killed").toBool();
            const double published = eqOfA(QStringLiteral("low"));
            std::fprintf(stdout,
                         "dj-ui: the low band's detent asks for %.1f dB, the knob calls it %s, the "
                         "engine published %.1f dB\n",
                         asked, killed ? "killed" : "not killed", published);
            if (!killed || asked > -119.0) {
                std::fprintf(stderr, "dj-ui: the end of the cut travel is not a detent\n");
                return 1;
            }
            if (published > -119.0) {
                std::fprintf(stderr,
                             "dj-ui: the detent sent %.1f dB, so the isolator is unreachable\n",
                             published);
                return 1;
            }
            // Back to the ordinary cut: a detent that swallows the end of the travel cannot be the only
            // thing the knob knows, and the deepest cut has to stay a cut.
            dragTo(-8.0);
            QElapsedTimer backing;
            backing.start();
            while (backing.elapsed() < 3000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (eqOfA(QStringLiteral("low")) > -119.0) {
                    break;
                }
                QThread::msleep(20);
            }
            const double cut = eqOfA(QStringLiteral("low"));
            std::fprintf(stdout, "dj-ui: one detent back the low band is at %.1f dB\n", cut);
            if (cut <= -119.0) {
                std::fprintf(stderr, "dj-ui: the knob cannot leave the detent\n");
                return 1;
            }
            if (cut < -26.5 || cut > -5.0) {
                std::fprintf(stderr,
                             "dj-ui: eight pixels back from the detent is %.1f dB, which is neither "
                             "the ordinary cut nor a way out of the kill\n",
                             cut);
                return 1;
            }
        }
        // The gesture above left deck A's low band where the hand dropped it; the checks below want a flat
        // channel, so put the knob and the deck back before measuring anything else.
        bridge->setChannelGain(QStringLiteral("A"), 1.0);
        bridge->setChannelEq(QStringLiteral("A"), QStringLiteral("low"), 0.0);
        for (int step = 0; step < 40; ++step) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
            QThread::msleep(5);
        }

        const double masterTop = masterKnob->mapToItem(content, QPointF(0, 0)).y();
        for (QQuickItem *trim : {trimA, trimB}) {
            const double trimTop = trim->mapToItem(content, QPointF(0, 0)).y();
            std::fprintf(stdout, "dj-ui: gain knob %s at %.0f px, master knob at %.0f px\n",
                         qPrintable(trim->property("deckId").toString()), trimTop, masterTop);
            if (std::abs(trimTop - masterTop) > 2.0) {
                std::fprintf(stderr, "dj-ui: a gain knob is not on the master knob's line\n");
                return 1;
            }
        }
        for (int band = 0; band < 3; ++band) {
            const QString letter = knobs[0][band]->property("letter").toString();
            std::fprintf(stdout, "dj-ui: the %s knob is labelled %s\n",
                         qPrintable(bands[band]), qPrintable(letter));
        }

        // What the engine says about one band, straight from the published snapshot.
        const auto bandOf = [&](int deck, int band) {
            const QVariantList published =
                bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : published) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != decks[deck]) {
                    continue;
                }
                const QVariantMap eq = map.value(QStringLiteral("eq")).toMap();
                return eq.value(bands[band] + QStringLiteral("_db")).toDouble();
            }
            return 0.0;
        };

        // Three different values on one deck: if the bands shared a queue slot or a filter, the
        // snapshot would come back with one number three times.
        const double wanted[3] = {6.0, -6.0, 3.0};
        for (int band = 0; band < 3; ++band) {
            bridge->setChannelEq(decks[1], bands[band], wanted[band]);
        }

        QElapsedTimer waiting;
        waiting.start();
        bool applied = false;
        while (waiting.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            applied = true;
            for (int band = 0; band < 3; ++band) {
                applied = applied && std::abs(bandOf(1, band) - wanted[band]) < 0.01;
            }
            if (applied) {
                break;
            }
            QThread::msleep(20);
        }
        // The knobs are painted from the snapshot: let the bindings run before reading them.
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);

        bool agreed = true;
        for (int deck = 0; deck < 2; ++deck) {
            for (int band = 0; band < 3; ++band) {
                const double engine = bandOf(deck, band);
                const double shown = knobs[deck][band]->property("db").toDouble();
                std::fprintf(stdout,
                             "dj-ui: knob %s %s shows %+.1f dB, the engine says %+.1f dB\n",
                             qPrintable(decks[deck]), qPrintable(bands[band]), shown, engine);
                if (std::abs(shown - engine) > 0.01) {
                    agreed = false;
                }
            }
        }
        if (!applied) {
            std::fprintf(stderr, "dj-ui: the EQ knobs did not reach the engine\n");
            return 1;
        }
        if (!agreed) {
            std::fprintf(stderr, "dj-ui: a knob shows a number the engine did not apply\n");
            return 1;
        }
        if (std::abs(bandOf(0, 0)) > 0.01 || std::abs(bandOf(0, 1)) > 0.01
            || std::abs(bandOf(0, 2)) > 0.01) {
            std::fprintf(stderr, "dj-ui: the knobs on B moved deck A\n");
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: eq B low %+.1f mid %+.1f high %+.1f applied, A still flat, the knobs "
                     "show the engine's numbers\n",
                     bandOf(1, 0), bandOf(1, 1), bandOf(1, 2));
        // Back to unity: the drag above moved deck A's gain, and the checks after this one should not
        // inherit a doubled channel.
        bridge->setChannelGain(QStringLiteral("A"), 1.0);
        return 0;
    }






    if (parser.isSet(loopOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        window->resize(1366, 768);
        window->show();
        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        pump(60);
        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --loop-check needs --load\n");
            return 1;
        }
        bridge->load(QStringLiteral("A"), path);
        pump(140);
        const auto deckOf = [&]() {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return map;
                }
            }
            return QVariantMap();
        };
        auto *loopButton = findItem(window->contentItem(), QStringLiteral("loop_A"));
        if (loopButton == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no loop button\n");
            return 1;
        }
        const auto click = [&](Qt::KeyboardModifiers modifiers) {
            const QPointF at =
                loopButton->mapToScene(QPointF(loopButton->width() / 2.0, loopButton->height() / 2.0));
            for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
                QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, modifiers);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            }
            pump(30);
        };
        const auto loopRegion = [&]() {
            const QVariant value = deckOf().value(QStringLiteral("loop_region"));
            return value.isNull() || !value.isValid() ? QVariantMap() : value.toMap();
        };

        // Loop mode is the armed set now: the saved loops plus the button's own, not one region.
        // QVariantList::size() is a qsizetype; the counters below are printed with %d.
        const auto armedCount = [&]() {
            return int(deckOf().value(QStringLiteral("active_loops")).toList().size());
        };
        auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        const auto stripProp = [&](const char *name) {
            return strip == nullptr ? 0.0 : strip->property(name).toDouble();
        };

        // A deck left with a loop by an earlier run would take the press as "enter the loop", so start
        // from a deck with nothing in force.
        bridge->setLoop(QStringLiteral("A"), false, 0, 0);
        pump(20);

        // Press one: a marked start. Nothing is in force yet, but the marker has to be visible — a
        // press that leaves no trace reads as a press that did nothing.
        bridge->play(QStringLiteral("A"), false);
        // The strip places markers by the deck's own frame count, so wait for the deck to know it.
        bool known = false;
        for (int step = 0; step < 60 && !known; ++step) {
            pump(2);
            known = deckOf().value(QStringLiteral("remaining_frames")).isValid();
        }
        if (!known) {
            std::fprintf(stderr, "dj-ui: the deck never reported the track's length\n");
            return 1;
        }
        bridge->seek(QStringLiteral("A"), 48000);
        pump(30);
        click(Qt::NoModifier);
        const double pending = stripProp("pendingFrom");
        std::fprintf(stdout, "dj-ui: the first press marks a start at %.4f of the track, loop %s\n",
                     pending, loopRegion().isEmpty() ? "not in force" : "already in force");
        if (pending < 0.0) {
            std::fprintf(stderr, "dj-ui: the first press left no marker on the strip\n");
            return 1;
        }
        if (!loopRegion().isEmpty()) {
            std::fprintf(stderr, "dj-ui: the first press activated a loop; it must only mark the start\n");
            return 1;
        }

        // Press two: the loop closes, switches on and carries the playhead to its beginning.
        bridge->seek(QStringLiteral("A"), 96000);
        bridge->play(QStringLiteral("A"), true);
        pump(40);
        // The loop ends where the playhead is at the press, so that is what the check compares with.
        const double pressed = deckOf().value(QStringLiteral("source_frame")).toDouble();
        click(Qt::NoModifier);
        const QVariantMap region = loopRegion();
        const double start = region.value(QStringLiteral("start")).toDouble();
        const double end = region.value(QStringLiteral("end")).toDouble();
        const double parked = deckOf().value(QStringLiteral("source_frame")).toDouble();
        const bool active = stripProp("loopActive") > 0.5;
        std::fprintf(stdout, "dj-ui: the second press closed %.0f…%.0f, active %s, armed %d, playhead at %.0f\n",
                     start, end, active ? "yes" : "no", armedCount(), parked);
        if (std::abs(start - 48000.0) > 2000.0) {
            std::fprintf(stderr, "dj-ui: the loop does not start where the first press was\n");
            return 1;
        }
        // A playing deck runs on between the read and the click, so the end is compared loosely: what
        // the check rules out is a loop that ends anywhere but at the press.
        if (std::abs(end - pressed) > 8000.0) {
            std::fprintf(stderr, "dj-ui: the loop does not end where the second press was (%.0f)\n", pressed);
            return 1;
        }
        if (!active) {
            std::fprintf(stderr, "dj-ui: the loop was drawn but not switched on\n");
            return 1;
        }
        // Tolerance covers a deck already playing: a quarter of a second of drift after the jump back.
        if (std::abs(parked - start) > 12000.0) {
            std::fprintf(stderr, "dj-ui: the playhead was not carried to the loop's beginning\n");
            return 1;
        }

        // Playing: the material must stay inside the loop instead of running past its end.
        const auto frames = [&]() { return deckOf().value(QStringLiteral("source_frame")).toDouble(); };
        double peak = frames();
        for (int step = 0; step < 12; ++step) {
            pump(6);
            peak = std::max(peak, frames());
        }
        std::fprintf(stdout, "dj-ui: after 12 steps of playing the playhead reached %.0f (loop ends %.0f)\n",
                     peak, end);
        if (peak > end + 4000.0) {
            std::fprintf(stderr, "dj-ui: the playhead ran past the end of the loop\n");
            return 1;
        }

        // Save it on a pad: the marks must be on the track without anyone pressing that pad, so a loop
        // saved last week is found today.
        bridge->setPadLoop(QStringLiteral("A"), 5);
        pump(30);
        const int padMarks = strip == nullptr ? 0 : strip->property("padLoops").toList().size();
        std::fprintf(stdout, "dj-ui: the strip paints %d stored loop(s)%s\n", padMarks,
                     padMarks > 0 ? "" : " — none");
        if (padMarks < 1) {
            std::fprintf(stderr, "dj-ui: a loop stored on a pad is not painted on the track\n");
            return 1;
        }

        // Shift is a mode switch, not a jump: stop the deck first so the frame can be compared exactly.
        bridge->play(QStringLiteral("A"), false);
        pump(30);
        const double beforeShift = frames();
        click(Qt::ShiftModifier);
        const double afterOff = frames();
        const int savedMarks = strip == nullptr ? 0 : strip->property("padLoops").toList().size();
        bool anyActiveMark = false;
        for (const QVariant &entry : strip == nullptr ? QVariantList()
                                                      : strip->property("padLoops").toList()) {
            if (entry.toMap().value(QStringLiteral("active")).toBool()) {
                anyActiveMark = true;
            }
        }
        const bool painted = savedMarks > 0;
        const bool stillActive = anyActiveMark;
        std::fprintf(stdout, "dj-ui: shift switched the loop %s, edges still painted %s, playhead %.0f -> %.0f\n",
                     loopRegion().isEmpty() ? "off" : "on", painted ? "yes" : "no", beforeShift, afterOff);
        if (armedCount() != 0) {
            std::fprintf(stderr, "dj-ui: shift did not switch the loop off\n");
            return 1;
        }
        if (!painted || stillActive) {
            std::fprintf(stderr, "dj-ui: a switched-off loop must keep its edges and lose its fill\n");
            return 1;
        }
        if (std::abs(afterOff - beforeShift) > 64.0) {
            std::fprintf(stderr, "dj-ui: shift moved the playhead; switching the mode must not jump\n");
            return 1;
        }

        // Shift again: back on, still without moving.
        click(Qt::ShiftModifier);
        const double afterOn = frames();
        std::fprintf(stdout, "dj-ui: the next shift brought the loop %s, playhead %.0f\n",
                     armedCount() > 0 ? "back" : "not back", afterOn);
        if (armedCount() == 0) {
            std::fprintf(stderr, "dj-ui: shift did not switch the loop back on\n");
            return 1;
        }
        if (std::abs(afterOn - beforeShift) > 64.0) {
            std::fprintf(stderr, "dj-ui: switching the loop back on jumped the playhead\n");
            return 1;
        }

        // A plain press with the playhead inside the loop switches the mode on where it stands.
        bridge->seek(QStringLiteral("A"), start + 2000.0);
        pump(20);
        click(Qt::ShiftModifier);
        pump(20);
        const double insideBefore = frames();
        click(Qt::NoModifier);
        const double insideAfter = frames();
        std::fprintf(stdout, "dj-ui: a press inside the loop switched it %s, playhead %.0f -> %.0f\n",
                     armedCount() > 0 ? "on" : "off", insideBefore, insideAfter);
        if (armedCount() == 0) {
            std::fprintf(stderr, "dj-ui: a press inside the loop did not switch the mode on\n");
            return 1;
        }
        if (std::abs(insideAfter - insideBefore) > 64.0) {
            std::fprintf(stderr, "dj-ui: a press inside the loop jumped the playhead\n");
            return 1;
        }

        // From outside the loop a press starts another one: it must mark a new start, not drag the hand
        // back into the loop that was already there.
        bridge->seek(QStringLiteral("A"), 5000);
        pump(20);
        const double outsideBefore = frames();
        click(Qt::NoModifier);
        const double outsideAfter = frames();
        const double freshMark = stripProp("pendingFrom");
        std::fprintf(stdout, "dj-ui: a press outside the loop left a marker at %.4f, playhead %.0f -> %.0f\n",
                     freshMark, outsideBefore, outsideAfter);
        if (freshMark < 0.0) {
            std::fprintf(stderr, "dj-ui: a press outside the loop did not mark a new start\n");
            return 1;
        }
        if (std::abs(outsideAfter - outsideBefore) > 64.0) {
            std::fprintf(stderr, "dj-ui: a press outside the loop jumped the playhead\n");
            return 1;
        }
        if (armedCount() != 0) {
            std::fprintf(stderr, "dj-ui: the old loop is still in force while a new one is being marked\n");
            return 1;
        }

        // A loop may not be born inside another: the pair starts outside and closes inside, and the
        // close is refused instead of cutting the existing loop in half.
        // An unfinished pair from the previous step would swallow the first press as a close.
        if (loopButton->property("pending").toBool()) {
            click(Qt::ShiftModifier);
            for (int step = 0; step < 30 && loopButton->property("pending").toBool(); ++step) {
                pump(2);
            }
        }
        bridge->setLoopMode(QStringLiteral("A"), false);
        // A loop of its own to protect, so the pair below really does cross it.
        bridge->setLoop(QStringLiteral("A"), true, 20000, 40000);
        pump(30);
        bridge->seek(QStringLiteral("A"), 15000);
        pump(20);
        click(Qt::NoModifier); // mark a start before it
        bridge->seek(QStringLiteral("A"), 30000);
        pump(20);
        const QVariantMap beforeRefusal = loopRegion();
        const int armedBefore = armedCount();
        click(Qt::NoModifier); // close it on top of the existing loop
        const QVariantMap afterRefusal = loopRegion();
        std::fprintf(stdout,
                     "dj-ui: a pair closed over an existing loop left it at %.0f…%.0f, armed %d, pending %s\n",
                     afterRefusal.value(QStringLiteral("start")).toDouble(),
                     afterRefusal.value(QStringLiteral("end")).toDouble(), armedCount(),
                     loopButton->property("pending").toBool() ? "yes" : "no");
        if (afterRefusal != beforeRefusal) {
            std::fprintf(stderr, "dj-ui: the refused pair changed the loop that was already there\n");
            return 1;
        }
        if (loopButton->property("pending").toBool()) {
            std::fprintf(stderr, "dj-ui: the refused pair is still waiting for a second press\n");
            return 1;
        }
        if (armedCount() != armedBefore) {
            std::fprintf(stderr, "dj-ui: the refused pair changed the armed set\n");
            return 1;
        }
        bridge->setLoopMode(QStringLiteral("A"), true);
        pump(30);
        if (loopButton->property("active").toBool() == false) {
            std::fprintf(stderr, "dj-ui: the mode did not come back for the next step\n");
            return 1;
        }

        // Switching the mode off while the loop is playing: the button goes dark and the track runs on
        // past the loop's end instead of repeating it.
        const double loopEnd = loopRegion().value(QStringLiteral("end")).toDouble();
        // An unfinished pair swallows the first shift, which is what cancelling it means; start clean.
        if (loopButton->property("pending").toBool()) {
            click(Qt::ShiftModifier);
            for (int step = 0; step < 30 && loopButton->property("pending").toBool(); ++step) {
                pump(2);
            }
        }
        bridge->setLoopMode(QStringLiteral("A"), true);
        // Shift toggles exactly what the button shows, so wait for the button itself to light up.
        for (int step = 0; step < 60 && !loopButton->property("active").toBool(); ++step) {
            pump(2);
        }
        if (!loopButton->property("active").toBool()) {
            std::fprintf(stderr, "dj-ui: the loop button did not light up for the mode\n");
            return 1;
        }
        bridge->seek(QStringLiteral("A"), qMax(0.0, loopEnd - 4000.0));
        bridge->play(QStringLiteral("A"), true);
        pump(24);
        click(Qt::ShiftModifier);
        const bool litAfterOff = loopButton->property("active").toBool();
        // The playhead must keep moving forward: a wrap sends it back, which is exactly what switching
        // the mode off has to stop.
        double ranOn = frames();
        double rewound = 0.0;
        double previous = ranOn;
        for (int step = 0; step < 12; ++step) {
            pump(6);
            const double now = frames();
            rewound = std::max(rewound, previous - now);
            previous = now;
            ranOn = std::max(ranOn, now);
        }
        std::fprintf(stdout,
                     "dj-ui: mode off while playing — button lit %s, armed %d, went back by %.0f, "
                     "playhead reached %.0f (loop end %.0f)\n",
                     litAfterOff ? "yes" : "no", armedCount(), rewound, ranOn, loopEnd);
        if (litAfterOff) {
            std::fprintf(stderr, "dj-ui: the loop button stayed lit after the mode was switched off\n");
            return 1;
        }
        if (armedCount() != 0) {
            std::fprintf(stderr, "dj-ui: loop mode is still armed after being switched off\n");
            return 1;
        }
        if (rewound > 1024.0) {
            std::fprintf(stderr, "dj-ui: the playhead jumped back — the loop still wrapped\n");
            return 1;
        }

        // Saving a loop while it plays is bookkeeping: the sound must not move. The playhead is watched
        // for a rewind, which is what a stale decoder cursor looks like from outside.
        bridge->setLoopMode(QStringLiteral("A"), true);
        for (int step = 0; step < 60 && !loopButton->property("active").toBool(); ++step) {
            pump(2);
        }
        bridge->play(QStringLiteral("A"), true);
        pump(30);
        const int armedBeforeSave = armedCount();
        double previousFrame = frames();
        double biggestRewind = 0.0;
        bridge->setPadLoop(QStringLiteral("A"), 6);
        for (int step = 0; step < 10; ++step) {
            pump(6);
            const double now = frames();
            biggestRewind = std::max(biggestRewind, previousFrame - now);
            previousFrame = now;
        }
        std::fprintf(stdout,
                     "dj-ui: saving a playing loop went back by %.0f frames, armed %d -> %d\n",
                     biggestRewind, armedBeforeSave, armedCount());
        if (biggestRewind > 1024.0) {
            std::fprintf(stderr, "dj-ui: saving a playing loop moved the sound backwards\n");
            return 1;
        }
        if (armedCount() < armedBeforeSave) {
            std::fprintf(stderr, "dj-ui: saving a playing loop disarmed it\n");
            return 1;
        }
        bridge->play(QStringLiteral("A"), false);
        pump(20);

        // Two saved loops and one switch: loop mode must repeat whichever one the playhead meets.
        click(Qt::ShiftModifier); // clear the pair that was just marked
        bridge->setLoopMode(QStringLiteral("A"), false);
        bridge->seek(QStringLiteral("A"), 20000);
        bridge->setLoop(QStringLiteral("A"), true, 20000, 40000);
        bridge->setPadLoop(QStringLiteral("A"), 1);
        bridge->seek(QStringLiteral("A"), 60000);
        bridge->setLoop(QStringLiteral("A"), true, 60000, 80000);
        bridge->setPadLoop(QStringLiteral("A"), 2);
        bridge->setLoopMode(QStringLiteral("A"), false);
        pump(40);
        const int disarmed = deckOf().value(QStringLiteral("active_loops")).toList().size();
        bridge->setLoopMode(QStringLiteral("A"), true);
        pump(40);
        const int armedNow = deckOf().value(QStringLiteral("active_loops")).toList().size();
        std::fprintf(stdout, "dj-ui: loop mode watches %d loop(s), off state had %d\n", armedNow,
                     disarmed);
        if (armedNow < 2 || disarmed != 0) {
            std::fprintf(stderr, "dj-ui: loop mode did not take in every saved loop\n");
            return 1;
        }

        // The strip has to agree with the mode: every saved loop bright, the button lit; and all of
        // them dim, the button dark, once the mode goes off.
        const auto litLoops = [&]() {
            int lit = 0;
            if (strip == nullptr) {
                return lit;
            }
            for (const QVariant &entry : strip->property("padLoops").toList()) {
                if (entry.toMap().value(QStringLiteral("active")).toBool()) {
                    ++lit;
                }
            }
            return lit;
        };
        const int litOn = litLoops();
        const bool buttonOn = loopButton->property("active").toBool();
        bridge->setLoopMode(QStringLiteral("A"), false);
        pump(40);
        const int litOff = litLoops();
        const bool buttonOff = loopButton->property("active").toBool();
        std::fprintf(stdout,
                     "dj-ui: with the mode on %d loop(s) lit and the button %s; off — %d lit, button %s\n",
                     litOn, buttonOn ? "lit" : "dark", litOff, buttonOff ? "lit" : "dark");
        if (litOn < 2 || !buttonOn) {
            std::fprintf(stderr, "dj-ui: the mode is on but the loops are not all lit\n");
            return 1;
        }
        if (litOff != 0 || buttonOff) {
            std::fprintf(stderr, "dj-ui: switching the mode off left loops lit or the button on\n");
            return 1;
        }
        bridge->setLoopMode(QStringLiteral("A"), true);
        pump(40);

        // Dropped before the first saved loop: it must wrap at that loop's end, not run on.
        bridge->seek(QStringLiteral("A"), 5000);
        bridge->play(QStringLiteral("A"), true);
        pump(40);
        double reached = frames();
        for (int step = 0; step < 12; ++step) {
            pump(6);
            reached = std::max(reached, frames());
        }
        std::fprintf(stdout, "dj-ui: from before the first loop the playhead reached %.0f (ends 40000)\n",
                     reached);
        if (reached > 52000.0) {
            std::fprintf(stderr, "dj-ui: the first saved loop did not repeat\n");
            return 1;
        }

        // Dropped inside the second saved loop: that one repeats.
        bridge->seek(QStringLiteral("A"), 62000);
        pump(30);
        reached = frames();
        for (int step = 0; step < 12; ++step) {
            pump(6);
            reached = std::max(reached, frames());
        }
        std::fprintf(stdout, "dj-ui: inside the second loop the playhead reached %.0f (ends 80000)\n",
                     reached);
        if (reached > 92000.0) {
            std::fprintf(stderr, "dj-ui: the second saved loop did not repeat\n");
            return 1;
        }

        bridge->setLoopMode(QStringLiteral("A"), false);
        bridge->play(QStringLiteral("A"), false);
        return 0;
    }

    if (parser.isSet(scenarioOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        window->resize(1366, 768);
        window->show();
        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --scenario-check needs --load\n");
            return 1;
        }
        bridge->load(QStringLiteral("A"), path);
        return djui::runScenarios(*bridge, *window, QStringLiteral("A"), path);
    }

    if (parser.isSet(padsOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        window->resize(1366, 768);
        window->show();
        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        pump(60);
        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --pads-check needs --load\n");
            return 1;
        }
        bridge->load(QStringLiteral("A"), path);
        // The loader decodes the file and measures its loudness before the graph attaches it, and an
        // attach puts the playhead at the start: a seek sent into that window is undone a moment later.
        pump(120);
        QElapsedTimer ready;
        ready.start();
        while (ready.elapsed() < 15000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(20);
            bool loaded = false;
            for (const QVariant &entry :
                 bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    loaded = map.value(QStringLiteral("remaining_frames")).toDouble() > 0.0;
                }
            }
            if (loaded) {
                break;
            }
        }

        const auto deckOf = [&]() {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return map;
                }
            }
            return QVariantMap();
        };
        const auto click = [&](QQuickItem *item, Qt::KeyboardModifiers modifiers) {
            if (item == nullptr) {
                return false;
            }
            const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
                QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, modifiers);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            }
            pump(30);
            return true;
        };

        // Park the playhead somewhere recognisable, then set pad 3 on it. The seek is repeated until it
        // sticks: a load still landing resets the playhead to the start of the track, and a check that
        // raced it would store the cue at zero and blame the pad.
        double parked = 0.0;
        for (int attempt = 0; attempt < 20; ++attempt) {
            bridge->seek(QStringLiteral("A"), 96000);
            pump(20);
            parked = deckOf().value(QStringLiteral("source_frame")).toDouble();
            if (std::abs(parked - 96000.0) < 512.0) {
                break;
            }
        }
        std::fprintf(stdout, "dj-ui: the playhead stands at %.0f before the pad is clicked\n", parked);
        if (std::abs(parked - 96000.0) > 512.0) {
            std::fprintf(stderr, "dj-ui: the playhead could not be parked for the check\n");
            return 1;
        }
        auto *pad = findItem(window->contentItem(), QStringLiteral("cuepad_A_3"));
        if (pad == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no pad 3\n");
            return 1;
        }
        {
            // Control hand: the same command without the mouse. If this one also lands at zero, the
            // pad is not the suspect.
            bridge->cue(QStringLiteral("A"), QStringLiteral("Set"), 5);
            pump(30);
            const QVariantMap controlled = deckOf();
            const QVariantList cuesNow = controlled.value(QStringLiteral("hotcues")).toList();
            std::fprintf(stdout,
                         "dj-ui: the same command without the mouse put pad 5 at %.0f with the "
                         "playhead at %.0f\n",
                         cuesNow.size() > 5 && !cuesNow[5].isNull() ? cuesNow[5].toDouble() : -1.0,
                         controlled.value(QStringLiteral("source_frame")).toDouble());
        }
        std::fprintf(stdout, "dj-ui: pad 3 sits at %.0f,%.0f %.0fx%.0f in a %dx%d window\n",
                     pad->mapToScene(QPointF(0, 0)).x(), pad->mapToScene(QPointF(0, 0)).y(),
                     pad->width(), pad->height(), window->width(), window->height());
        click(pad, Qt::NoModifier);
        {
            const QVariantMap after = deckOf();
            const QVariantList cuesNow = after.value(QStringLiteral("hotcues")).toList();
            QStringList parts;
            for (const QVariant &cue : cuesNow) {
                parts << (cue.isNull() ? QStringLiteral("—") : QString::number(cue.toDouble()));
            }
            std::fprintf(stdout, "dj-ui: after the click the deck's load state is %s\n",
                         qPrintable(QJsonDocument::fromVariant(
                                        after.value(QStringLiteral("load")))
                                        .toJson(QJsonDocument::Compact)));
            std::fprintf(stdout, "dj-ui: after the click the pads read [%s] at playhead %.0f\n",
                         qPrintable(parts.join(QStringLiteral(", "))),
                         after.value(QStringLiteral("source_frame")).toDouble());
        }
        const QVariantMap set = deckOf();
        const QVariantList cues = set.value(QStringLiteral("hotcues")).toList();
        const double stored = cues.size() > 3 && cues[3].isValid() && !cues[3].isNull()
                                  ? cues[3].toDouble() : -1.0;
        std::fprintf(stdout, "dj-ui: clicking an empty pad 3 stored a cue at %.0f\n", stored);
        if (std::abs(stored - 96000.0) > 2000.0) {
            std::fprintf(stderr, "dj-ui: the pad did not store the playhead\n");
            return 1;
        }

        // A name and a colour travel through the engine and come back in the snapshot.
        bridge->setCueMeta(QStringLiteral("A"), 3, QStringLiteral("Drop"),
                           QStringLiteral("#5fb85f"));
        pump(40);
        const QVariantMap named = deckOf();
        const QVariantList labels = named.value(QStringLiteral("cue_labels")).toList();
        const QVariantList colors = named.value(QStringLiteral("cue_colors")).toList();
        const QString label = labels.size() > 3 ? labels[3].toString() : QString();
        const QString color = colors.size() > 3 ? colors[3].toString() : QString();
        std::fprintf(stdout, "dj-ui: the pad is called %s and coloured %s\n", qPrintable(label),
                     qPrintable(color));
        if (label != QStringLiteral("Drop") || color != QStringLiteral("#5fb85f")) {
            std::fprintf(stderr, "dj-ui: the pad's name or colour did not survive the round trip\n");
            return 1;
        }

        // The strip has to be told about the pad: drawing is the point of storing it.
        {
            auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
            const QVariantList marks = strip != nullptr ? strip->property("cues").toList()
                                                        : QVariantList();
            std::fprintf(stdout, "dj-ui: the waveform strip carries %d pad mark(s)\n",
                         int(marks.size()));
            if (marks.isEmpty()) {
                std::fprintf(stderr, "dj-ui: the pad did not reach the waveform strip\n");
                return 1;
            }
            const QVariantMap first = marks.first().toMap();
            const QString colour = first.value(QStringLiteral("color")).toString();
            const QString label = first.value(QStringLiteral("label")).toString();
            std::fprintf(stdout, "dj-ui: the mark wears %s and is called %s at %.3f of the track\n",
                         qPrintable(colour), qPrintable(label),
                         first.value(QStringLiteral("ratio")).toDouble());
            if (colour != QStringLiteral("#5fb85f") || label != QStringLiteral("Drop")) {
                std::fprintf(stderr, "dj-ui: the mark lost its colour or its name on the way\n");
                return 1;
            }
        }

        // Clicking a set pad jumps to it.
        bridge->seek(QStringLiteral("A"), 10000);
        pump(30);
        click(pad, Qt::NoModifier);
        const QVariantMap jumped = deckOf();
        const double landed = jumped.value(QStringLiteral("source_frame")).toDouble();
        std::fprintf(stdout, "dj-ui: clicking the pad again moved the playhead to %.0f\n", landed);
        if (std::abs(landed - stored) > 2000.0) {
            std::fprintf(stderr, "dj-ui: the pad did not jump to its cue\n");
            return 1;
        }

        // Shift-click clears it.
        click(pad, Qt::ShiftModifier);
        const QVariantMap cleared = deckOf();
        const QVariantList after = cleared.value(QStringLiteral("hotcues")).toList();
        const bool stillThere = after.size() > 3 && after[3].isValid() && !after[3].isNull();
        std::fprintf(stdout, "dj-ui: shift-click left pad 3 %s\n", stillThere ? "set" : "empty");
        if (stillThere) {
            std::fprintf(stderr, "dj-ui: shift-click did not clear the pad\n");
            return 1;
        }

        // A pad that holds a loop is deleted the same way: the loop goes with the rest of the pad.
        bridge->setLoop(QStringLiteral("A"), true, 96000, 144000);
        pump(30);
        bridge->setPadLoop(QStringLiteral("A"), 4);
        pump(40);
        const QVariantList loopsSet = deckOf().value(QStringLiteral("pad_loops")).toList();
        const bool heldLoop = loopsSet.size() > 4 && loopsSet[4].isValid() && !loopsSet[4].isNull();
        std::fprintf(stdout, "dj-ui: pad 4 %s a loop\n", heldLoop ? "holds" : "does not hold");
        if (!heldLoop) {
            std::fprintf(stderr, "dj-ui: the loop did not reach pad 4\n");
            return 1;
        }
        auto *padFour = findItem(window->contentItem(), QStringLiteral("cuepad_A_4"));
        if (padFour == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no pad 4\n");
            return 1;
        }
        click(padFour, Qt::ShiftModifier);
        pump(30);
        const QVariantList loopsAfter = deckOf().value(QStringLiteral("pad_loops")).toList();
        const bool loopLeft = loopsAfter.size() > 4 && loopsAfter[4].isValid() && !loopsAfter[4].isNull();
        const int armedLeft = int(deckOf().value(QStringLiteral("active_loops")).toList().size());
        std::fprintf(stdout, "dj-ui: shift-click left pad 4 %s loop, %d loop(s) armed\n",
                     loopLeft ? "holding a" : "without a", armedLeft);
        if (loopLeft) {
            std::fprintf(stderr, "dj-ui: shift-click did not clear the loop on the pad\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(transportOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        window->resize(1366, 768);
        window->show();
        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        pump(60);

        const auto fieldOf = [&](const char *field) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                    continue;
                }
                return map.value(QString::fromLatin1(field)).toDouble();
            }
            return -1.0;
        };
        const auto playing = [&]() {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return map.value(QStringLiteral("playing")).toBool();
                }
            }
            return false;
        };
        // The point Stop returns to is hotcue 0 when the track carries one, the start of the file
        // otherwise: the check reads the deck's own answer instead of assuming zero.
        const auto cuePoint = [&]() {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                    continue;
                }
                // `slots` is a Qt keyword: the list is named for what it holds.
                const QVariantList cues = map.value(QStringLiteral("hotcues")).toList();
                if (!cues.isEmpty() && cues[0].isValid() && !cues[0].isNull()) {
                    return cues[0].toDouble();
                }
                return 0.0;
            }
            return 0.0;
        };
        const auto press = [&](QQuickItem *item) {
            if (item == nullptr) {
                return false;
            }
            const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            QMouseEvent down(QEvent::MouseButtonPress, at, window->mapToGlobal(at.toPoint()),
                             Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &down);
            QCoreApplication::processEvents();
            return true;
        };
        const auto release = [&](QQuickItem *item) {
            if (item == nullptr) {
                return false;
            }
            const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
            QMouseEvent up(QEvent::MouseButtonRelease, at, window->mapToGlobal(at.toPoint()),
                           Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &up);
            QCoreApplication::processEvents();
            return true;
        };

        // Loaded already by --load; wait for the deck to carry a track.
        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --transport-check needs --load\n");
            return 1;
        }
        // A check of the transport needs a track: the load is part of the setup, not something the
        // production code does on its own.
        bridge->load(QStringLiteral("A"), path);
        QElapsedTimer ready;
        ready.start();
        while (ready.elapsed() < 15000 && fieldOf("remaining_frames") <= 0.0) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(20);
        }
        // A fresh engine is not guaranteed: the checks of a gate run share one, so the deck is put in a
        // known state before the transport is measured — silent, at its cue point.
        bridge->play(QStringLiteral("A"), false);
        pump(30);
        bridge->seek(QStringLiteral("A"), 0);
        pump(30);
        // The snapshot carries the position and what is left of it; their sum is the track.
        const double totalFrames = fieldOf("source_frame")
                                   + fieldOf("remaining_frames");
        auto *playButton = findItem(window->contentItem(), QStringLiteral("play_A"));
        auto *stopButton = findItem(window->contentItem(), QStringLiteral("stop_A"));
        auto *cueButton = findItem(window->contentItem(), QStringLiteral("cue_A"));
        auto *overview = findItem(window->contentItem(), QStringLiteral("deckOverview_A"));
        auto *clock = findItem(window->contentItem(), QStringLiteral("clock_A"));
        if (playButton == nullptr || stopButton == nullptr || cueButton == nullptr
            || overview == nullptr || clock == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck is missing its transport, its clock or its overview\n");
            return 1;
        }

        // Play, let the playhead move, then Stop: the deck must be silent and back at the cue point.
        press(playButton);
        pump(5);
        release(playButton);
        pump(60);
        if (!playing()) {
            std::fprintf(stderr, "dj-ui: the play button did not start the deck\n");
            return 1;
        }
        const double moved = fieldOf("source_frame");
        const double cue = cuePoint();
        press(stopButton);
        release(stopButton);
        pump(40);
        const double stoppedAt = fieldOf("source_frame");
        std::fprintf(stdout,
                     "dj-ui: play moved the playhead to %.0f, stop put it back at %.0f (the cue point "
                     "is %.0f) and left the deck %s\n",
                     moved, stoppedAt, cue, playing() ? "playing" : "silent");
        if (playing()) {
            std::fprintf(stderr, "dj-ui: stop left the deck playing\n");
            return 1;
        }
        if (std::abs(stoppedAt - cue) > 1.0) {
            std::fprintf(stderr, "dj-ui: stop did not return the playhead to the cue point\n");
            return 1;
        }

        // Scrub: drag the hand across the overview and require the engine's playhead to follow.
        const QPointF from = overview->mapToScene(QPointF(overview->width() * 0.2, overview->height() / 2));
        const QPointF to = overview->mapToScene(QPointF(overview->width() * 0.6, overview->height() / 2));
        QMouseEvent down(QEvent::MouseButtonPress, from, window->mapToGlobal(from.toPoint()),
                         Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &down);
        QMouseEvent along(QEvent::MouseMove, to, window->mapToGlobal(to.toPoint()), Qt::LeftButton,
                          Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &along);
        QMouseEvent up(QEvent::MouseButtonRelease, to, window->mapToGlobal(to.toPoint()),
                       Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &up);
        pump(40);
        const double scrubbed = fieldOf("source_frame");
        const double total = totalFrames;
        std::fprintf(stdout, "dj-ui: scrubbing to sixty per cent of the overview put the playhead at "
                             "%.0f of %.0f frames\n", scrubbed, total);
        if (total <= 0.0 || scrubbed < total * 0.45 || scrubbed > total * 0.75) {
            std::fprintf(stderr, "dj-ui: the scrub did not move the playhead where the hand went\n");
            return 1;
        }

        // Scrubbing by hand on the zoomed strip: dragging to the right takes the material back, and in
        // the default silent mode the transport is left alone.
        {
            auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
            if (strip == nullptr) {
                std::fprintf(stderr, "dj-ui: deck A has no waveform strip\n");
                return 1;
            }
            const bool wasPlaying = playing();
            const double before = fieldOf("source_frame");
            const QPointF grab =
                strip->mapToScene(QPointF(strip->width() * 0.5, strip->height() * 0.5));
            const QPointF moved = grab + QPointF(60, 0);
            for (const auto &[type, point] :
                 {std::pair{QEvent::MouseButtonPress, grab}, std::pair{QEvent::MouseMove, moved},
                  std::pair{QEvent::MouseButtonRelease, moved}}) {
                QMouseEvent event(type, point, window->mapToGlobal(point.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            }
            pump(40);
            const double after = fieldOf("source_frame");
            std::fprintf(stdout,
                         "dj-ui: dragging the strip 60 px right moved the playhead from %.0f to "
                         "%.0f, transport %s\n",
                         before, after, playing() == wasPlaying ? "unchanged" : "changed");
            if (after >= before - 500.0) {
                std::fprintf(stderr, "dj-ui: the hand on the strip did not move the material back\n");
                return 1;
            }
            if (playing() != wasPlaying) {
                std::fprintf(stderr, "dj-ui: a silent scrub changed the transport\n");
                return 1;
            }
        }

        // A silent deck with the sound switched on: the noise must come from the hand, not from a deck
        // that quietly started playing. Moving makes sound; stopping must be silent again.
        {
            auto *mode = findItem(window->contentItem(), QStringLiteral("scrubMode_A"));
            auto *strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
            if (mode == nullptr || strip == nullptr) {
                std::fprintf(stderr, "dj-ui: deck A has no scrub mode or no strip\n");
                return 1;
            }
            // Button click: press and release at its centre.
            const QPointF at = mode->mapToScene(QPointF(mode->width() / 2.0, mode->height() / 2.0));
            for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
                QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            }
            pump(20);
            if (playing()) {
                std::fprintf(stderr, "dj-ui: the deck was playing before the audio scrub began\n");
                return 1;
            }

            const QPointF grab =
                strip->mapToScene(QPointF(strip->width() * 0.5, strip->height() * 0.5));
            QMouseEvent down(QEvent::MouseButtonPress, grab, window->mapToGlobal(grab.toPoint()),
                             Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &down);
            QCoreApplication::processEvents();
            pump(4);
            const double beforeMove = fieldOf("source_frame");
            // A hand that has not moved yet makes no sound.
            pump(20);
            if (playing()) {
                std::fprintf(stderr, "dj-ui: holding the strip started the deck\n");
                return 1;
            }
            const QPointF along = grab + QPointF(40, 0);
            QMouseEvent move(QEvent::MouseMove, along, window->mapToGlobal(along.toPoint()),
                             Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &move);
            QCoreApplication::processEvents();
            const double afterMove = fieldOf("source_frame");
            // The hand has moved and stops; the button is still down. Nothing may run on: whatever the
            // scratch sounded like, the playhead has to stay where the hand left it.
            QElapsedTimer held;
            held.start();
            while (held.elapsed() < 400) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(10);
            }
            const double afterHold = fieldOf("source_frame");
            QMouseEvent up(QEvent::MouseButtonRelease, along, window->mapToGlobal(along.toPoint()),
                           Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &up);
            pump(20);
            const double afterRelease = fieldOf("source_frame");
            const bool quietAfter = !playing();
            std::fprintf(stdout,
                         "dj-ui: audio scrub — the hand moved the playhead to %.0f, 400 ms of a held "
                         "button left it at %.0f, releasing left it at %.0f, deck %s\n",
                         afterMove, afterHold, afterRelease, quietAfter ? "silent" : "playing");
            // Whether the hand moves the material is proved by the silent drag above; what matters here
            // is that nothing keeps running after the hand stops.
            (void)beforeMove;
            if (std::abs(afterHold - afterMove) > 4800.0) {
                std::fprintf(stderr, "dj-ui: the deck ran on after the hand stopped\n");
                return 1;
            }
            if (!quietAfter || std::abs(afterRelease - afterHold) > 4800.0) {
                std::fprintf(stderr, "dj-ui: releasing the hand did not leave the deck where it stood\n");
                return 1;
            }
        }

        // The clock counts down, so it must show the time left, not the length.
        const QString shown = clock->property("text").toString();
        std::fprintf(stdout, "dj-ui: the deck's clock reads %s\n", qPrintable(shown));
        if (!shown.contains(QStringLiteral("−")) || shown.contains(QStringLiteral("—"))) {
            std::fprintf(stderr, "dj-ui: the clock does not show the time left\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(syncCheckOption)) {
        // The engine's own measure of sync is in S2 and lives in cargo tests; what is checked here is
        // that a hand can reach it and read it: two buttons, a snapshot and a needle (DJP-SYNC-001/002).
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString first = parser.value(loadOption);
        const QString second = parser.value(loadSecondOption);
        if (first.isEmpty() || second.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --sync-check needs --load and --load-b\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        const auto waitFor = [](const std::function<bool()> &ready, int budget) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (ready()) {
                    return true;
                }
                QThread::msleep(10);
            }
            return ready();
        };
        if (!waitFor([&]() { return bridge->isConnected(); }, 3000)) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto deckOf = [&](const QString &id) {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == id) {
                    return deck;
                }
            }
            return QVariantMap();
        };
        // A mode of `off` is left off the wire, so an absent field is the deck following nothing — not a
        // missing answer. Reading it as an empty string would make "stop following" untestable.
        const auto modeOf = [&](const QString &id) {
            const QString mode = deckOf(id).value(QStringLiteral("sync")).toString();
            return mode.isEmpty() ? QStringLiteral("off") : mode;
        };
        const auto press = [&](const QString &name) {
            auto *item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s to press\n", qPrintable(name));
                return false;
            }
            QMetaObject::invokeMethod(item, "clicked");
            QCoreApplication::processEvents();
            return true;
        };

        bridge->load(QStringLiteral("A"), first);
        bridge->load(QStringLiteral("B"), second);
        const QString first_name = QFileInfo(first).fileName();
        const QString second_name = QFileInfo(second).fileName();
        const bool loaded = waitFor(
            [&]() {
                return deckOf(QStringLiteral("A")).value(QStringLiteral("path")).toString().endsWith(first_name)
                       && deckOf(QStringLiteral("B")).value(QStringLiteral("path")).toString().endsWith(second_name)
                       && deckOf(QStringLiteral("A")).value(QStringLiteral("beat")).isValid();
            },
            20000);
        if (!loaded) {
            std::fprintf(stderr, "dj-ui: the two decks did not both come up with a beat\n");
            return 1;
        }

        if (!press(QStringLiteral("syncMaster_A")) || !press(QStringLiteral("syncBeat_B"))) {
            return 1;
        }
        const bool locked = waitFor(
            [&]() {
                return deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool()
                       && modeOf(QStringLiteral("B")) == QStringLiteral("beat");
            },
            5000);
        if (!locked) {
            std::fprintf(stderr, "dj-ui: the master and sync buttons did not reach the engine (A master %d, "
                                 "B sync %s)\n",
                         deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool(),
                         qPrintable(modeOf(QStringLiteral("B"))));
            return 1;
        }
        // A deck that stands still publishes no phase: the distance from a beat that is not moving is a
        // clock, not a phase, and the meter would sweep. So the meter's presence is checked here and the
        // number itself is checked once the deck plays.
        auto *meter = findItem(window->contentItem(), QStringLiteral("phaseMeter_B"));
        auto *needle = findItem(window->contentItem(), QStringLiteral("phaseNeedle_B"));
        // The strip takes a frame or two to re-bind after the mode reaches the deck, and the meter is
        // only drawn while a mode is on: wait for it rather than assert on the first look.
        const bool meter_shown = waitFor(
            [&]() {
                return meter != nullptr && needle != nullptr && meter->isVisible();
            },
            5000);
        if (!meter_shown) {
            std::fprintf(stderr,
                         "dj-ui: the phase meter is missing or hidden while sync is on (meter %d, "
                         "needle %d)\n",
                         meter != nullptr, needle != nullptr);
            return 1;
        }
        // The meter is laid out with the rest of the strip, so its width is zero until a frame has
        // passed: measuring before that would compare a needle with a bar that has no length.
        const bool drawn = waitFor([&]() { return meter->width() > 20.0; }, 3000);
        if (!drawn) {
            std::fprintf(stderr, "dj-ui: the phase meter was never laid out (%.0f px)\n", meter->width());
            return 1;
        }
        const double centre = meter->width() / 2.0;
        // Standing still, both decks on the same frame: the needle belongs in the centre of the bar.
        const bool parked = waitFor([&]() { return std::abs(needle->x() - centre) <= 2.0; }, 3000);
        if (!parked) {
            std::fprintf(stderr,
                         "dj-ui: the needle stands at %.0f of %.0f with both decks on the same frame\n",
                         needle->x(), centre);
            return 1;
        }

        // Sound, not only numbers. A follow that keeps the deck silent looks perfect in a snapshot and
        // is useless in a mix. The meter is a peak that decays between the hits of a drum loop, so it has
        // to be watched over a window: one reading in a gap says nothing about a deck.
        const auto peakOf = [&](const QString &id) {
            return deckOf(id)
                .value(QStringLiteral("levels"))
                .toMap()
                .value(QStringLiteral("peak_left"))
                .toDouble();
        };
        const auto loudestOver = [&](const QString &id, int budget_ms) {
            double loudest = 0.0;
            QElapsedTimer window;
            window.start();
            while (window.elapsed() < budget_ms) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                loudest = std::max(loudest, peakOf(id));
                QThread::msleep(10);
            }
            return loudest;
        };
        const auto barErrorOf = [&](const QString &id) {
            const QVariant bar = deckOf(id).value(QStringLiteral("bar_error_beats"));
            return bar.isValid() ? bar.toDouble() : 0.0;
        };
        const auto frameOf = [&](const QString &id) {
            return deckOf(id).value(QStringLiteral("source_frame")).toDouble();
        };

        // The follower first, alone: a deck that cannot sound on its own makes the rest of this stage
        // measure nothing. The master has not played yet, so this is the deck and nothing else.
        bridge->play(QStringLiteral("B"), true);
        const double bare = loudestOver(QStringLiteral("B"), 2500);
        if (bare <= 0.01) {
            std::fprintf(stderr,
                         "dj-ui: the follower makes no sound on its own (deck peak %.4f)\n",
                         bare);
            return 1;
        }
        bridge->play(QStringLiteral("A"), true);
        const double master_heard = loudestOver(QStringLiteral("A"), 2500);
        if (master_heard <= 0.01) {
            std::fprintf(stderr, "dj-ui: the master makes no sound (deck peak %.4f)\n", master_heard);
            return 1;
        }
        // Both playing and following: the follower is placed on the master's beat and must stay on it.
        const bool locked_playing = waitFor(
            [&]() {
                const QVariant phase = deckOf(QStringLiteral("B")).value(QStringLiteral("phase_error_ms"));
                return phase.isValid() && std::abs(phase.toDouble()) < 2.0;
            },
            8000);
        if (!locked_playing) {
            std::fprintf(stderr, "dj-ui: the follower did not come onto the master's beat\n");
            return 1;
        }
        const double phase_playing =
            deckOf(QStringLiteral("B")).value(QStringLiteral("phase_error_ms")).toDouble();

        // A stopped follower next to a playing master: its play head must not creep. Every step the
        // master's beat moves on, and a jump onto it drags a stopped deck backwards through the track.
        bridge->play(QStringLiteral("B"), false);
        const bool follower_stopped = waitFor(
            [&]() {
                return !deckOf(QStringLiteral("B")).value(QStringLiteral("playing")).toBool();
            },
            5000);
        if (!follower_stopped) {
            std::fprintf(stderr, "dj-ui: the follower would not stop\n");
            return 1;
        }
        const double held_at = frameOf(QStringLiteral("B"));
        const double needle_held = needle->x();
        QElapsedTimer creep;
        creep.start();
        while (creep.elapsed() < 1200) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        const double held_after = frameOf(QStringLiteral("B"));
        const double needle_after_stop = needle->x();
        if (std::abs(held_after - held_at) > 0.5) {
            std::fprintf(stderr,
                         "dj-ui: the stopped follower was dragged %.0f frames while the master played\n",
                         held_after - held_at);
            return 1;
        }
        if (std::abs(needle_after_stop - needle_held) > 1.0) {
            std::fprintf(stderr,
                         "dj-ui: the needle moved %.1f px over a stopped deck\n",
                         needle_after_stop - needle_held);
            return 1;
        }

        // Started again: back onto the beat.
        bridge->play(QStringLiteral("B"), true);
        const bool back_on_beat = waitFor(
            [&]() {
                const QVariant phase = deckOf(QStringLiteral("B")).value(QStringLiteral("phase_error_ms"));
                return phase.isValid() && std::abs(phase.toDouble()) < 2.0;
            },
            8000);
        if (!back_on_beat) {
            std::fprintf(stderr, "dj-ui: a restarted follower did not come back onto the beat\n");
            return 1;
        }
        // The middle of the bar, taken from the meter itself: the needle before the arm is not a
        // reference, because a deck can be on the beat and a beat along the bar at the same time.
        const double middle = meter->width() / 2.0 - 1.0;

        // The control arm for the meter: it draws the bar, so a deck moved a whole beat shows a quarter
        // of the bar, and a needle wired to the centre cannot pass this. Tempo mode first, because beat
        // sync would put the deck back on the beat inside half a second and take the arm with it.
        if (!press(QStringLiteral("syncBeat_B")) || !press(QStringLiteral("syncTempo_B"))) {
            return 1;
        }
        const bool tempo_for_the_arm = waitFor(
            [&]() {
                return modeOf(QStringLiteral("B")) == QStringLiteral("tempo");
            },
            5000);
        if (!tempo_for_the_arm) {
            std::fprintf(stderr, "dj-ui: the tempo button did not take\n");
            return 1;
        }
        const QVariantMap follower = deckOf(QStringLiteral("B"));
        const double rate = follower.value(QStringLiteral("source_rate")).toDouble();
        const double beat_ms = 60000.0 / follower.value(QStringLiteral("local_bpm")).toDouble();
        const double here = frameOf(QStringLiteral("B"));
        bridge->seek(QStringLiteral("B"), static_cast<qulonglong>(here + rate * beat_ms / 1000.0));
        const bool bar_moved = waitFor(
            [&]() {
                return std::abs(barErrorOf(QStringLiteral("B"))) > 0.8;
            },
            5000);
        if (!bar_moved) {
            std::fprintf(stderr,
                         "dj-ui: a beat along the bar reads %.3f beats\n",
                         barErrorOf(QStringLiteral("B")));
            return 1;
        }
        const bool needleMoved = waitFor([&]() { return std::abs(needle->x() - middle) > 5.0; }, 3000);
        if (!needleMoved) {
            std::fprintf(stderr,
                         "dj-ui: the needle stayed at %.0f while the deck stood a beat along the bar\n",
                         needle->x());
            return 1;
        }
        const double offCentre = needle->x() - middle;
        const double bar_along = barErrorOf(QStringLiteral("B"));
        // The needle has to lean the way the deck has to move, and by a distance a person can see.
        if ((offCentre > 0.0) != (bar_along > 0.0) || std::abs(offCentre) > 40.0) {
            std::fprintf(stderr,
                         "dj-ui: the needle leans %.1f px for a deck %.3f beats along the bar\n",
                         offCentre, bar_along);
            return 1;
        }

        // Released: the mode button does what it says.
        if (!press(QStringLiteral("syncTempo_B"))) {
            return 1;
        }
        const bool released = waitFor(
            [&]() {
                return modeOf(QStringLiteral("B")) == QStringLiteral("off");
            },
            5000);
        if (!released) {
            std::fprintf(stderr, "dj-ui: the deck would not stop following\n");
            return 1;
        }

        // Putting the master down: the mode goes with it and the deck plays on exactly as it was — the
        // tempo a hand set is a decision, not a state to undo (owner's rule).
        if (!press(QStringLiteral("syncBeat_B"))) {
            return 1;
        }
        const bool following_again = waitFor(
            [&]() {
                return modeOf(QStringLiteral("B")) == QStringLiteral("beat");
            },
            5000);
        if (!following_again) {
            std::fprintf(stderr, "dj-ui: the sync button did not take before the master went\n");
            return 1;
        }
        const double ratio_before = deckOf(QStringLiteral("B"))
                                        .value(QStringLiteral("tempo_ratio"))
                                        .toDouble();
        if (!press(QStringLiteral("syncMaster_A"))) {
            return 1;
        }
        const bool leader_gone = waitFor(
            [&]() {
                return !deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool()
                       && modeOf(QStringLiteral("B")) == QStringLiteral("off");
            },
            5000);
        if (!leader_gone) {
            std::fprintf(stderr,
                         "dj-ui: with the master put down the follower is A master %d, B mode %s\n",
                         deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool(),
                         qPrintable(modeOf(QStringLiteral("B"))));
            return 1;
        }
        const double ratio_after = deckOf(QStringLiteral("B"))
                                       .value(QStringLiteral("tempo_ratio"))
                                       .toDouble();
        const double head_at_release = frameOf(QStringLiteral("B"));
        QElapsedTimer playing_on;
        playing_on.start();
        while (playing_on.elapsed() < 700) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        const double head_later = frameOf(QStringLiteral("B"));
        if (std::abs(ratio_after - ratio_before) > 1e-9) {
            std::fprintf(stderr,
                         "dj-ui: the tempo moved when the master went (%.9 -> %.9)\n",
                         ratio_before, ratio_after);
            return 1;
        }
        if (head_later - head_at_release < 0.3 * rate) {
            std::fprintf(stderr,
                         "dj-ui: the deck stopped when the master went (%.0f frames in 0,7 s)\n",
                         head_later - head_at_release);
            return 1;
        }

        // Both decks back to quiet: the next check loads its own file, and a deck that is still playing
        // does not take a load.
        bridge->play(QStringLiteral("A"), false);
        bridge->play(QStringLiteral("B"), false);
        waitFor(
            [&]() {
                return !deckOf(QStringLiteral("A")).value(QStringLiteral("playing")).toBool()
                       && !deckOf(QStringLiteral("B")).value(QStringLiteral("playing")).toBool();
            },
            5000);

        std::fprintf(stdout,
                     "dj-ui: sync — MASTER on A and SYNC on B put the follower on the beat (phase %.3f "
                     "ms), a stopped follower held its head at %.0f frames for 1,2 s while the needle "
                     "stood still, one beat along the bar (%.3f beats) moved the needle %.0f px off "
                     "centre, and putting the master down cleared the mode while the deck kept its "
                     "speed %.9f and played %.0f frames on\n",
                     phase_playing, held_at, bar_along, offCentre, ratio_after,
                     head_later - head_at_release);
        return 0;
    }

    if (parser.isSet(pitchOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }
        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        const auto tempoOf = [&](const char *deck) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() == QString::fromLatin1(deck)) {
                    return map.value(QStringLiteral("tempo_ratio")).toDouble();
                }
            }
            return -1.0;
        };
        auto *pitch = findItem(window->contentItem(), QStringLiteral("pitch_A"));
        if (pitch == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no pitch fader\n");
            return 1;
        }
        // The material measurement below needs something on the deck: load what --load names and wait
        // for the attach, the same way the other checks do.
        {
            const QString track = parser.value(loadOption);
            if (!track.isEmpty()) {
                bridge->load(QStringLiteral("A"), track);
                pump(120);
                QElapsedTimer ready;
                ready.start();
                while (ready.elapsed() < 15000) {
                    bool attached = false;
                    for (const QVariant &entry :
                         bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                        const QVariantMap map = entry.toMap();
                        if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                            attached = map.value(QStringLiteral("remaining_frames")).toDouble() > 0.0;
                        }
                    }
                    if (attached) {
                        break;
                    }
                    QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    QThread::msleep(20);
                }
            }
        }
        pump(20);
        const double perPixel = pitch->property("ratioPerPixel").toDouble();
        if (!(perPixel > 0.0)) {
            std::fprintf(stderr, "dj-ui: the pitch fader has no throw (%g per pixel)\n", perPixel);
            return 1;
        }

        // The point is recomputed for every gesture: loading a track changes the deck's layout (the
        // level correction appears beside the clock), and a point captured before that lands somewhere
        // else on the fader.
        const auto grabPoint = [&] {
            return pitch->mapToScene(
                QPointF(pitch->width() / 2.0, pitch->height() / 2.0));
        };
        const QPointF grab = grabPoint();
        const auto send = [&](QEvent::Type type, const QPointF &at) {
            // A release carries no button: with `LeftButton` in the buttons mask the mouse area never
            // sees the drag end, and the next press is measured from a stale position.
            const Qt::MouseButtons buttons =
                type == QEvent::MouseButtonRelease ? Qt::NoButton : Qt::LeftButton;
            QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton, buttons,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
        };
        // The throw is a setting: ±16 % by default, and the whole travel of the fader is that range.
        // Checked both ways — the fader's own numbers, and a real drag whose expected rate is computed
        // from them — because a fader can show a range it cannot reach.
        {
            const double rangePercent = pitch->property("rangePercent").toDouble();
            const double minimum = pitch->property("minimum").toDouble();
            const double maximum = pitch->property("maximum").toDouble();
            const double perPixelNow = pitch->property("ratioPerPixel").toDouble();
            const double inset = pitch->property("inset").toDouble();
            std::fprintf(stdout, "dj-ui: the pitch throw is ±%.1f %% (%.3f … %.3f)\n",
                         rangePercent, minimum, maximum);
            if (std::abs(rangePercent - 16.0) > 1e-6 || std::abs(minimum - 0.84) > 1e-6
                || std::abs(maximum - 1.16) > 1e-6) {
                std::fprintf(stderr, "dj-ui: the throw is %.1f %% (%.3f … %.3f), not ±16 %%\n",
                             rangePercent, minimum, maximum);
                return 1;
            }
            // The travel is the range: when the fader has been laid out, its pixels must cover the
            // whole throw. A collapsed layout (height 0) says nothing either way, so it is not asked.
            if (pitch->height() > 100.0) {
                const double travel = (pitch->height() - 2.0 * inset) * perPixelNow;
                if (std::abs(travel - (maximum - minimum)) > 0.002) {
                    std::fprintf(stderr,
                                 "dj-ui: the fader's travel covers %.4f, but the range is %.4f\n",
                                 travel, maximum - minimum);
                    return 1;
                }
            }
        }

        // Let the release's own events drain before resetting: a move still queued would otherwise
        // re-send the old rate on top of the reset.
        pump(40);
        // Back to the file's own speed, so the next gesture starts from a known place. The fader reads
        // its position from the engine's snapshot, so the reset has to make the round trip.
        QMetaObject::invokeMethod(pitch, "ask", Q_ARG(QVariant, QVariant(1.0)));
        {
            QElapsedTimer settle;
            settle.start();
            while (settle.elapsed() < 2000 && std::abs(tempoOf("A") - 1.0) > 1e-6) {
                pump(2);
            }
            std::fprintf(stdout, "dj-ui: after the reset the fader reads %.4f (engine %.4f)\n",
                         pitch->property("ratio").toDouble(), tempoOf("A"));
        }

        // Ten pixels up from unity, on the same fader: the middle of the throw still moves by the hand.
        const QPointF centre = grabPoint();
        send(QEvent::MouseButtonPress, centre);
        send(QEvent::MouseMove, centre - QPointF(0, 10));
        pump(30);
        const double asked = pitch->property("lastRatio").toDouble();
        const double playing = tempoOf("A");
        const double other = tempoOf("B");
        const double expected = 1.0 + 10.0 * perPixel;
        std::fprintf(stdout,
                     "dj-ui: ten pixels up the pitch fader asks for %.4f and the engine plays at "
                     "%.4f (deck B at %.3f)\n", asked, playing, other);
        if (std::abs(asked - expected) > 0.001 || std::abs(playing - expected) > 0.001) {
            std::fprintf(stderr, "dj-ui: the pitch fader and the engine disagree about the rate\n");
            return 1;
        }
        if (playing <= 1.0 || std::abs(other - 1.0) > 1e-9) {
            std::fprintf(stderr, "dj-ui: the pitch moved the wrong deck or the wrong way\n");
            return 1;
        }

        // The number in the snapshot is a promise; the material has to move at that rate. Play the deck
        // and compare how far the source went with how much time passed — the check that would have
        // caught a fader whose value never reached the sound.
        {
            const auto frameOf = [&] {
                for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                    const QVariantMap map = entry.toMap();
                    if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                        return map.value(QStringLiteral("source_frame")).toDouble();
                    }
                }
                return -1.0;
            };
            bridge->play(QStringLiteral("A"), true);
            // A deck that is still loading accepts Play and does nothing; wait for material to move
            // rather than measuring the load.
            const double before = frameOf();
            QElapsedTimer start;
            start.start();
            while (start.elapsed() < 4000 && !(frameOf() > before + 100.0)) {
                pump(2);
            }
            if (!(frameOf() > before + 100.0)) {
                QString state;
                for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                    const QVariantMap map = entry.toMap();
                    if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                        continue;
                    }
                    state = QStringLiteral("load %1, playing %2, frame %3, remaining %4")
                                .arg(QJsonDocument::fromVariant(map.value(QStringLiteral("load")))
                                         .toJson(QJsonDocument::Compact),
                                     map.value(QStringLiteral("playing")).toBool() ? "yes" : "no")
                                .arg(map.value(QStringLiteral("source_frame")).toDouble(), 0, 'f', 0)
                                .arg(map.value(QStringLiteral("remaining_frames")).toDouble(), 0, 'f', 0);
                }
                std::fprintf(stderr, "dj-ui: the deck did not start playing at all (%s)\n",
                             qPrintable(state));
                return 1;
            }
            QElapsedTimer window;
            window.start();
            const double from = frameOf();
            // Long enough that the snapshot's 20 Hz publishing is a small part of the window.
            pump(240);
            const double elapsed = window.elapsed() / 1000.0;
            const double to = frameOf();
            bridge->play(QStringLiteral("A"), false);
            const double heard = (to - from) / (elapsed * 48000.0);
            std::fprintf(stdout,
                         "dj-ui: the deck consumed %.3f seconds of the file per second at %.4f\n",
                         heard, playing);
            if (elapsed < 0.5 || std::abs(heard - playing) > 0.06) {
                std::fprintf(stderr,
                             "dj-ui: the fader says %.3f but the material moved at %.3f\n",
                             playing, heard);
                return 1;
            }
        }

        // The same gesture back down, and a double click to the file's own speed.
        send(QEvent::MouseMove, grab);
        pump(20);
        if (std::abs(tempoOf("A") - 1.0) > 0.002) {
            std::fprintf(stderr, "dj-ui: the fader did not come back to the file's speed\n");
            return 1;
        }
        send(QEvent::MouseMove, grab - QPointF(0, 10));
        pump(20);
        send(QEvent::MouseButtonDblClick, grab);
        send(QEvent::MouseButtonRelease, grab);
        pump(30);
        const double reset = tempoOf("A");
        std::fprintf(stdout, "dj-ui: a double click put the pitch back at %.3f\n", reset);
        if (std::abs(reset - 1.0) > 1e-6) {
            std::fprintf(stderr, "dj-ui: a double click did not return the deck to its own speed\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(neutralOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: no engine\n");
            return 1;
        }

        const auto pump = [&](int rounds) {
            for (int step = 0; step < rounds; ++step) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
                QThread::msleep(5);
            }
        };
        // A real double click arrives as press, release, double click, release: Qt synthesises the
        // double click in the platform layer, so a check that only presses twice never sees one.
        const auto doubleClick = [&](QQuickItem *target, const char *name) {
            if (target == nullptr) {
                std::fprintf(stderr, "dj-ui: nothing to double click for %s\n", name);
                return false;
            }
            const QPointF at =
                target->mapToScene(QPointF(target->width() / 2.0, target->height() / 2.0));
            const auto send = [&](QEvent::Type type) {
                QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                                  Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
                QCoreApplication::processEvents();
            };
            send(QEvent::MouseButtonPress);
            send(QEvent::MouseButtonRelease);
            send(QEvent::MouseButtonDblClick);
            send(QEvent::MouseButtonRelease);
            pump(30);
            return true;
        };
        const auto fieldOf = [&](const char *deck, const char *field) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QString::fromLatin1(deck)) {
                    continue;
                }
                return map.value(QString::fromLatin1(field)).toDouble();
            }
            return -1.0;
        };
        const auto bandOf = [&](const char *deck, const char *band) {
            for (const QVariant &entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QString::fromLatin1(deck)) {
                    continue;
                }
                const QVariantMap eq = map.value(QStringLiteral("eq")).toMap();
                return eq.value(QString::fromLatin1(band) + QStringLiteral("_db")).toDouble();
            }
            return 999.0;
        };

        // Every control away from where it should land: a double click that is not wired would then leave
        // the old number in place and the check would say which one.
        bridge->setChannelGain(QStringLiteral("A"), 0.5);
        bridge->setChannelGain(QStringLiteral("B"), 2.0);
        bridge->setChannelEq(QStringLiteral("A"), QStringLiteral("low"), 9.0);
        bridge->setChannelEq(QStringLiteral("B"), QStringLiteral("high"), -12.0);
        bridge->setMasterGain(0.25);
        bridge->setChannelFader(QStringLiteral("A"), 0.4);
        bridge->setChannelFader(QStringLiteral("B"), 0.7);
        bridge->setCrossfader(0.15);
        pump(60);

        if (!doubleClick(findItem(window->contentItem(), QStringLiteral("trim_A")), "the gain of A")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("trim_B")), "the gain of B")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("eq_low_A")), "the low band")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("eq_high_B")), "the high band")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("masterKnob")), "the master")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("fader_A")), "the fader of A")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("fader_B")), "the fader of B")
            || !doubleClick(findItem(window->contentItem(), QStringLiteral("crossfaderHandle")),
                            "the crossfader")) {
            return 1;
        }

        const double gainA = fieldOf("A", "linear_gain");
        const double gainB = fieldOf("B", "linear_gain");
        const double lowA = bandOf("A", "low");
        const double highB = bandOf("B", "high");
        const double master = bridge->snapshot().value(QStringLiteral("master_gain")).toDouble();
        const double faderA = fieldOf("A", "fader");
        const double faderB = fieldOf("B", "fader");
        const double crossfader = bridge->snapshot().value(QStringLiteral("crossfader")).toDouble();
        // Three quarters of the level: 0.75 of full gain, which is -2.5 dB.
        const double threeQuarters = 0.75;
        std::fprintf(stdout,
                     "dj-ui: double clicks put the gains at %.3f / %.3f, the bands at %+.1f / %+.1f dB, "
                     "the master at %.3f (%.1f dB), the faders at %.3f / %.3f and the crossfader at "
                     "%.2f\n",
                     gainA, gainB, lowA, highB, master, 20.0 * std::log10(master), faderA, faderB,
                     crossfader);
        if (std::abs(gainA - 1.0) > 0.01 || std::abs(gainB - 1.0) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not trim the gain back to unity\n");
            return 1;
        }
        if (std::abs(lowA) > 0.01 || std::abs(highB) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not flatten the band\n");
            return 1;
        }
        if (std::abs(master - threeQuarters) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not stand the master at three quarters of "
                                 "its level\n");
            return 1;
        }
        if (std::abs(faderA - 1.0) > 0.01 || std::abs(faderB - 1.0) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not open a fader\n");
            return 1;
        }
        if (std::abs(crossfader - 0.5) > 0.01) {
            std::fprintf(stderr, "dj-ui: a double click did not centre the crossfader\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(spaceCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString track = parser.value(loadOption);
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        if (!track.isEmpty()) {
            bridge->load(QStringLiteral("A"), track);
        }

        const auto deckPlaying = [&]() {
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    return deck.value(QStringLiteral("playing")).toBool();
                }
            }
            return false;
        };
        QElapsedTimer waiting;
        waiting.start();
        bool playAsked = false;
        while (waiting.elapsed() < 10000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            if (!deckPlaying() && !playAsked && waiting.elapsed() > 800) {
                bridge->play(QStringLiteral("A"), true);
                playAsked = true;
            }
            if (deckPlaying()) {
                break;
            }
            QThread::msleep(20);
        }
        if (!deckPlaying()) {
            std::fprintf(stderr, "dj-ui: deck A never started playing\n");
            return 1;
        }

        // A Shortcut is not an item, so it is looked up as an object: the visual tree has no place
        // for something that draws nothing.
        auto *toggle = window->findChild<QObject *>(QStringLiteral("browserToggle"));
        auto *browserPanel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        auto *shell = findItem(window->contentItem(), QStringLiteral("globalBar"));
        if (toggle == nullptr || browserPanel == nullptr || shell == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no browser toggle\n");
            return 1;
        }
        shell = shell->parentItem();
        std::fprintf(stdout, "dj-ui: the browser key is %s\n",
                     qPrintable(toggle->property("sequence").toString()));
        if (toggle->property("sequence").toString() != QStringLiteral("Space")) {
            std::fprintf(stderr, "dj-ui: the browser is not on Space\n");
            return 1;
        }

        // The gesture itself, delivered as the window delivers keys: Space must fold the browser and
        // leave the deck playing exactly as it was.
        const auto pressSpace = [&]() {
            for (const QEvent::Type type : {QEvent::KeyPress, QEvent::KeyRelease}) {
                QKeyEvent event(type, Qt::Key_Space, Qt::NoModifier, QStringLiteral(" "));
                QCoreApplication::sendEvent(window, &event);
            }
            QCoreApplication::processEvents();
            QThread::msleep(100);
            QCoreApplication::processEvents();
        };

        const bool before = deckPlaying();
        pressSpace();
        const bool folded = !browserPanel->isVisible();
        pressSpace();
        const bool unfolded = browserPanel->isVisible();
        const bool after = deckPlaying();
        std::fprintf(stdout, "dj-ui: deck A playing before %s, after %s; folded %s, unfolded %s\n",
                     before ? "yes" : "no", after ? "yes" : "no", folded ? "yes" : "no",
                     unfolded ? "yes" : "no");
        if (before != after) {
            std::fprintf(stderr, "dj-ui: Space still reaches playback\n");
            return 1;
        }
        if (!folded || !unfolded) {
            std::fprintf(stderr, "dj-ui: Space does not fold the browser\n");
            return 1;
        }
        bridge->play(QStringLiteral("A"), false);
        std::fprintf(stdout, "dj-ui: Space leaves playback alone and folds the browser\n");
        return 0;
    }

    if (parser.isSet(refusalCheckOption)) {
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString track = parser.value(loadOption);
        if (track.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --refusal-check needs --load\n");
            return 2;
        }
        // The shell connects at the end of main; this mode returns before that.
        bridge->connectToEngine(parser.value(connectOption));
        QElapsedTimer linking;
        linking.start();
        while (!bridge->isConnected() && linking.elapsed() < 3000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        bridge->load(QStringLiteral("A"), track);

        bool playing = false;
        QElapsedTimer waiting;
        waiting.start();
        while (!playing && waiting.elapsed() < 10000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant &entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    playing = deck.value(QStringLiteral("playing")).toBool();
                }
            }
            if (!playing && waiting.elapsed() > 1000) {
                bridge->play(QStringLiteral("A"), true);
            }
            QThread::msleep(20);
        }
        if (!playing) {
            std::fprintf(stderr, "dj-ui: deck A never started playing\n");
            return 1;
        }

        // The browser, pointed at the track's own folder with the track selected.
        browser.open(QFileInfo(track).absolutePath());
        QCoreApplication::processEvents();
        auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        auto *list = findItem(window->contentItem(), QStringLiteral("browserList"));
        if (panel == nullptr || list == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no browser\n");
            return 1;
        }
        int fileIndex = -1;
        const QVariantList entries = browser.entries();
        for (int index = 0; index < entries.size(); ++index) {
            if (entries.at(index).toMap().value(QStringLiteral("path")).toString() == track) {
                fileIndex = index;
            }
        }
        if (fileIndex < 0) {
            std::fprintf(stderr, "dj-ui: the browser does not list the track\n");
            return 1;
        }
        list->setProperty("currentIndex", fileIndex);
        QVariant refused;
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, refused),
                                  Q_ARG(QVariant, QStringLiteral("A")));
        QCoreApplication::processEvents();

        auto *deckButton = findItem(window->contentItem(), QStringLiteral("deckButton_A"));
        if (deckButton == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no deck buttons\n");
            return 1;
        }
        // A playing deck's button is blue before anything is pressed: the browser tells the truth
        // about the decks it feeds.
        const QColor accent(settings.themeAccent());
        const auto faceOf = [](QQuickItem *button) {
            return button->property("faceColor").value<QColor>();
        };
        std::fprintf(stdout, "dj-ui: deck A is playing, its browser button is %s\n",
                     qPrintable(faceOf(deckButton).name()));
        if (faceOf(deckButton) != accent) {
            std::fprintf(stderr, "dj-ui: a playing deck's button is not marked as playing\n");
            return 1;
        }

        // Pressing the button is the way a person finds out; it has to blink.
        QMetaObject::invokeMethod(deckButton, "clicked");
        QCoreApplication::processEvents();
        const QColor whileFlashing = faceOf(deckButton);
        const bool flashingNow = deckButton->property("flashing").toBool();
        if (whileFlashing != QColor(settings.themeDanger())) {
            std::fprintf(stderr, "dj-ui: the refusal did not turn the button red\n");
            return 1;
        }
        // ... and it has to come back to blue, not to grey, once the blink is over.
        QElapsedTimer blink;
        blink.start();
        while (blink.elapsed() < 1200) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 30);
            QThread::msleep(20);
        }
        const QColor afterBlink = faceOf(deckButton);
        std::fprintf(stdout, "dj-ui: after the blink the button is %s\n",
                     qPrintable(afterBlink.name()));
        if (afterBlink != accent) {
            std::fprintf(stderr, "dj-ui: the button did not return to its playing colour\n");
            return 1;
        }
        const int buttonRefusals = deckButton->property("refusals").toInt();
        const bool flashing = flashingNow;
        std::fprintf(stdout, "dj-ui: refused load: deck button %d, flashing %s\n", buttonRefusals,
                     flashing ? "yes" : "no");
        if (!refused.toBool() && buttonRefusals > 0 && flashing) {
            bridge->play(QStringLiteral("A"), false);
            std::fprintf(stdout, "dj-ui: a load into a playing deck is refused and the button blinks\n");
            return 0;
        }
        std::fprintf(stderr, "dj-ui: the refusal was not signalled\n");
        return 1;
    }

    if (parser.isSet(stripCheckOption)) {
        // The picture is the thing the user sees, so it gets its own measurement: the envelope the
        // engine publishes has to reach the strip that draws it.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();

        // The shell connects at the end of main; this mode returns before that, so the link is made
        // here or the check would measure a UI that never heard from the engine.
        if (parser.isSet(connectOption)) {
            bridge->connectToEngine(parser.value(connectOption));
            QElapsedTimer linking;
            linking.start();
            while (!bridge->isConnected() && linking.elapsed() < 3000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                QThread::msleep(10);
            }
            std::fprintf(stdout, "dj-ui: strip check link %s\n",
                         bridge->isConnected() ? "up" : "down");
        }

        auto *detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
        if (detail == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no strip\n");
            return 1;
        }
        auto *envelope = qobject_cast<QObject *>(detail->property("envelope").value<QObject *>());
        if (envelope == nullptr) {
            std::fprintf(stderr, "dj-ui: the strip has no envelope\n");
            return 1;
        }
        const QString track = parser.value(loadOption);
        if (!track.isEmpty() && bridge->isConnected()) {
            bridge->load(QStringLiteral("A"), track);
            std::fprintf(stdout, "dj-ui: asked deck A for %s\n", qPrintable(track));
        }

        QElapsedTimer waited;
        waited.start();
        int buckets = 0;
        while (waited.elapsed() < 15000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
            buckets = envelope->property("buckets").toInt();
            if (buckets > 0) {
                break;
            }
            QThread::msleep(20);
        }
        const QString error = envelope->property("error").toString();
        std::fprintf(stdout, "dj-ui: strip A holds %d columns after %lld ms%s%s\n", buckets,
                     static_cast<long long>(waited.elapsed()),
                     error.isEmpty() ? "" : ", error: ", qPrintable(error));
        if (buckets <= 0) {
            std::fprintf(stderr, "dj-ui: the strip never received an envelope\n");
            return 1;
        }
        return 0;
    }

    if (parser.isSet(browserStartOption)) {
        // Where the browser opens matters to a working DJ more than any pixel: it is the difference
        // between two clicks and a walk through the file system.
        const QString sessionFile = parser.value(sessionFileOption);
        if (sessionFile.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --browser-start-check needs --session-file\n");
            return 2;
        }
        QTemporaryDir first;
        QTemporaryDir second;
        if (!first.isValid() || !second.isValid()) {
            std::fprintf(stderr, "dj-ui: cannot make temporary folders\n");
            return 1;
        }
        for (const QString &folder : {first.path(), second.path()}) {
            QFile file(folder + QStringLiteral("/track.wav"));
            if (!file.open(QIODevice::WriteOnly)) {
                std::fprintf(stderr, "dj-ui: cannot write into %s\n", qPrintable(folder));
                return 1;
            }
            file.write("RIFF", 4);
        }
        QJsonObject remembered;
        remembered.insert(QStringLiteral("folder"), first.path());
        QFile state(sessionFile);
        if (!state.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(sessionFile));
            return 1;
        }
        state.write(QJsonDocument(remembered).toJson());
        state.close();

        // The check needs its own run of the startup path, so the file it wrote is read now.
        SessionState reloaded;
        reloaded.setFile(sessionFile);
        if (!reloaded.load()) {
            std::fprintf(stderr, "dj-ui: the session file was not readable\n");
            return 1;
        }
        browser.open(reloaded.folder());
        std::fprintf(stdout, "dj-ui: browser opened %s (remembered)\n", qPrintable(browser.folder()));
        if (browser.folder() != first.path()) {
            std::fprintf(stderr, "dj-ui: the browser did not open the remembered folder\n");
            return 1;
        }

        // A load elsewhere has to move both the browser and the file.
        const QString elsewhere = second.path() + QStringLiteral("/track.wav");
        bridge->load(QStringLiteral("A"), elsewhere);
        std::fprintf(stdout, "dj-ui: browser moved to %s after a load\n", qPrintable(browser.folder()));
        if (browser.folder() != second.path()) {
            std::fprintf(stderr, "dj-ui: a load did not move the browser\n");
            return 1;
        }
        QFile written(sessionFile);
        if (!written.open(QIODevice::ReadOnly)) {
            std::fprintf(stderr, "dj-ui: the session file vanished\n");
            return 1;
        }
        const QString stored =
            QJsonDocument::fromJson(written.readAll()).object().value(QStringLiteral("folder")).toString();
        if (stored != second.path()) {
            std::fprintf(stderr, "dj-ui: the session file still says %s\n", qPrintable(stored));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the folder survives a restart and follows the last load\n");
        return 0;
    }

    if (parser.isSet(browserRestoreOption)) {
        // The second start: a new process, a layout file left by the first one, and no other hint. What
        // the panel and the deck hold now is what a person would see, not what the file says.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        // The splitter has not laid anything out until the scene has had a frame: reading geometry
        // before that measures the minimum sizes, not the settings.
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();
        QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        QQuickItem *detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
        if (panel == nullptr || detail == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no browser panel or no deck waveform\n");
            return 1;
        }
        QQuickItem *shellItem = findItem(window->contentItem(), QStringLiteral("shellLayout"));
        if (shellItem == nullptr || shellItem->height() <= 0.0) {
            std::fprintf(stderr, "dj-ui: the shell has no height to divide\n");
            return 1;
        }
        const int rail = panel->property("railWidth").toInt();
        const int font = panel->property("listFont").toInt();
        const double span = detail->property("zoomSpan").toDouble();
        const double heightShare = panel->height() / shellItem->height() * 100.0;
        if (qAbs(heightShare - layout.browserPercent()) > 2.0) {
            std::fprintf(stderr,
                         "dj-ui: the browser came back %0.1f%% tall instead of %0.1f%%\n",
                         heightShare, layout.browserPercent());
            return 1;
        }
        if (rail != layout.browserRailWidth() || font != layout.browserFont()
            || qAbs(span - layout.waveformSpan()) > 1e-6) {
            std::fprintf(stderr,
                         "dj-ui: the shell did not come back as it was left (rail %d of %d, font %d "
                         "of %d, span %.3f of %.3f)\n",
                         rail, layout.browserRailWidth(), font, layout.browserFont(), span,
                         layout.waveformSpan());
            return 1;
        }
        std::fprintf(stdout,
                     "dj-ui: a new run opens with the rail at %d px, the browser text at %d px, the "
                     "browser %0.1f%% tall and the deck zoom at a span of %s, the numbers the last run "
                     "left\n",
                     rail, font, heightShare, qPrintable(QString::number(span, 'f', 3)));
        return 0;
    }

    if (parser.isSet(browserCheckOption)) {
        // A list you cannot pick from is a picture of a list, so the check goes through the same
        // function the mouse does: select a track, activate it, read what the panel says happened.
        QTemporaryDir folder;
        if (!folder.isValid()) {
            std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
            return 1;
        }
        QDir(folder.path()).mkpath(QStringLiteral("subfolder"));
        const QString track = folder.path() + QStringLiteral("/track.wav");
        QFile file(track);
        if (!file.open(QIODevice::WriteOnly)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(track));
            return 1;
        }
        file.write("RIFF", 4);
        file.close();
        browser.open(folder.path());

        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();

        QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        QQuickItem *list = findItem(window->contentItem(), QStringLiteral("browserList"));
        if (panel == nullptr || list == nullptr) {
            std::fprintf(stderr, "dj-ui: the shell has no browser\n");
            return 1;
        }
        const QVariantList entries = browser.entries();
        int fileIndex = -1;
        int folderIndex = -1;
        for (int index = 0; index < entries.size(); ++index) {
            const QVariantMap entry = entries.at(index).toMap();
            if (entry.value(QStringLiteral("dir")).toBool()) {
                folderIndex = index;
            } else if (fileIndex < 0) {
                fileIndex = index;
            }
        }
        if (fileIndex < 0 || folderIndex < 0) {
            std::fprintf(stderr, "dj-ui: the browser did not list the track and the folder\n");
            return 1;
        }

        const auto invoke = [&](const char *method, int index) {
            list->setProperty("currentIndex", index);
            QVariant returned;
            QMetaObject::invokeMethod(panel, method, Q_RETURN_ARG(QVariant, returned),
                                      Q_ARG(QVariant, index));
            return panel->property("lastAction").toString();
        };
        const auto loadInto = [&](const QString &deck) {
            list->setProperty("currentIndex", fileIndex);
            QVariant returned;
            QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, returned),
                                      Q_ARG(QVariant, deck));
            return panel->property("lastAction").toString();
        };

        const QString onTrack = loadInto(QStringLiteral("A"));
        // The selection has to hold too: a highlight that never lights is the same bug as a click
        // that never lands. Checked before the folder, because opening a folder reloads the model
        // and a fresh list correctly starts with nothing selected.
        const int selected = list->property("currentIndex").toInt();
        const QString onFolder = invoke("openFolder", folderIndex);
        std::fprintf(stdout, "dj-ui: browser track  -> %s\n", qPrintable(onTrack));
        std::fprintf(stdout, "dj-ui: browser folder -> %s\n", qPrintable(onFolder));
        if (!onTrack.startsWith(QStringLiteral("loaded ")) || !onTrack.endsWith(QStringLiteral("into A"))) {
            std::fprintf(stderr, "dj-ui: a track click did not load into the chosen deck\n");
            return 1;
        }
        if (!onFolder.startsWith(QStringLiteral("opened "))) {
            std::fprintf(stderr, "dj-ui: a folder click did not open the folder\n");
            return 1;
        }
        if (selected != fileIndex) {
            std::fprintf(stderr, "dj-ui: the selection did not follow the click (%d, wanted %d)\n",
                         selected, fileIndex);
            return 1;
        }
        // The click itself, delivered to the window the way a mouse delivers it: calling the action
        // proves the panel works, sending the event proves the row is reachable.
        browser.open(folder.path());
        QCoreApplication::processEvents();
        const QPointF rowPoint =
            list->mapToScene(QPointF(20.0, fileIndex * 18.0 + 9.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, rowPoint, window->mapToGlobal(rowPoint.toPoint()),
                              Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        QCoreApplication::processEvents();
        const int clicked = list->property("currentIndex").toInt();
        std::fprintf(stdout, "dj-ui: browser click at row %d selected %d\n", fileIndex, clicked);
        if (clicked != fileIndex) {
            std::fprintf(stderr, "dj-ui: a click on the browser row did not select it\n");
            return 1;
        }
        // A double click must not load: it is the gesture that ruins a transition when the hand
        // slips. Verified with real events at the row, not by reading the QML.
        browser.open(folder.path());
        QCoreApplication::processEvents();
        panel->setProperty("lastAction", QString());
        const QPointF rowCentre = list->mapToScene(QPointF(20.0, fileIndex * 18.0 + 9.0));
        for (int repeat = 0; repeat < 2; ++repeat) {
            for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
                QMouseEvent event(type, rowCentre, window->mapToGlobal(rowCentre.toPoint()),
                                  Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
            }
        }
        QCoreApplication::processEvents();
        const QString afterDoubleClick = panel->property("lastAction").toString();
        std::fprintf(stdout, "dj-ui: browser double click -> %s\n",
                     afterDoubleClick.isEmpty() ? "nothing" : qPrintable(afterDoubleClick));
        if (afterDoubleClick.startsWith(QStringLiteral("loaded "))) {
            std::fprintf(stderr, "dj-ui: a double click loaded a track\n");
            return 1;
        }

        // A deck button is also a load button when something is chosen.
        list->setProperty("currentIndex", fileIndex);
        auto *buttonB = findItem(window->contentItem(), QStringLiteral("deckButton_B"));
        if (buttonB == nullptr) {
            std::fprintf(stderr, "dj-ui: the browser has no deck buttons\n");
            return 1;
        }
        QMetaObject::invokeMethod(buttonB, "clicked");
        QCoreApplication::processEvents();
        const QString onDeckButton = panel->property("lastAction").toString();
        std::fprintf(stdout, "dj-ui: browser deck B button -> %s\n", qPrintable(onDeckButton));
        if (!onDeckButton.startsWith(QStringLiteral("loaded ")) || !onDeckButton.endsWith(QStringLiteral("into B"))) {
            std::fprintf(stderr, "dj-ui: a deck button did not load the chosen track\n");
            return 1;
        }
        // The way up is a row, not a button: it is first, it is called "..", and it goes up.
        const QVariantList rows = browser.entries();
        if (rows.isEmpty() || !rows.first().toMap().value(QStringLiteral("parent")).toBool()
            || rows.first().toMap().value(QStringLiteral("name")).toString() != QStringLiteral("..")) {
            std::fprintf(stderr, "dj-ui: the browser has no \"..\" row to go up with\n");
            return 1;
        }
        const QString inside = browser.folder();
        QDir up(inside);
        const bool hasParent = up.cdUp();
        QVariant opened;
        QMetaObject::invokeMethod(panel, "openFolder", Q_RETURN_ARG(QVariant, opened),
                                  Q_ARG(QVariant, 0));
        QCoreApplication::processEvents();
        if (hasParent && browser.folder() != up.absolutePath()) {
            std::fprintf(stderr, "dj-ui: \"..\" did not go up (%s, wanted %s)\n",
                         qPrintable(browser.folder()), qPrintable(up.absolutePath()));
            return 1;
        }

        // The text size is a control in the browser, and the row grows with it.
        const int small = panel->property("rowHeight").toInt();
        QQuickItem *plus = panel->findChild<QQuickItem *>(QStringLiteral("fontPlus"));
        QQuickItem *minus = panel->findChild<QQuickItem *>(QStringLiteral("fontMinus"));
        if (plus == nullptr || minus == nullptr) {
            std::fprintf(stderr, "dj-ui: the browser has no text-size control\n");
            return 1;
        }
        QMetaObject::invokeMethod(plus, "clicked");
        QCoreApplication::processEvents();
        const int bigger = panel->property("rowHeight").toInt();
        QMetaObject::invokeMethod(minus, "clicked");
        QCoreApplication::processEvents();
        const int back = panel->property("rowHeight").toInt();
        if (bigger != small + 1 || back != small) {
            std::fprintf(stderr, "dj-ui: the text size did not follow the buttons (%d, %d, %d)\n",
                         small, bigger, back);
            return 1;
        }
        // The rail's width is adjustable, and the divider is the only thing that adjusts it.
        QQuickItem *rail = panel->findChild<QQuickItem *>(QStringLiteral("browserSections"));
        QQuickItem *splitterItem = panel->findChild<QQuickItem *>(QStringLiteral("browserRailSplitter"));
        if (rail == nullptr || splitterItem == nullptr) {
            std::fprintf(stderr, "dj-ui: the browser has no sections rail or no divider\n");
            return 1;
        }
        // A layout re-polishes on the next frame: the check waits for the width instead of reading a
        // value that may not have been laid out yet.
        const auto railWidthBecomes = [&](int want) {
            for (int attempt = 0; attempt < 60; ++attempt) {
                QCoreApplication::processEvents();
                QThread::msleep(10);
                if (qAbs(rail->width() - static_cast<qreal>(want)) < 2.0) {
                    return true;
                }
            }
            return false;
        };
        const qreal railBefore = rail->width();
        panel->setProperty("railWidth", 140);
        const bool grew = railWidthBecomes(140);
        const qreal railAfter = rail->width();
        panel->setProperty("railWidth", 92);
        const bool shrank = railWidthBecomes(92);
        const qreal railBack = rail->width();
        if (!grew || !shrank || !(railAfter > railBefore + 20.0 && qAbs(railBack - railBefore) < 2.0)) {
            std::fprintf(stderr, "dj-ui: the rail width did not follow the setting (%.0f, %.0f, "
                                 "%.0f)\n",
                         railBefore, railAfter, railBack);
            return 1;
        }

        // The section text is the same type as the list: one control for the browser, not two.
        // The section text is the same type as the list: one control for the browser, not two.
        // Measured on the label the rail actually renders, not on the property that feeds it.
        // Delegates of a Repeater are item children of the rail but not QObject children of the
        // panel, so the search walks the item tree (findItem) instead of findChild.
        QQuickItem *sectionRow = findItem(panel, QStringLiteral("section_files"));
        if (sectionRow == nullptr || sectionRow->childItems().isEmpty()) {
            std::fprintf(stderr, "dj-ui: the sections rail has no label to measure\n");
            return 1;
        }
        QQuickItem *sectionLabel = sectionRow->childItems().first();
        const int sectionBefore = sectionLabel->property("font").value<QFont>().pixelSize();
        QMetaObject::invokeMethod(plus, "clicked");
        QCoreApplication::processEvents();
        const int sectionAfter = sectionLabel->property("font").value<QFont>().pixelSize();
        QMetaObject::invokeMethod(minus, "clicked");
        QCoreApplication::processEvents();
        const int sectionBack = sectionLabel->property("font").value<QFont>().pixelSize();
        if (sectionAfter != sectionBefore + 1 || sectionBack != sectionBefore) {
            std::fprintf(stderr, "dj-ui: the section text did not follow the preset (%d, %d, %d)\n",
                         sectionBefore, sectionAfter, sectionBack);
            return 1;
        }

        // The height of the browser is a hand's decision too. A drop writes percentages, and the two
        // numbers must not move when the type changes size: a panel that snaps back the moment the
        // font grows is a panel that never remembers how tall it was.
        QQuickItem *shellItem = findItem(window->contentItem(), QStringLiteral("shellLayout"));
        if (shellItem == nullptr || shellItem->height() <= 0.0) {
            std::fprintf(stderr, "dj-ui: the shell has no height to divide\n");
            return 1;
        }
        const double shellHeight = shellItem->height();
        panel->setHeight(shellHeight * 0.45);
        QMetaObject::invokeMethod(shellItem, "publishRowSizes");
        QCoreApplication::processEvents();
        const int browserPercent = qRound(layout.browserPercent());
        if (layout.preset() != QStringLiteral("custom") || qAbs(browserPercent - 45) > 2) {
            std::fprintf(stderr,
                         "dj-ui: dropping the browser splitter did not reach the settings (preset "
                         "\"%s\", browser %d%%)\n",
                         qPrintable(layout.preset()), browserPercent);
            return 1;
        }
        const double browserBeforeFont = panel->height();
        QMetaObject::invokeMethod(plus, "clicked");
        QCoreApplication::processEvents();
        const double browserAfterFont = panel->height();
        if (qAbs(browserAfterFont - browserBeforeFont) > 0.02 * shellHeight) {
            std::fprintf(stderr,
                         "dj-ui: the browser changed height when only the text grew (%.0f → %.0f of "
                         "%.0f)\n",
                         browserBeforeFont, browserAfterFont, shellHeight);
            return 1;
        }

        // The view numbers outlive the run: they are written to the layout file and read back by a
        // fresh instance, which is exactly what the next start does (DJP-UI-001).
        panel->setProperty("railWidth", 130);
        panel->setProperty("listFont", 14);
        QQuickItem *detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
        if (detail == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck has no main waveform to zoom\n");
            return 1;
        }
        detail->setProperty("zoomSpan", 0.25);
        QCoreApplication::processEvents();
        if (!layout.save()) {
            std::fprintf(stderr, "dj-ui: the layout file could not be written (%s)\n",
                         qPrintable(layout.file()));
            return 1;
        }
        LayoutSettings reloaded;
        reloaded.setFile(layout.file());
        if (!reloaded.load()) {
            std::fprintf(stderr, "dj-ui: the layout file could not be read back\n");
            return 1;
        }
        if (reloaded.browserRailWidth() != 130 || reloaded.browserFont() != 14
            || qAbs(reloaded.waveformSpan() - 0.25) > 1e-9) {
            std::fprintf(stderr,
                         "dj-ui: the view did not come back as it was left (rail %d, font %d, span "
                         "%.3f)\n",
                         reloaded.browserRailWidth(), reloaded.browserFont(),
                         reloaded.waveformSpan());
            return 1;
        }

        std::fprintf(stdout,
                     "dj-ui: browser selects and acts, selection follows the click, \"..\" goes up, "
                     "the text size moves the row and the sections (%d px), the rail is %.0f px wide, "
                     "the browser keeps %d%% of the window when the type grows, and the rail width, the "
                     "font and the deck zoom come back after a restart (130 px, 14 px, 0.25)\n",
                     small, railAfter, browserPercent);
        return 0;
    }

    if (parser.isSet(themeCheckOption) || parser.isSet(screenshotOption)) {
        // A window that never reaches a screen still has to look right: grabbing it here, from the
        // engine the shell actually uses, is how the theme gets measured instead of described.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        QEventLoop settle;
        QTimer::singleShot(600, &settle, &QEventLoop::quit);
        settle.exec();
        const QImage image = window->grabWindow();
        if (parser.isSet(screenshotOption)) {
            const QString target = parser.value(screenshotOption);
            if (!image.save(target)) {
                std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(target));
                return 1;
            }
        }
        // The theme, measured: the reference interface is dark greys with one blue accent, and a
        // regression to the light default shows up as a majority of bright pixels, not as a missing
        // property.
        QMap<QString, int> counts;
        int bright = 0;
        int total = 0;
        for (int y = 0; y < image.height(); y += 4) {
            for (int x = 0; x < image.width(); x += 4) {
                const QColor pixel = image.pixelColor(x, y);
                const int luma = (pixel.red() * 299 + pixel.green() * 587 + pixel.blue() * 114) / 1000;
                if (luma > 128) {
                    ++bright;
                }
                ++total;
                counts[pixel.name()] += 1;
            }
        }
        if (parser.isSet(screenshotOption)) {
            std::fprintf(stdout, "dj-ui: wrote %s (%dx%d)\n",
                         qPrintable(parser.value(screenshotOption)), image.width(), image.height());
        }
        std::fprintf(stdout, "dj-ui: theme: %.1f%% bright pixels, dominant colours:",
                     100.0 * bright / qMax(1, total));
        QList<QPair<int, QString>> ranked;
        for (auto it = counts.cbegin(); it != counts.cend(); ++it) {
            ranked.append({it.value(), it.key()});
        }
        std::sort(ranked.begin(), ranked.end(), [](const auto &left, const auto &right) {
            return left.first > right.first;
        });
        for (int index = 0; index < ranked.size() && index < 4; ++index) {
            std::fprintf(stdout, " %s %.0f%%", qPrintable(ranked.at(index).second),
                         100.0 * ranked.at(index).first / qMax(1, total));
        }
        std::fprintf(stdout, "\n");
        if (bright * 4 > total) {
            std::fprintf(stderr, "dj-ui: the shell is light: %.0f%% of its pixels are bright\n",
                         100.0 * bright / qMax(1, total));
            return 1;
        }
        return 0;
    }

    if (parser.isSet(smokeOption)) {
        if (g_warningCount != 0) {
            std::fprintf(stderr, "dj-ui: %d Qt warning(s) while loading the scene\n", g_warningCount);
            return 1;
        }
        std::fprintf(stdout, "dj-ui: QML scene loaded, no Qt warnings\n");
        return 0;
    }

    const QString path = parser.isSet(connectOption) ? parser.value(connectOption)
                                                     : defaultSocketPath();
    bridge->connectToEngine(path);
    std::fprintf(stdout, "dj-ui: connecting to %s\n", qPrintable(path));

    if (parser.isSet(exitAfterOption)) {
        bool ok = false;
        const int after = parser.value(exitAfterOption).toInt(&ok);
        if (!ok || after < 0) {
            std::fprintf(stderr, "dj-ui: --exit-after needs milliseconds\n");
            return 2;
        }
        // The shutdown path, exercised instead of described: close the window the way a person does.
        QTimer::singleShot(after, qApp, &QCoreApplication::quit);
    }

    return app.exec();
}
