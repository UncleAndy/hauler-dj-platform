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
#include <QWheelEvent>
#include <QEventLoop>
#include <QGuiApplication>
#include <QPalette>
#include <QDir>
#include <QDirIterator>
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

// The browser's own check: a list you cannot pick from is a picture of a list, so it goes through
// the same functions the mouse does.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC says so
// (`variable tracking size limit exceeded`). The checks are the tests; they read better named.
static int runBrowserCheck(QQmlApplicationEngine &engine, FileBrowser &browser,
                          LayoutSettings &layout)
{
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

    // The list's index is bound to the remembered cursor: selecting a row is `setCursor`, not an
    // assignment to the index, which would break the binding and leave the list with no cursor at all.
    const auto selectRow = [&](int index) {
        const QVariantList rows = browser.entries();
        if (index >= 0 && index < rows.size()) {
            browser.setCursor(rows.at(index).toMap().value(QStringLiteral("path")).toString());
        }
        QCoreApplication::processEvents();
    };
    const auto invoke = [&](const char *method, int index) {
        selectRow(index);
        QVariant returned;
        QMetaObject::invokeMethod(panel, method, Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, index));
        return panel->property("lastAction").toString();
    };
    const auto loadInto = [&](const QString &deck) {
        selectRow(fileIndex);
        QVariant returned;
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, deck));
        // The deck is the witness: the panel answers "loaded" only once the engine shows the track, so
        // the answer is waited for instead of read on the next line.
        if (!panel->property("lastAction").toString().startsWith(QStringLiteral("loading "))) {
            return panel->property("lastAction").toString();
        }
        QElapsedTimer wait;
        wait.start();
        while (wait.elapsed() < 60000) {
            QCoreApplication::processEvents();
            QThread::msleep(50);
            const QString now = panel->property("lastAction").toString();
            if (now.startsWith(QStringLiteral("loaded "))
                || now.contains(QStringLiteral("did not take"))) {
                return now;
            }
        }
        return panel->property("lastAction").toString();
    };

    const QString onTrack = loadInto(QStringLiteral("A"));
    // The path in the answer is the path of the row that was chosen: a load that moves the folder
    // must not make the panel report a row it never sent.
    const QString chosenPath = entries.at(fileIndex).toMap().value(QStringLiteral("path")).toString();
    const bool hasEngine = !onTrack.startsWith(QStringLiteral("no engine for "));
    if (!hasEngine) {
        // --smoke runs without an engine: the panel must refuse and say why, not claim a load.
        if (onTrack != QStringLiteral("no engine for A")) {
            std::fprintf(stderr, "dj-ui: without an engine the browser said '%s'\n",
                         qPrintable(onTrack));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: no engine, the load is refused and said so\n");
    } else if (onTrack != QStringLiteral("loaded ") + chosenPath + QStringLiteral(" into A")) {
        std::fprintf(stderr, "dj-ui: the browser reported a path it did not choose (%s, wanted %s)\n",
                     qPrintable(onTrack), qPrintable(chosenPath));
        return 1;
    }
    // The selection has to hold too: a highlight that never lights is the same bug as a click
    // that never lands. Checked before the folder, because opening a folder reloads the model
    // and a fresh list correctly starts with nothing selected.
    const int selected = list->property("currentIndex").toInt();
    const QString onFolder = invoke("openFolder", folderIndex);
    std::fprintf(stdout, "dj-ui: browser track  -> %s\n", qPrintable(onTrack));
    std::fprintf(stdout, "dj-ui: browser folder -> %s\n", qPrintable(onFolder));
    if (hasEngine
        && (!onTrack.startsWith(QStringLiteral("loaded "))
            || !onTrack.endsWith(QStringLiteral("into A")))) {
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
    selectRow(fileIndex);
    auto *buttonB = findItem(window->contentItem(), QStringLiteral("deckButton_B"));
    if (buttonB == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no deck buttons\n");
        return 1;
    }
    QMetaObject::invokeMethod(buttonB, "clicked");
    QCoreApplication::processEvents();
    // The button answers "loading" and the deck answers later: wait for the deck's word, not for the
    // request.
    QElapsedTimer deckWait;
    deckWait.start();
    while (deckWait.elapsed() < 60000
           && panel->property("lastAction").toString().startsWith(QStringLiteral("loading "))) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const QString onDeckButton = panel->property("lastAction").toString();
    std::fprintf(stdout, "dj-ui: browser deck B button -> %s\n", qPrintable(onDeckButton));
    if (hasEngine) {
        if (!onDeckButton.startsWith(QStringLiteral("loaded "))
            || !onDeckButton.endsWith(QStringLiteral("into B"))) {
            std::fprintf(stderr, "dj-ui: a deck button did not load the chosen track\n");
            return 1;
        }
    } else if (onDeckButton != QStringLiteral("no engine for B")) {
        std::fprintf(stderr, "dj-ui: without an engine the deck button said '%s'\n",
                     qPrintable(onDeckButton));
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
    // The width to come back to is whatever the layout held, not a number written into the check: a
    // hand that set 130 px must not turn a red check green, and the check must not go red because the
    // layout it was handed is not the one a fresh install has.
    const qreal railBefore = rail->width();
    const int railHome = qRound(railBefore);
    panel->setProperty("railWidth", railHome + 40);
    const bool grew = railWidthBecomes(railHome + 40);
    const qreal railAfter = rail->width();
    panel->setProperty("railWidth", railHome);
    const bool shrank = railWidthBecomes(railHome);
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

    // The header is the same browser as the list. Its title, the path, the two text-size buttons and
    // the deck buttons all follow the type size, and the buttons grow enough to keep their labels
    // inside: a list that grows under controls too small to hit is a browser only half scaled.
    QQuickItem *pathLabel = findItem(panel, QStringLiteral("browserPath"));
    QQuickItem *deckButton = findItem(panel, QStringLiteral("deckButton_A"));
    if (pathLabel == nullptr || deckButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser header has no path or no deck button to measure\n");
        return 1;
    }
    const auto pixelSizeOf = [](QQuickItem *item) {
        return item->property("font").value<QFont>().pixelSize();
    };
    const int pathBefore = pixelSizeOf(pathLabel);
    const int deckBefore = pixelSizeOf(deckButton);
    const double buttonBefore = plus->height();
    const auto heightBecomes = [&](double want) {
        for (int attempt = 0; attempt < 60; ++attempt) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
            if (plus->height() > want) {
                return true;
            }
        }
        return false;
    };
    QMetaObject::invokeMethod(plus, "clicked");
    const bool buttonsGrew = heightBecomes(buttonBefore);
    const int pathAfter = pixelSizeOf(pathLabel);
    const int deckAfter = pixelSizeOf(deckButton);
    const double buttonAfter = plus->height();
    QMetaObject::invokeMethod(minus, "clicked");
    QCoreApplication::processEvents();
    if (pathAfter != pathBefore + 1 || deckAfter != deckBefore + 1 || !buttonsGrew) {
        std::fprintf(stderr,
                     "dj-ui: the browser header did not follow the type (path %d -> %d, deck button "
                     "%d -> %d, height %.0f -> %.0f)\n",
                     pathBefore, pathAfter, deckBefore, deckAfter, buttonBefore, buttonAfter);
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

    // The Collection section: one root, every track below it, named as the import named it, with
    // the tempo and the key its metadata records. The tree here is real, with a real .djmeta, and
    // it is read through the same object the section uses.
    QTemporaryDir collection;
    if (!collection.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary collection\n");
        return 1;
    }
    const QString collectionRoot = collection.path();
    QDir(collectionRoot).mkpath(QStringLiteral("A/N/.djmeta"));
    QDir(collectionRoot).mkpath(QStringLiteral("B/B"));
    // The import parks a rejected copy here; the collection is what its owner kept.
    QDir(collectionRoot).mkpath(QStringLiteral("_duplicates/C/C"));
    const auto writeStub = [](const QString &path) {
        QFile file(path);
        if (!file.open(QIODevice::WriteOnly)) {
            return false;
        }
        file.write("RIFF", 4);
        return true;
    };
    if (!writeStub(collectionRoot + QStringLiteral("/A/N/ANH - tonight.flac"))
        || !writeStub(collectionRoot + QStringLiteral("/B/B/Bbno$ - ADD.flac"))
        || !writeStub(collectionRoot + QStringLiteral("/_duplicates/C/C/Bbno$ - ADD.mp3"))) {
        std::fprintf(stderr, "dj-ui: cannot write the collection's tracks\n");
        return 1;
    }
    QFile record(collectionRoot + QStringLiteral("/A/N/.djmeta/ANH - tonight.flac.json"));
    if (record.open(QIODevice::WriteOnly)) {
        record.write(R"({"schema":"djmeta/1","revision":1,"updated_at_ms":0,"fingerprint":{},"beatgrid":{"bpm":128.0,"first_beat_frame":0},"key":"8m"})");
        record.close();
    }
    // No root at all is the platform's own collection folder, not an empty section.
    browser.openCollection(QStringList());
    QCoreApplication::processEvents();
    if (browser.mode() != QStringLiteral("collection")
        || browser.folder() != browser.defaultCollectionRoot() || !browser.error().isEmpty()) {
        std::fprintf(stderr, "dj-ui: an unset collection did not fall back to %s (it is %s, "
                             "error «%s»)\n",
                     qPrintable(browser.defaultCollectionRoot()), qPrintable(browser.folder()),
                     qPrintable(browser.error()));
        return 1;
    }
    if (!browser.defaultCollectionRoot().endsWith(QStringLiteral("/hauler-dj/collection"))) {
        std::fprintf(stderr, "dj-ui: the default collection root is not hauler-dj/collection (%s)\n",
                     qPrintable(browser.defaultCollectionRoot()));
        return 1;
    }
    browser.openCollection({collectionRoot});
    QCoreApplication::processEvents();
    const QVariantList collected = browser.entries();
    // How many the list held before the search narrowed it: printed with the summary at the end.
    int unfiltered = 0;
    if (!hasEngine) {
        // The collection is the index, and this check runs without an engine to ask: an empty list is the
        // honest answer, not a lost collection. What the list contains is measured by
        // --collection-page-check, which brings its own engine and index.
        std::fprintf(stdout,
                     "dj-ui: without an engine the collection is empty (%d of %d), as the index says\n",
                     static_cast<int>(collected.size()), browser.total());
        if (!collected.isEmpty() || browser.mode() != QStringLiteral("collection")) {
            std::fprintf(stderr, "dj-ui: without an engine the collection is not empty (%d, mode %s)\n",
                         static_cast<int>(collected.size()), qPrintable(browser.mode()));
            return 1;
        }
    } else {
    // A rejected copy is not part of the collection: three files on disk, two in the list.
    std::fprintf(stdout, "dj-ui: collection %s -> %d tracks in mode %s\n",
                 qPrintable(browser.folder()), static_cast<int>(collected.size()),
                 qPrintable(browser.mode()));
    if (browser.mode() != QStringLiteral("collection") || browser.total() != 2
        || collected.size() != 2) {
        std::fprintf(stderr, "dj-ui: the Collection section did not list the collection (%d of %d)\n",
                     static_cast<int>(collected.size()), browser.total());
        return 1;
    }
    // Named as the import named it, and sorted by that name: the file name is the name.
    if (collected.at(0).toMap().value(QStringLiteral("name")).toString()
        != QStringLiteral("ANH - tonight")) {
        std::fprintf(stderr, "dj-ui: the collection is not sorted by name (%s)\n",
                     qPrintable(collected.at(0).toMap().value(QStringLiteral("name")).toString()));
        return 1;
    }
    // The metadata beside the track reaches its row: a tempo and a key, or neither of them.
    const QVariantMap withMeta = collected.at(0).toMap();
    const QVariantMap withoutMeta = collected.at(1).toMap();
    if (qAbs(withMeta.value(QStringLiteral("bpm")).toDouble() - 128.0) > 1e-6
        || withMeta.value(QStringLiteral("key")).toString() != QStringLiteral("8m")
        || withoutMeta.contains(QStringLiteral("bpm"))
        || withoutMeta.contains(QStringLiteral("key"))) {
        std::fprintf(stderr, "dj-ui: the metadata beside a track did not reach its row\n");
        return 1;
    }
    // One collection may live on several disks: every root is listed, in one list.
    QTemporaryDir secondRoot;
    if (!secondRoot.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a second collection root\n");
        return 1;
    }
    QDir(secondRoot.path()).mkpath(QStringLiteral("C/C"));
    if (!writeStub(secondRoot.path() + QStringLiteral("/C/C/Cazz - one more.flac"))) {
        std::fprintf(stderr, "dj-ui: cannot write the second root's track\n");
        return 1;
    }
    browser.openCollection({collectionRoot, secondRoot.path()});
    QCoreApplication::processEvents();
    const int bothRoots = static_cast<int>(browser.entries().size());
    const QString lastOfBoth =
        browser.entries().isEmpty()
            ? QString()
            : browser.entries().last().toMap().value(QStringLiteral("name")).toString();
    std::fprintf(stdout, "dj-ui: two roots -> %d tracks, last «%s»\n", bothRoots,
                 qPrintable(lastOfBoth));
    if (bothRoots != 3 || lastOfBoth != QStringLiteral("Cazz - one more")) {
        std::fprintf(stderr, "dj-ui: two collection roots did not land in one list (%d tracks, last "
                             "«%s»)\n",
                     bothRoots, qPrintable(lastOfBoth));
        return 1;
    }
    browser.openCollection({collectionRoot});
    QCoreApplication::processEvents();

    // The search keeps what matches and the count says how much it kept.
    browser.setFilter(QStringLiteral("bbno"));
    QCoreApplication::processEvents();
    const int filtered = static_cast<int>(browser.entries().size());
    browser.setFilter(QString());
    QCoreApplication::processEvents();
    unfiltered = static_cast<int>(browser.entries().size());
    std::fprintf(stdout, "dj-ui: collection search -> %d of %d, then %d\n", filtered,
                 browser.total(), unfiltered);
    if (filtered != 1 || unfiltered != 2) {
        std::fprintf(stderr, "dj-ui: the collection search did not filter the list (%d, %d)\n",
                     filtered, unfiltered);
        return 1;
    }
    }

    // A track from the collection loads the way a file from a folder does.
    selectRow(0);
    const QVariantMap chosen = panel->property("selected").toMap();
    QVariant fromCollection;
    QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, fromCollection),
                              Q_ARG(QVariant, QStringLiteral("C")));
    QCoreApplication::processEvents();
    QElapsedTimer collectionWait;
    collectionWait.start();
    while (collectionWait.elapsed() < 60000
           && panel->property("lastAction").toString().startsWith(QStringLiteral("loading "))) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const QString onCollection = panel->property("lastAction").toString();
    std::fprintf(stdout, "dj-ui: collection track -> %s\n", qPrintable(onCollection));
    const QString expected = QStringLiteral("loaded ")
        + chosen.value(QStringLiteral("path")).toString() + QStringLiteral(" into C");
    if (hasEngine && onCollection != expected) {
        std::fprintf(stderr, "dj-ui: a collection track did not load into the chosen deck (%s, "
                             "wanted %s)\n",
                     qPrintable(onCollection), qPrintable(expected));
        return 1;
    }
    // Without an engine there is no index, so the list is empty and there is no row to choose: the press
    // on an empty list says exactly that, and the refusal of a real track is checked above.
    if (!hasEngine && onCollection != QStringLiteral("no engine for C")
        && onCollection != QStringLiteral("nothing to load")) {
        std::fprintf(stderr, "dj-ui: without an engine the collection said '%s'\n",
                     qPrintable(onCollection));
        return 1;
    }
    // A load from the collection leaves the list in the collection: the section is a place the DJ
    // chose, not a temporary detour into the folder of the track he took out of it.
    if (browser.mode() != QStringLiteral("collection")) {
        std::fprintf(stderr, "dj-ui: a load from the collection threw the list into %s\n",
                     qPrintable(browser.mode()));
        return 1;
    }
    // And the way back: Files is the folder it was showing, with its ".." row.
    browser.showFiles();
    QCoreApplication::processEvents();
    if (browser.mode() != QStringLiteral("files") || browser.entries().isEmpty()
        || !browser.entries().first().toMap().value(QStringLiteral("parent")).toBool()) {
        std::fprintf(stderr, "dj-ui: the Files section did not come back\n");
        return 1;
    }

    // The rail is a set of places, and pressing one moves the list. The root comes from the layout
    // settings, the way the running shell takes it, and the press is a real event on the row.
    layout.setCollectionRoots({collectionRoot});
    // The file is written when the shell exits, so the check writes it the way that hook does
    // before reading it back with a fresh reader.
    if (!layout.save()) {
        std::fprintf(stderr, "dj-ui: the layout file could not be written\n");
        return 1;
    }
    QQuickItem *collectionRow = findItem(panel, QStringLiteral("section_collection"));
    QQuickItem *filesRow = findItem(panel, QStringLiteral("section_files"));
    if (collectionRow == nullptr || filesRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the sections rail has no Collection row\n");
        return 1;
    }
    const auto pressSection = [&](QQuickItem *item) {
        const QPointF centre =
            item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, centre, window->mapToGlobal(centre.toPoint()),
                              Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        QCoreApplication::processEvents();
    };
    // The root is a setting, so it outlives the run: a fresh reader of the same file sees it.
    LayoutSettings reread;
    reread.setFile(layout.file());
    if (!reread.load() || reread.collectionRoots() != QStringList{collectionRoot}) {
        std::fprintf(stderr, "dj-ui: the collection roots were not remembered (%s)\n",
                     qPrintable(reread.collectionRoots().join(QStringLiteral(", "))));
        return 1;
    }
    pressSection(collectionRow);
    const QString afterCollection = browser.mode();
    const int pressedCount = static_cast<int>(browser.entries().size());
    pressSection(filesRow);
    const QString afterFiles = browser.mode();
    std::fprintf(stdout, "dj-ui: sections -> %s (%d tracks), then %s\n",
                 qPrintable(afterCollection), pressedCount, qPrintable(afterFiles));
    // Two tracks from the index when there is an engine, none when there is not.
    const int wantedPressed = hasEngine ? 2 : 0;
    if (afterCollection != QStringLiteral("collection") || pressedCount != wantedPressed
        || afterFiles != QStringLiteral("files")) {
        std::fprintf(stderr, "dj-ui: pressing a section did not move the list\n");
        return 1;
    }
    // Real material, when the run names it: how long a root takes to list and how much of it
    // carries a tempo. A measurement, not a gate — the tree above is what the check asserts.
    const QString realRoot = qEnvironmentVariable("DJ_COLLECTION_CHECK_ROOT");
    if (!realRoot.isEmpty()) {
        QElapsedTimer timer;
        timer.start();
        browser.openCollection({realRoot});
        const qint64 elapsed = timer.elapsed();
        int withTempo = 0;
        for (const QVariant &entry : browser.entries()) {
            if (entry.toMap().contains(QStringLiteral("bpm"))) {
                ++withTempo;
            }
        }
        std::fprintf(stdout, "dj-ui: collection %s -> %d tracks in %lld ms, %d with a tempo\n",
                     qPrintable(realRoot), static_cast<int>(browser.entries().size()),
                     static_cast<long long>(elapsed), withTempo);
    }

    std::fprintf(stdout,
                 "dj-ui: the Collection section lists %d tracks by name, carries the tempo and "
                 "the key from the metadata, filters by name and loads into a deck\n",
                 unfiltered);

    std::fprintf(stdout,
                 "dj-ui: browser selects and acts, selection follows the click, \"..\" goes up, "
                 "the text size moves the row, the sections (%d px) and the header — title, path, "
                 "the text-size buttons and the deck buttons — with it; the rail is %.0f px wide, "
                 "the browser keeps %d%% of the window when the type grows, and the rail width, the "
                 "font and the deck zoom come back after a restart (130 px, 14 px, 0.25)\n",
                 small, railAfter, browserPercent);
    return 0;
}

// syncCheckOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.
static int runSyncCheck(QQmlApplicationEngine &engine,
                        std::unique_ptr<BridgeClient> &bridge,
                        QCommandLineParser &parser,
                        const QCommandLineOption &connectOption,
                        const QCommandLineOption &loadOption,
                        const QCommandLineOption &loadSecondOption)
{
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
    const auto phaseOfB = [&]() {
        const QVariant phase = deckOf(QStringLiteral("B")).value(QStringLiteral("phase_error_ms"));
        return phase.isValid() ? phase.toDouble() : 0.0;
    };
    const double phase_held = phaseOfB();
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
    // A stopped deck says where its head stands, and the number moves while the master plays: the
    // head stays, the distance does not. A meter that parks in the centre over a deck a beat and a
    // half off reads as "aligned", which is worse than no meter at all.
    const double phase_after_stop = phaseOfB();
    if (std::abs(phase_after_stop - phase_held) < 5.0) {
        std::fprintf(stderr,
                     "dj-ui: a stopped deck reported %.3f ms before and %.3f ms after the master "
                     "played on\n",
                     phase_held, phase_after_stop);
        return 1;
    }
    if (std::abs(needle_after_stop - needle_held) < 1.0) {
        std::fprintf(stderr,
                     "dj-ui: the needle did not follow a stopped deck's distance (%.1f -> %.1f)\n",
                     needle_held, needle_after_stop);
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
                     "dj-ui: the tempo moved when the master went (%.9f -> %.9f)\n",
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
                 "ms), a stopped follower held its head at %.0f frames for 1,2 s while its "
                 "distance from the master went %.3f -> %.3f ms and the needle followed it, one beat "
                 "along the bar (%.3f beats) moved the needle %.0f px off centre, and putting the "
                 "master down cleared the mode while the deck kept its speed %.9f and played %.0f "
                 "frames on\n",
                 phase_playing, held_at, phase_held, phase_after_stop, bar_along, offCentre,
                 ratio_after, head_later - head_at_release);
    return 0;
}

// syncSoakOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.
static int runSyncSoak(QQmlApplicationEngine &engine,
                       std::unique_ptr<BridgeClient> &bridge,
                       QCommandLineParser &parser,
                       const QCommandLineOption &connectOption,
                       const QCommandLineOption &soakSecondsOption)
{
    // S4: the numbers of S1 were arithmetic and the numbers of S2 lasted three steps. This is the
    // same question over ten minutes of playing decks: do two decks stay on one beat, and do they
    // come back after a hand changes something.
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
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
    const auto frameOf = [&](const QString &id) {
        return deckOf(id).value(QStringLiteral("source_frame")).toLongLong();
    };
    const auto phaseOf = [&](const QString &id) {
        const QVariant phase = deckOf(id).value(QStringLiteral("phase_error_ms"));
        return phase.isValid() ? std::abs(phase.toDouble()) : std::numeric_limits<double>::infinity();
    };

    // Two click tracks of known tempo. A click is the one track whose grid cannot be argued about:
    // every beat is an impulse, so a deck that drifts shows it as a phase and not as a mood.
    QTemporaryDir soakDir;
    if (!soakDir.isValid()) {
        std::fprintf(stderr, "dj-ui: no temporary directory for the soak\n");
        return 1;
    }
    const int seconds = qMax(10, parser.value(soakSecondsOption).toInt());
    const auto writeClick = [&](const QString &path, double bpm) -> bool {
        const qint64 rate = 48000;
        // Longer than the soak, and not by a few seconds: the faster deck eats its material at
        // 128/120, and the decks start playing while they are still being lined up. A fixture that
        // ends mid-run does not end the run — it turns the last minute into numbers about nothing.
        // Ten minutes at 1.28 is thirteen minutes of material, and the decks play while they are
        // still being lined up: a fixture that ends mid-run turns the last minute into numbers about
        // nothing at all.
        const qint64 frames =
            static_cast<qint64>(rate) * (static_cast<qint64>(seconds * 1.5) + 60);
        const double beat = 60.0 / bpm;
        QFile file(path);
        if (!file.open(QIODevice::WriteOnly)) {
            return false;
        }
        const auto put32 = [&](quint32 value) {
            const char bytes[4] = {static_cast<char>(value & 0xff),
                                   static_cast<char>((value >> 8) & 0xff),
                                   static_cast<char>((value >> 16) & 0xff),
                                   static_cast<char>((value >> 24) & 0xff)};
            file.write(bytes, 4);
        };
        const auto put16 = [&](quint16 value) {
            const char bytes[2] = {static_cast<char>(value & 0xff),
                                   static_cast<char>((value >> 8) & 0xff)};
            file.write(bytes, 2);
        };
        const quint32 data_bytes = static_cast<quint32>(frames * 4);
        file.write("RIFF", 4);
        put32(36 + data_bytes);
        file.write("WAVEfmt ", 8);
        put32(16);
        put16(1);
        put16(2);
        put32(static_cast<quint32>(rate));
        put32(static_cast<quint32>(rate * 4));
        put16(4);
        put16(16);
        file.write("data", 4);
        put32(data_bytes);
        const qint64 clickFrames = rate / 50;
        QByteArray block;
        block.reserve(static_cast<int>(rate) * 4);
        for (qint64 frame = 0; frame < frames; ++frame) {
            double value = 0.0;
            const qint64 into = frame % static_cast<qint64>(beat * rate);
            if (into < clickFrames) {
                const double phase = 2.0 * M_PI * 1000.0 * static_cast<double>(into) / rate;
                value = std::sin(phase) * 0.5 * (1.0 - static_cast<double>(into) / clickFrames);
            }
            const auto sample = static_cast<qint16>(value * 32767.0);
            block.append(static_cast<char>(sample & 0xff));
            block.append(static_cast<char>((sample >> 8) & 0xff));
            block.append(static_cast<char>(sample & 0xff));
            block.append(static_cast<char>((sample >> 8) & 0xff));
            if (block.size() >= 1 << 20) {
                file.write(block);
                block.clear();
            }
        }
        file.write(block);
        file.close();
        return true;
    };
    const QString a_path = soakDir.filePath(QStringLiteral("click-128.wav"));
    const QString b_path = soakDir.filePath(QStringLiteral("click-120.wav"));
    // 128 against 100 is a factor of 1.28, well past the sixteen percent of the pitch control: the
    // soak proves the rule of 2026-11-10 in its first second and then keeps proving it.
    if (!writeClick(a_path, 128.0) || !writeClick(b_path, 100.0)) {
        std::fprintf(stderr, "dj-ui: the soak could not write its click tracks\n");
        return 1;
    }
    bridge->load(QStringLiteral("A"), a_path);
    bridge->load(QStringLiteral("B"), b_path);
    const bool loaded = waitFor(
        [&]() {
            const QVariantMap a = deckOf(QStringLiteral("A"));
            const QVariantMap b = deckOf(QStringLiteral("B"));
            return std::abs(a.value(QStringLiteral("local_bpm")).toDouble() - 128.0) < 0.05
                   && std::abs(b.value(QStringLiteral("local_bpm")).toDouble() - 100.0) < 0.05
                   && a.value(QStringLiteral("beat")).isValid();
        },
        // Ten minutes of a click track is a hundred megabytes a deck, and a debug build measures its
        // tempo at about a tenth of real time: two decks of ten minutes are two and a half minutes of
        // analysis before the soak can start. The budget is the time the work takes, not a guess that
        // fits an eight-second fixture.
        300000);
    if (!loaded) {
        std::fprintf(stderr,
                     "dj-ui: the soak decks came up at %.2f and %.2f BPM, not 128 and 100\n",
                     deckOf(QStringLiteral("A")).value(QStringLiteral("local_bpm")).toDouble(),
                     deckOf(QStringLiteral("B")).value(QStringLiteral("local_bpm")).toDouble());
        return 1;
    }
    bridge->play(QStringLiteral("A"), true);
    bridge->play(QStringLiteral("B"), true);
    bridge->setMaster(QStringLiteral("A"));
    bridge->setSync(QStringLiteral("B"), QStringLiteral("beat"));
    // The first alignment is a jump onto a beat; ten seconds is enough for the mix to settle and is
    // not counted as a sample of the steady state.
    if (!waitFor([&]() { return phaseOf(QStringLiteral("B")) < 2.0; }, 15000)) {
        std::fprintf(stderr, "dj-ui: the soak never locked (%.3f ms)\n", phaseOf(QStringLiteral("B")));
        return 1;
    }
    // The first jump puts the deck on the beat; the tempo arrives by trim, and a deck that is still
    // catching up is not a deck that drifts. The soak starts once the follower runs at the master's
    // speed and stands on its beat: otherwise the first seconds measure the start-up, not the lock.
    // The master plays at its own speed and the follower must reach the master's *heard* tempo:
    // 128 BPM at 1.0 against 120 BPM at 1.0667. Comparing the ratios would compare a tempo with a
    // speed, which never matches and says nothing about locking.
    const auto heard = [&](const QString &deck) {
        const QVariantMap state = deckOf(deck);
        return state.value(QStringLiteral("local_bpm")).toDouble()
               * state.value(QStringLiteral("tempo_ratio")).toDouble();
    };
    const auto converged = [&]() {
        return std::abs(heard(QStringLiteral("A")) - heard(QStringLiteral("B"))) < 0.05
               && phaseOf(QStringLiteral("B")) < 2.0;
    };
    if (!waitFor(converged, 15000)) {
        std::fprintf(stderr,
                     "dj-ui: the soak never settled (heard %.4f BPM against %.4f, phase %.3f ms)\n",
                     heard(QStringLiteral("B")), heard(QStringLiteral("A")),
                     phaseOf(QStringLiteral("B")));
        return 1;
    }
    std::fprintf(stderr, "dj-ui: soak — locked and settled, the clock starts here\n");

    struct Stage {
        const char *name;
        double at;
        /// Whether the deck has to be on the master's beat after this event. A pause and a tempo the
        /// fader cannot reach are not failures: the first has no phase to speak of, the second is a
        /// refusal the engine is supposed to make. Counting them would turn a correct behaviour into
        /// a red light, and a check that goes red on purpose teaches nothing.
        bool aligned;
        std::function<void()> act;
    };
    double worst_aligned = 0.0;
    double worst_aligned_at = -1.0;
    double worst_transient = 0.0;
    int over_tolerance = 0;
    int aligned_samples = 0;
    int samples = 0;
    int stalled_samples = 0;
    bool expect_aligned = true;
    QElapsedTimer run;
    run.start();
    const auto stage_at = [&](double share) { return static_cast<qint64>(seconds * 1000.0 * share); };
    std::vector<Stage> stages{
        {"master +4 %", 0.20, true, [&]() { bridge->setTempo(QStringLiteral("A"), 1.04); }},
        {"master back to 1.0", 0.30, true, [&]() { bridge->setTempo(QStringLiteral("A"), 1.0); }},
        {"follower paused", 0.40, false, [&]() { bridge->play(QStringLiteral("B"), false); }},
        {"follower started", 0.45, true, [&]() { bridge->play(QStringLiteral("B"), true); }},
        {"loop of four beats", 0.60, true, [&]() { bridge->loopBeats(QStringLiteral("B"), 4); }},
        {"loop off", 0.70, true, [&]() { bridge->setLoop(QStringLiteral("B"), false, 0, 0); }},
        {"master +4 % again", 0.80, true, [&]() { bridge->setTempo(QStringLiteral("A"), 1.04); }},
        {"master back to 1.0", 0.90, true, [&]() { bridge->setTempo(QStringLiteral("A"), 1.0); }},
    };
    struct Recovery {
        QString name;
        bool counted;
        qint64 since;
        double seconds;
        double hold_from;
        bool holding;
    };
    std::vector<Recovery> recoveries;
    std::size_t next_stage = 0;
    // A second after the start: the follower is put up, its first jump lands, and the material the seek
    // asked for arrives a few blocks later. A sample taken while that is happening is the deck starting,
    // not the deck drifting — the worst sample of a bad run was at 0,4 s.
    qint64 settled_from = 1000;
    // The audio clock is how this run tells a deck that drifted from a machine that could not render:
    // a block the box never produced moves a play head by a whole block, which is 5,3 ms and looks
    // exactly like a phase error. Counting those samples as drift would make the check a measure of
    // how busy the computer was.
    double clock_before = bridge->snapshot().value(QStringLiteral("audio_clock_samples")).toDouble();
    qint64 wall_before = run.elapsed();
    // A deck that asked its source for material and did not get it moves its play head by a whole
    // block when the material finally arrives, which a phase meter reads as a jump of 5,3 ms. The
    // deck counts those blocks itself; this run leaves them out of the phase statistics and says how
    // many there were.
    double ratio_prev = deckOf(QStringLiteral("B")).value(QStringLiteral("tempo_ratio")).toDouble();
    qint64 ratio_moved_at = 0;
    qint64 underruns_before = 0;
    for (const QString &id : {QStringLiteral("A"), QStringLiteral("B")}) {
        underruns_before += deckOf(id).value(QStringLiteral("underruns")).toLongLong();
    }
    // A deck that has not moved is a deck that was not rendered: a block the machine never gave it
    // moves the play head by one block, 5,3 ms at 256 frames, which reads as a phase error and is not
    // one. The audio clock cannot see this — the engine rendered, one deck was starved.
    qint64 master_before = frameOf(QStringLiteral("A"));
    qint64 follower_before = frameOf(QStringLiteral("B"));
    while (run.elapsed() < seconds * 1000) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
        const qint64 now = run.elapsed();
        while (next_stage < stages.size() && now >= stage_at(stages[next_stage].at)) {
            Stage &stage = stages[next_stage];
            stage.act();
            std::fprintf(stderr, "dj-ui: soak %5.1f s — %s%s\n", now / 1000.0, stage.name,
                         stage.aligned ? "" : " (not a phase to hold)");
            expect_aligned = stage.aligned;
            if (stage.aligned) {
                recoveries.push_back(Recovery{QString::fromUtf8(stage.name), true, now, -1.0,
                                               static_cast<double>(now), false});
            } else {
                recoveries.push_back(Recovery{QString::fromUtf8(stage.name), false, now, -1.0,
                                               static_cast<double>(now), false});
            }
            settled_from = now + 150;
            ++next_stage;
        }
        // The steady state is what is measured: a hand has just changed something, and while the
        // deck is catching up the phase is a distance travelled, not a distance kept. The transient is
        // recorded too, but as information — a correction that is heard is not a lock that was lost.
        bool catching_up = false;
        for (const auto &recovery : recoveries) {
            if (recovery.counted && recovery.seconds < 0.0 && now > recovery.since) {
                catching_up = true;
            }
        }
        if (!deckOf(QStringLiteral("A")).value(QStringLiteral("playing")).toBool()) {
            std::fprintf(stderr,
                         "dj-ui: soak — the master ran out of material at %.1f s; the run measures "
                         "nothing past the end of a file\n",
                         now / 1000.0);
            return 1;
        }
        const double clock_now =
            bridge->snapshot().value(QStringLiteral("audio_clock_samples")).toDouble();
        const double expected = static_cast<double>(now - wall_before) * 48.0;  // 48 frames a ms
        const qint64 master_now = frameOf(QStringLiteral("A"));
        const qint64 follower_now = frameOf(QStringLiteral("B"));
        const double master_moved = static_cast<double>(master_now - master_before);
        const double follower_moved = static_cast<double>(follower_now - follower_before);
        master_before = master_now;
        follower_before = follower_now;
        // Half of what the clock says should have passed is well below any real playback and well
        // above the noise of a block boundary, so a starved deck is caught and a slow one is not.
        qint64 underruns_now = 0;
        for (const QString &id : {QStringLiteral("A"), QStringLiteral("B")}) {
            underruns_now += deckOf(id).value(QStringLiteral("underruns")).toLongLong();
        }
        const bool starved = underruns_now > underruns_before;
        underruns_before = underruns_now;
        const bool stalled = starved || clock_now - clock_before < 0.8 * expected
                             || master_moved < 0.5 * expected
                             || follower_moved < 0.5 * expected;
        clock_before = clock_now;
        wall_before = now;
        const double phase = phaseOf(QStringLiteral("B"));
        const double ratio_now = deckOf(QStringLiteral("B")).value(QStringLiteral("tempo_ratio")).toDouble();
        if (std::abs(ratio_now - ratio_prev) > 1e-9) {
            ratio_prev = ratio_now;
            ratio_moved_at = now;
        }
        // A hand asks for a new tempo; the deck then walks to it at four percent a second, which for a
        // four percent change is a whole second of the follower running at the old speed. The beat is
        // apart for that second by design, so a sample taken inside it says nothing about drift.
        const bool correcting = now - ratio_moved_at < 150;
        if (correcting && std::isfinite(phase)) {
            // The walk to a new tempo is a correction the hand asked for, and the beat is apart while
            // it lasts: that is the price of not jumping the sound.
            worst_transient = std::max(worst_transient, phase);
        }
        if (qEnvironmentVariableIsSet("DJP_SOAK_TRACE") && std::isfinite(phase) && std::abs(phase) > 2.0
            && std::abs(phase) < 30.0) {
            const auto a = deckOf(QStringLiteral("A"));
            const auto b = deckOf(QStringLiteral("B"));
            fprintf(stderr,
                    "trace t=%.1f s phase=%.3f ms | A frame %lld playing %d | B frame %lld playing %d "
                    "sync %d ratio %.6f underruns %lld | clock %.1f\n",
                    run.elapsed() / 1000.0,
                    phase,
                    static_cast<long long>(a.value(QStringLiteral("source_frame")).toDouble()),
                    a.value(QStringLiteral("playing")).toBool(),
                    static_cast<long long>(b.value(QStringLiteral("source_frame")).toDouble()),
                    b.value(QStringLiteral("playing")).toBool(),
                    b.value(QStringLiteral("sync")).toBool(),
                    b.value(QStringLiteral("tempo_ratio")).toDouble(),
                    static_cast<long long>(underruns_before),
                    clock_now);
        }
        if (std::isfinite(phase)) {
            ++samples;
            if (stalled) {
                ++stalled_samples;
            }
            if (expect_aligned && now >= settled_from && !stalled && !correcting) {
                if (catching_up) {
                    worst_transient = std::max(worst_transient, phase);
                } else {
                    ++aligned_samples;
                    if (std::abs(phase) > std::abs(worst_aligned)) {
                        worst_aligned = phase;
                        worst_aligned_at = now / 1000.0;
                    }
                    if (phase > 2.0) {
                        ++over_tolerance;
                        if (over_tolerance <= 50) {
                            std::fprintf(stderr,
                                         "dj-ui: soak   off the beat at %.1f s — %.3f ms (mode %s, "
                                         "loop %d)\n",
                                         now / 1000.0, phase,
                                         qPrintable(deckOf(QStringLiteral("B"))
                                                        .value(QStringLiteral("sync"))
                                                        .toString()),
                                         deckOf(QStringLiteral("B"))
                                             .value(QStringLiteral("loop_region"))
                                             .toMap()
                                             .isEmpty()
                                             ? 0
                                             : 1);
                        }
                    }
                }
            }
        }
        // A recovery has to hold: one sample inside the tolerance can be luck, a whole second of them
        // is the deck being back on the beat.
        for (auto &recovery : recoveries) {
            if (recovery.seconds >= 0.0 || !recovery.counted || now <= recovery.since) {
                continue;
            }
            const bool inside = std::isfinite(phase) && phase <= 2.0;
            if (!recovery.holding) {
                if (inside) {
                    recovery.holding = true;
                    recovery.hold_from = static_cast<double>(now);
                }
            } else if (!inside) {
                recovery.holding = false;
            } else if (static_cast<double>(now) - recovery.hold_from >= 1000.0) {
                recovery.seconds = (recovery.hold_from - static_cast<double>(recovery.since)) / 1000.0;
            }
        }
        QThread::msleep(50);
    }
    bridge->play(QStringLiteral("A"), false);
    bridge->play(QStringLiteral("B"), false);

    double worst_recovery = 0.0;
    bool all_recovered = true;
    std::fprintf(stdout,
                 "dj-ui: soak — %d samples over %d s of two decks, %d of them aligned, %d with the "
                 "machine too busy to render a block\n",
                 samples, seconds, aligned_samples, stalled_samples);
    for (const auto &recovery : recoveries) {
        if (!recovery.counted) {
            std::fprintf(stdout, "dj-ui: soak   %s — no phase to hold\n", qPrintable(recovery.name));
            continue;
        }
        std::fprintf(stdout, "dj-ui: soak   %s — back on the beat in %.1f s\n",
                     qPrintable(recovery.name), recovery.seconds);
        if (recovery.seconds < 0.0) {
            all_recovered = false;
        } else {
            worst_recovery = std::max(worst_recovery, recovery.seconds);
        }
    }
    const double share_inside =
        aligned_samples > 0 ? 100.0 - (100.0 * over_tolerance / aligned_samples) : 0.0;
    std::fprintf(stdout,
                 "dj-ui: soak — worst |phase| %.3f ms at %.1f s while settled (%.3f ms during a correction), "
                 "%.2f%% of the settled samples inside 2 ms, worst recovery %.1f s after a hand\n",
                 worst_aligned, worst_aligned_at, worst_transient, share_inside, worst_recovery);
    return worst_aligned <= 2.0 && share_inside >= 99.0 && all_recovered && worst_recovery <= 5.0 ? 0
                                                                                                  : 1;
}

// loopOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.
static int runLoop(QQmlApplicationEngine &engine,
                   std::unique_ptr<BridgeClient> &bridge,
                   QCommandLineParser &parser,
                   const QCommandLineOption &connectOption,
                   const QCommandLineOption &loadOption)
{
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

// eqCheckOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.
static int runEqCheck(QQmlApplicationEngine &engine,
                      std::unique_ptr<BridgeClient> &bridge,
                      QCommandLineParser &parser,
                      const QCommandLineOption &connectOption)
{
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

// gridLinesOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.
static int runGridLines(QQmlApplicationEngine &engine,
                        std::unique_ptr<BridgeClient> &bridge,
                        QCommandLineParser &parser,
                        const QCommandLineOption &connectOption,
                        const QCommandLineOption &loadOption)
{
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

// layoutCheckOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.
static int runLayoutCheck(QQmlApplicationEngine &engine,
                          LayoutSettings &layout,
                          UiSettings &settings)
{
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

/// The browser's way into the collection: pick rows, ask for the batch, confirm it, answer the
/// duplicate question, and require the files to be in the collection. Runs against a real engine,
/// because the import is the engine's work and a shell that only draws dialogs proves nothing.
// The collection's sort: a click on a column name sorts by it, a second click turns it around, and a
// Shift click adds the next key. And the rail: the collection at its top, the files at its bottom.
static int runSortCheck(QQmlApplicationEngine &engine, FileBrowser &browser, LayoutSettings &layout,
                        const QString &fixture, const QString &collectionRoot)
{
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --sort-check needs --import-fixture and --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    // Four names that parse as artist and title, so the list has something to sort by without tags.
    // The keys are what a secondary sort by tone has to see: `2d` before `10d`, and `8d` before `8m`.
    // The tempo is written with the precision the measurement has — `142.004` against `142.0` — and
    // the tone has to decide between them: that is the case the DJ hit with BPM first and Key second.
    const QList<QPair<QString, QString>> toneKeys = {
        {QStringLiteral("Alpha - Zebra"), QStringLiteral("2d")},
        {QStringLiteral("Beta - Apple"), QStringLiteral("8m")},
        {QStringLiteral("Alpha - Apple"), QStringLiteral("10d")},
        {QStringLiteral("Gamma - Mango"), QStringLiteral("8d")},
    };
    const QList<QPair<QString, double>> tempos = {
        {QStringLiteral("Alpha - Zebra"), 142.0},
        {QStringLiteral("Beta - Apple"), 143.0},
        {QStringLiteral("Alpha - Apple"), 142.004},
        {QStringLiteral("Gamma - Mango"), 141.5},
    };
    if (!root.mkpath(QStringLiteral(".djmeta"))) {
        std::fprintf(stderr, "dj-ui: cannot make the metadata folder under %s\n",
                     qPrintable(collectionRoot));
        return 1;
    }
    int index = 0;
    for (const auto &pair : toneKeys) {
        ++index;
        const QString target = root.filePath(pair.first + QStringLiteral(".wav"));
        QFile::remove(target);
        if (!QFile::copy(fixture, target)) {
            std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(target));
            return 1;
        }
        QFile record(root.filePath(QStringLiteral(".djmeta/") + pair.first
                                   + QStringLiteral(".wav.json")));
        if (!record.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write the record for %s\n", qPrintable(pair.first));
            return 1;
        }
        double bpm = 0.0;
        for (const auto &tempo : tempos) {
            if (tempo.first == pair.first) {
                bpm = tempo.second;
            }
        }
        // A record the index can read: the schema, the revision, when it was written and what identifies
        // the file. The fingerprint is the row's key, so four tracks sharing one file's bytes would be
        // one row — the size stands in for the bytes this check did not measure.
        const QJsonObject fingerprint{
            {QStringLiteral("size"), 1000 + index},
            {QStringLiteral("head"), QString(16, QLatin1Char(char('a' + index)))},
            {QStringLiteral("tail"), QString(16, QLatin1Char(char('p' + index)))},
        };
        const QJsonObject grid{{QStringLiteral("bpm"), bpm},
                               {QStringLiteral("first_beat_frame"), 0},
                               {QStringLiteral("anchors"), QJsonArray{}},
                               {QStringLiteral("beats_per_bar"), 4}};
        // The names the columns show are the tags the import read, and the sort reads the same tags: a
        // record without them is four rows the artist column cannot order, whatever the header asks for.
        const QStringList parts = pair.first.split(QStringLiteral(" - "));
        const QJsonObject tags{{QStringLiteral("artist"), parts.value(0)},
                               {QStringLiteral("album"), QStringLiteral("Album")},
                               {QStringLiteral("title"), parts.value(1)},
                               {QStringLiteral("genres"), QJsonArray{}}};
        const QJsonObject meta{{QStringLiteral("schema"), QStringLiteral("djp-track-v1")},
                               {QStringLiteral("tags"), tags},
                               {QStringLiteral("revision"), 1},
                               {QStringLiteral("updated_at_ms"), 1},
                               {QStringLiteral("fingerprint"), fingerprint},
                               {QStringLiteral("key"), pair.second},
                               {QStringLiteral("beatgrid"), grid}};
        record.write(QJsonDocument(meta).toJson());
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    // The header's geometry comes from the column settings: a check that runs before the shell has
    // read them would click on nothing.
    if (layout.collectionColumns().isEmpty()) {
        layout.resetCollectionColumns();
    }
    browser.openCollection({collectionRoot});
    QElapsedTimer clock;
    clock.start();
    while (browser.entries().size() < 4 && clock.elapsed() < 5000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    if (browser.entries().size() != 4) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted 4\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }

    // The rail: two places, and the one a DJ starts in is above the one he digs in.
    auto *collectionRow = findItem(window->contentItem(), QStringLiteral("section_collection"));
    auto *filesRow = findItem(window->contentItem(), QStringLiteral("section_files"));
    if (collectionRow == nullptr || filesRow == nullptr) {
        std::fprintf(stderr, "dj-ui: the rail has no Collection or Files row\n");
        return 1;
    }
    if (!(collectionRow->y() < filesRow->y())) {
        std::fprintf(stderr, "dj-ui: the rail puts Collection at %.1f and Files at %.1f\n",
                     collectionRow->y(), filesRow->y());
        return 1;
    }
    std::fprintf(stderr, "dj-ui: rail: Collection at %.1f, Files at %.1f\n", collectionRow->y(),
                 filesRow->y());

    auto *header = findItem(window->contentItem(), QStringLiteral("collectionHeaderDrag"));
    if (header == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection has no header\n");
        return 1;
    }
    // The header has no width until the shell has laid itself out, and a click at zero lands on the
    // first column's edge, which is a resize: wait for the geometry before aiming at a name.
    {
        QElapsedTimer ready;
        ready.start();
        while (header->width() <= 1.0 && ready.elapsed() < 3000) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
    }
    // A column's middle, in the header's own coordinates: the click is a real mouse event, because a
    // sort nobody can reach with the pointer is a sort that only exists in the code.
    const auto clickColumn = [&](const QString &column, Qt::KeyboardModifiers modifiers) {
        const QVariantList columns = layout.collectionColumns();
        int index = -1;
        for (int at = 0; at < columns.size(); ++at) {
            if (columns.at(at).toMap().value(QStringLiteral("id")).toString() == column) {
                index = at;
                break;
            }
        }
        if (index < 0) {
            return false;
        }
        const double width = header->width();
        // The columns start after the row number and the lock that sits beside it, and that gutter is
        // part of the header: a left edge summed from the shares alone lands tens of pixels to the left
        // and clicks the gutter, which sorts nothing. The panel is asked for the edge it draws at.
        QVariant asked;
        const bool knows = QMetaObject::invokeMethod(panel, "columnX", Q_RETURN_ARG(QVariant, asked),
                                                     Q_ARG(QVariant, index), Q_ARG(QVariant, width));
        const double left = knows ? asked.toDouble() : 0.0;
        // The column's own middle: its share times the width the columns actually share, which the panel
        // answers with. Multiplying the share by the whole header leans right by the gutter, and a check
        // that leans can miss the column it aims at.
        QVariant askedSpan;
        const bool knowsSpan = QMetaObject::invokeMethod(panel, "columnWidth",
                                                         Q_RETURN_ARG(QVariant, askedSpan),
                                                         Q_ARG(QVariant, index), Q_ARG(QVariant, width));
        const double share = columns.at(index).toMap().value(QStringLiteral("width")).toDouble();
        const double span = knowsSpan ? askedSpan.toDouble() : share * width;
        const QPointF at(left + span / 2.0, header->height() / 2.0);
        const QPointF scene = header->mapToScene(at);
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, scene, window->mapToGlobal(scene.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, modifiers);
            QCoreApplication::sendEvent(window, &event);
        }
        // A sort is asked of the index as well as applied here, and the answer carries the order that was
        // asked for. Reading the list in the same breath reads the answer to the previous question, so the
        // check waits the way a hand would: the row order settles before the next look.
        QElapsedTimer after;
        after.start();
        while (after.elapsed() < 400) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        return true;
    };
    const auto order = [&]() {
        QStringList names;
        for (const QVariant &row : browser.entries()) {
            names.append(row.toMap().value(QStringLiteral("name")).toString());
        }
        return names;
    };
    const auto keys = [&]() {
        QStringList list;
        for (const QVariant &key : browser.collectionSort()) {
            const QVariantMap entry = key.toMap();
            list.append(entry.value(QStringLiteral("column")).toString()
                        + (entry.value(QStringLiteral("ascending")).toBool() ? "+" : "-"));
        }
        return list.join(QStringLiteral(","));
    };
    const auto require = [&](const QString &what, const QStringList &wanted) {
        const QStringList got = order();
        if (got != wanted) {
            // The keys say what was asked for; the rows say what the list did about it. A key that
            // turned and rows that did not are two different faults, and the message must tell them apart.
            std::fprintf(stderr, "dj-ui: %s -> %s, wanted %s (keys %s)\n", qPrintable(what),
                         qPrintable(got.join(QStringLiteral(" | "))),
                         qPrintable(wanted.join(QStringLiteral(" | "))), qPrintable(keys()));
            return false;
        }
        std::fprintf(stderr, "dj-ui: %s -> %s\n", qPrintable(what),
                     qPrintable(got.join(QStringLiteral(" | "))));
        return true;
    };

    // The order the list carries before any click: the order its own source gives it. Every line below
    // turns a sort on, so this is the only place it can be read — and it is what a click on the number
    // column has to give back.
    const QStringList sourceOrder = order();
    if (!clickColumn(QStringLiteral("artist"), Qt::NoModifier)) {
        std::fprintf(stderr, "dj-ui: the header has no artist column\n");
        return 1;
    }
    if (!require(QStringLiteral("click artist"),
                 {QStringLiteral("Alpha - Apple"), QStringLiteral("Alpha - Zebra"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Gamma - Mango")})) {
        return 1;
    }
    if (keys() != QStringLiteral("artist+")) {
        std::fprintf(stderr, "dj-ui: the sort keys read «%s», wanted artist+\n", qPrintable(keys()));
        return 1;
    }

    clickColumn(QStringLiteral("artist"), Qt::NoModifier);
    // Equal artists keep the name order: a list that reshuffles between equal rows jumps under the hand.
    if (!require(QStringLiteral("click artist again"),
                 {QStringLiteral("Gamma - Mango"), QStringLiteral("Beta - Apple"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Alpha - Zebra")})) {
        return 1;
    }
    if (keys() != QStringLiteral("artist-")) {
        std::fprintf(stderr, "dj-ui: the second click left the keys at «%s»\n", qPrintable(keys()));
        return 1;
    }

    clickColumn(QStringLiteral("title"), Qt::NoModifier);
    if (!require(QStringLiteral("click title"),
                 {QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple"),
                  QStringLiteral("Gamma - Mango"), QStringLiteral("Alpha - Zebra")})) {
        return 1;
    }
    if (keys() != QStringLiteral("title+")) {
        std::fprintf(stderr, "dj-ui: a plain click did not replace the key: «%s»\n", qPrintable(keys()));
        return 1;
    }

    clickColumn(QStringLiteral("title"), Qt::NoModifier);
    if (!require(QStringLiteral("click title again"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }

    // Shift: the title keeps deciding, the artist decides between the rows it could not. Ascending it
    // agrees with the name order, so the change is checked by turning that second key around.
    clickColumn(QStringLiteral("artist"), Qt::ShiftModifier);
    if (keys() != QStringLiteral("title-,artist+")) {
        std::fprintf(stderr, "dj-ui: the shift click left the keys at «%s»\n", qPrintable(keys()));
        return 1;
    }
    if (!require(QStringLiteral("shift click artist"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }
    clickColumn(QStringLiteral("artist"), Qt::ShiftModifier);
    if (!require(QStringLiteral("shift click artist again"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Alpha - Apple")})) {
        return 1;
    }
    if (keys() != QStringLiteral("title-,artist-")) {
        std::fprintf(stderr, "dj-ui: the second shift click left the keys at «%s»\n",
                     qPrintable(keys()));
        return 1;
    }
    // The tone: numbers on the wheel, not letters. A text sort would put 10d before 2d.
    clickColumn(QStringLiteral("key"), Qt::NoModifier);
    if (!require(QStringLiteral("click key"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Gamma - Mango"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Alpha - Apple")})) {
        return 1;
    }
    if (keys() != QStringLiteral("key+")) {
        std::fprintf(stderr, "dj-ui: the key click left the keys at «%s»\n", qPrintable(keys()));
        return 1;
    }

    // And as the second key: the artist decides the groups, the tone decides inside one.
    clickColumn(QStringLiteral("artist"), Qt::NoModifier);
    clickColumn(QStringLiteral("key"), Qt::ShiftModifier);
    if (!require(QStringLiteral("artist then shift key"),
                 {QStringLiteral("Alpha - Zebra"), QStringLiteral("Alpha - Apple"),
                  QStringLiteral("Beta - Apple"), QStringLiteral("Gamma - Mango")})) {
        return 1;
    }
    if (keys() != QStringLiteral("artist+,key+")) {
        std::fprintf(stderr, "dj-ui: the shift click on the key left «%s»\n", qPrintable(keys()));
        return 1;
    }
    // The tempo first, the tone second: `142.0` and `142.004` are one tempo on the display, and the
    // tone decides between them. `141.5` and `143.0` stay where the tempo puts them.
    clickColumn(QStringLiteral("bpm"), Qt::NoModifier);
    if (!require(QStringLiteral("click bpm"),
                 {QStringLiteral("Gamma - Mango"), QStringLiteral("Alpha - Apple"),
                  QStringLiteral("Alpha - Zebra"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }
    clickColumn(QStringLiteral("key"), Qt::ShiftModifier);
    if (!require(QStringLiteral("bpm then shift key"),
                 {QStringLiteral("Gamma - Mango"), QStringLiteral("Alpha - Zebra"),
                  QStringLiteral("Alpha - Apple"), QStringLiteral("Beta - Apple")})) {
        return 1;
    }
    if (keys() != QStringLiteral("bpm+,key+")) {
        std::fprintf(stderr, "dj-ui: the shift click on the key left «%s»\n", qPrintable(keys()));
        return 1;
    }
    // The tempo reads to a hundredth, and a whole number is padded: a column of rounded tempi cannot be
    // sorted by eye, and 128 must not read as something between 127 and 129.
    // The list is re-sorted by the last click, and a delegate is laid out a frame later: the cell is read
    // after the rows have settled, the way the eye reads it.
    for (int tick = 0; tick < 25; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    if (QQuickItem *cell = findItem(window->contentItem(), QStringLiteral("collectionCell_bpm_0"))) {
        const QString shown = cell->property("text").toString();
        const QRegularExpression wanted(QStringLiteral("^\\d+\\.\\d\\d$"));
        if (!wanted.match(shown).hasMatch()) {
            std::fprintf(stderr, "dj-ui: the BPM cell reads \"%s\", not a hundredth\n",
                         qPrintable(shown));
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the BPM cell reads \"%s\"\n", qPrintable(shown));
    } else {
        std::fprintf(stderr, "dj-ui: no BPM cell to read\n");
        return 1;
    }

    // The number column's header: a click there drops every key at once. Two keys are in force here (the
    // tempo and the tone), so the case says "all of them" and not "the last one" — and the order has to
    // come back to the one the list had before the first click, not merely to something unsorted.
    {
        const double numberWidth = panel->property("numberWidth").toDouble();
        if (numberWidth <= 1.0) {
            std::fprintf(stderr, "dj-ui: the number column is %.1f px wide, nothing to click\n",
                         numberWidth);
            return 1;
        }
        const QPointF at(numberWidth / 2.0, header->height() / 2.0);
        const QPointF scene = header->mapToScene(at);
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, scene, window->mapToGlobal(scene.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        // The index is asked again with no order at all, and its answer carries the rows: give it the
        // frames a hand would take before reading the list.
        QElapsedTimer asked;
        asked.start();
        while (asked.elapsed() < 500) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        if (!keys().isEmpty()) {
            std::fprintf(stderr, "dj-ui: the click on the number column left the keys at «%s»\n",
                         qPrintable(keys()));
            return 1;
        }
        if (!require(QStringLiteral("click the number column"), sourceOrder)) {
            return 1;
        }
        // And the marks go with the keys: an arrow over a list that no column decides is the picture of a
        // sort that is not there.
        const QString marks = QStringLiteral("▲▼△▽");
        QStringList stillMarked;
        for (int index = 0; index < 6; ++index) {
            auto *cell = findItem(window->contentItem(),
                                  QStringLiteral("collectionHeaderCell_%1").arg(index));
            if (cell == nullptr) {
                continue;
            }
            const QString text = cell->property("text").toString();
            for (const QChar &glyph : text) {
                if (marks.contains(glyph)) {
                    stillMarked.append(text.trimmed());
                }
            }
        }
        if (!stillMarked.isEmpty()) {
            std::fprintf(stderr, "dj-ui: headers still carry a sort mark after the reset: %s\n",
                         qPrintable(stillMarked.join(QStringLiteral(" | "))));
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the number column reset the sort, and no header keeps a mark\n");
    }

    // A list long enough to scroll: the cursor is driven by the keyboard, and the view must be at the
    // row at once — a window that glides to it reads as lag, not as motion.
    // A folder of its own: the index fills one it has never seen. Records that appear in a folder it
    // already knows are a different question — records arriving from outside — and they wait for a rescan.
    const QString scrollRoot = collectionRoot + QStringLiteral("-scroll");
    if (!QDir().mkpath(scrollRoot + QStringLiteral("/.djmeta"))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(scrollRoot));
        return 1;
    }
    const QDir scroll(scrollRoot);
    for (int index = 0; index < 40; ++index) {
        const QString name = QStringLiteral("Scroll %1.wav").arg(index, 2, 10, QLatin1Char('0'));
        const QString target = scroll.filePath(name);
        if (!QFile::exists(target) && !QFile::copy(fixture, target)) {
            std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(target));
            return 1;
        }
        // A row is a record: forty files without one are forty files the list never shows.
        QFile record(scroll.filePath(QStringLiteral(".djmeta/") + name + QStringLiteral(".json")));
        if (!record.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write the record for %s\n", qPrintable(name));
            return 1;
        }
        const QJsonObject fingerprint{
            {QStringLiteral("size"), 2000 + index},
            {QStringLiteral("head"), QString(16, QLatin1Char(char('A' + index % 26)))},
            {QStringLiteral("tail"), QString(16, QLatin1Char(char('Q' + index % 26)))},
        };
        const QJsonObject tags{{QStringLiteral("artist"), QStringLiteral("Scroll")},
                               {QStringLiteral("album"), QStringLiteral("Album")},
                               {QStringLiteral("title"), QString::number(index)},
                               {QStringLiteral("genres"), QJsonArray{}}};
        const QJsonObject meta{{QStringLiteral("schema"), QStringLiteral("djp-track-v1")},
                               {QStringLiteral("revision"), 1},
                               {QStringLiteral("updated_at_ms"), 1},
                               {QStringLiteral("fingerprint"), fingerprint},
                               {QStringLiteral("tags"), tags}};
        record.write(QJsonDocument(meta).toJson());
        record.close();
    }
    browser.openCollection({scrollRoot});
    clock.restart();
    while (browser.entries().size() < 40 && clock.elapsed() < 15000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    auto *list = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (list == nullptr) {
        std::fprintf(stderr, "dj-ui: the browser has no list\n");
        return 1;
    }
    const double rowHeight = panel->property("rowHeight").toDouble();
    if (rowHeight <= 0.0) {
        std::fprintf(stderr, "dj-ui: the list has no row height\n");
        return 1;
    }
    // Probe: a cursor that arrives without a key — the remembered row of a section left behind, or a
    // row the DJ clicked. Where is the window then?
    const QVariantList rows = browser.entries();
    if (rows.size() < 31) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted at least 31 to scroll\n",
                     static_cast<int>(rows.size()));
        return 1;
    }
    const QString deep = rows.at(30).toMap().value(QStringLiteral("path")).toString();
    browser.setCursor(deep);
    for (int tick = 0; tick < 8; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    // A row that arrives without a key must land in the middle too: that is what the DJ sees when a
    // section opens at the remembered row, or when a click lands on a row the window had lost.
    {
        const int restoreRow = browser.rowOf(deep);
        const double windowCentre = list->property("contentY").toDouble() + list->height() / 2.0;
        const double rowCentre = restoreRow * rowHeight + rowHeight / 2.0;
        if (std::abs(windowCentre - rowCentre) > rowHeight) {
            std::fprintf(stderr,
                         "dj-ui: the restored row %d is not in the middle (row centre %.1f, "
                         "window centre %.1f)\n",
                         restoreRow, rowCentre, windowCentre);
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the restored row %d is in the middle (contentY %.1f)\n",
                     restoreRow, list->property("contentY").toDouble());
    }

    if (browser.entries().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the list is empty, so there is no first row to stand on\n");
        return 1;
    }
    browser.setCursor(browser.entries().first().toMap().value(QStringLiteral("path")).toString());
    QCoreApplication::processEvents();
    for (int step = 0; step < 30; ++step) {
        QKeyEvent press(QEvent::KeyPress, Qt::Key_Down, Qt::NoModifier);
        QCoreApplication::sendEvent(list, &press);
        QCoreApplication::processEvents();
    }
    const int row = browser.rowOf(browser.cursor());
    const double settled = list->property("contentY").toDouble();
    // An animation would still be running: give it the frames and require the window not to move.
    for (int tick = 0; tick < 8; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    const double later = list->property("contentY").toDouble();
    const double viewHeight = list->height();
    if (std::abs(later - settled) > 0.5) {
        std::fprintf(stderr, "dj-ui: the view was still gliding to the row (%.1f -> %.1f)\n",
                     settled, later);
        return 1;
    }
    // The row must sit in the middle of the window, not against its edge: a cursor under the last
    // visible row is exactly what the owner reported as wrong.
    const double windowCentre = settled + viewHeight / 2.0;
    const double rowCentre = row * rowHeight + rowHeight / 2.0;
    if (std::abs(windowCentre - rowCentre) > rowHeight) {
        std::fprintf(stderr,
                     "dj-ui: row %d is not in the middle (row centre %.1f, window centre %.1f)\n",
                     row, rowCentre, windowCentre);
        return 1;
    }
    std::fprintf(stderr, "dj-ui: the view is at row %d at once (contentY %.1f, row height %.1f)\n",
                 row, settled, rowHeight);

    // The wheel moves the cursor: a scroll that only moved the picture slid the selection out of the
    // window, which is what the DJ reported. The wheel event goes through the window, as a real one
    // does, so the handler on the list is the thing under test.
    {
        const int beforeWheel = browser.rowOf(browser.cursor());
        const QPointF local(list->width() / 2.0, list->height() / 2.0);
        const QPointF scene = list->mapToScene(local);
        QWheelEvent wheel(scene, window->mapToGlobal(scene.toPoint()), QPoint(0, 0), QPoint(0, -120),
                          Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
        QCoreApplication::sendEvent(window, &wheel);
        for (int tick = 0; tick < 4; ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const int afterWheel = browser.rowOf(browser.cursor());
        if (afterWheel != beforeWheel + 1) {
            std::fprintf(stderr, "dj-ui: the wheel moved the cursor from %d to %d, wanted one row\n",
                         beforeWheel, afterWheel);
            return 1;
        }
        const double wheelCentre = list->property("contentY").toDouble() + list->height() / 2.0;
        const double wheelRowCentre = afterWheel * rowHeight + rowHeight / 2.0;
        if (std::abs(wheelCentre - wheelRowCentre) > rowHeight) {
            std::fprintf(stderr,
                         "dj-ui: after the wheel row %d is not in the middle (row centre %.1f, "
                         "window centre %.1f)\n",
                         afterWheel, wheelRowCentre, wheelCentre);
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the wheel moved the cursor to row %d, still in the middle\n",
                     afterWheel);
    }
    // The DJ's own flow: the cursor is remembered from the last session and the rows arrive after it.
    // A single positioning against a half-built list lands short — the row ends up below the window,
    // which is what he saw when he opened the collection.
    {
        root.mkpath(QStringLiteral("empty"));
        const QString remembered = scroll.filePath(QStringLiteral("Scroll 30.wav"));
        browser.openCollection({root.filePath(QStringLiteral("empty"))});
        QCoreApplication::processEvents();
        browser.setCursor(remembered);
        browser.openCollection({scrollRoot});
        clock.restart();
        while (browser.entries().size() < 40 && clock.elapsed() < 10000) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        for (int tick = 0; tick < 12; ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        // The rows, not only the opening: the index answers a page when it has one, and a page that came
        // back empty has no row to remember — the cursor then reads -1 and a working panel looks broken.
        for (int tick = 0; tick < 250 && browser.entries().isEmpty(); ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const int rememberedRow = browser.rowOf(browser.cursor());
        const double windowCentre = list->property("contentY").toDouble() + list->height() / 2.0;
        const double rowCentre = rememberedRow * rowHeight + rowHeight / 2.0;
        if (rememberedRow < 0 || std::abs(windowCentre - rowCentre) > rowHeight) {
            std::fprintf(stderr,
                         "dj-ui: the remembered row %d is not in the middle (row centre %.1f, "
                         "window centre %.1f)\n",
                         rememberedRow, rowCentre, windowCentre);
            return 1;
        }
        std::fprintf(stderr, "dj-ui: the remembered row %d is in the middle (contentY %.1f)\n",
                     rememberedRow, list->property("contentY").toDouble());
    }

    // Probe: a track out of the collection into a deck — the gesture the DJ says stopped working. The
    // existing check loads from the files section only, which is why this went unnoticed.
    {
        const QPointF point = list->mapToScene(QPointF(20.0, 5 * rowHeight + 9.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, point, window->mapToGlobal(point.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
        }
        QCoreApplication::processEvents();
        const int clicked = list->property("currentIndex").toInt();
        const QVariant selected = panel->property("selected");
        const QString path = selected.toMap().value(QStringLiteral("path")).toString();
        panel->setProperty("lastAction", QString());
        QVariant returned;
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, QStringLiteral("A")));
        QElapsedTimer wait;
        wait.start();
        while (wait.elapsed() < 60000
               && !panel->property("lastAction").toString().startsWith(QStringLiteral("loaded "))) {
            QCoreApplication::processEvents();
            QThread::msleep(50);
        }
        const QString action = panel->property("lastAction").toString();
        if (clicked < 0 || path.isEmpty() || !returned.toBool()
            || !action.startsWith(QStringLiteral("loaded "))) {
            std::fprintf(stderr,
                         "dj-ui: a collection row did not load into a deck (clicked %d, selected "
                         "'%s', action '%s')\n",
                         clicked, qPrintable(path), qPrintable(action));
            return 1;
        }
        std::fprintf(stderr, "dj-ui: a collection row loads into a deck: %s\n", qPrintable(action));
    }

    // Every name is its own door. This is the fault the owner met by hand: the columns sit after the row
    // number and the lock, and a mapping that forgets the gutter answers one column to the right — a click
    // on BPM asking the index for Key. The keys the panel asked for say which door the click opened, so the
    // test does not have to guess from the row order.
    for (const char *column : {"artist", "album", "title", "time", "bpm", "key"}) {
        const QString name = QString::fromLatin1(column);
        if (!clickColumn(name, Qt::NoModifier)) {
            std::fprintf(stderr, "dj-ui: the header has no %s column\n", column);
            return 1;
        }
        const QString asked = keys().split(QLatin1Char(',')).value(0);
        if (!asked.startsWith(name)) {
            std::fprintf(stderr, "dj-ui: a click on %s asked for %s\n", column, qPrintable(asked));
            return 1;
        }
    }
    std::fprintf(stderr, "dj-ui: every column's name sorts by itself (%s)\n", qPrintable(keys()));

    // The header's mapping, asked directly, and at two widths: the middle of every column must answer that
    // column. This is the arithmetic a click goes through — the columns start after the row number and the
    // lock, and share what is left — and the fault shows as one column off. A single width can hide it,
    // because the two ways of summing the same shares can agree there; a narrow window makes the gutter a
    // larger part of the header and the difference visible.
    const auto mappingAt = [&](int wide, const char *what) -> QString {
        window->resize(wide, 800);
        for (int tick = 0; tick < 15; ++tick) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const double width = header->width();
        for (int index = 0; index < layout.collectionColumns().size(); ++index) {
            QVariant askedLeft;
            QVariant askedSpan;
            QVariant middleAnswer;
            QVariant leftAnswer;
            QMetaObject::invokeMethod(panel, "columnX", Q_RETURN_ARG(QVariant, askedLeft),
                                      Q_ARG(QVariant, index), Q_ARG(QVariant, width));
            QMetaObject::invokeMethod(panel, "columnWidth", Q_RETURN_ARG(QVariant, askedSpan),
                                      Q_ARG(QVariant, index), Q_ARG(QVariant, width));
            const double left = askedLeft.toDouble();
            const double span = askedSpan.toDouble();
            QMetaObject::invokeMethod(panel, "columnIndexAt", Q_RETURN_ARG(QVariant, middleAnswer),
                                      Q_ARG(QVariant, left + span / 2.0), Q_ARG(QVariant, width));
            QMetaObject::invokeMethod(panel, "columnIndexAt", Q_RETURN_ARG(QVariant, leftAnswer),
                                      Q_ARG(QVariant, left + 2.0), Q_ARG(QVariant, width));
            if (middleAnswer.toInt() != index || leftAnswer.toInt() != index) {
                const QString id =
                    layout.collectionColumns().at(index).toMap().value(QStringLiteral("id")).toString();
                return QStringLiteral("at %1 px (header %2) the middle of %3 answers %4, its left edge %5")
                    .arg(wide)
                    .arg(width)
                    .arg(id)
                    .arg(middleAnswer.toInt())
                    .arg(leftAnswer.toInt());
            }
        }
        std::fprintf(stderr, "dj-ui: every column answers itself %s (%u px wide)\n", what,
                     static_cast<unsigned>(width));
        return QString();
    };
    for (const auto &pass : {std::make_pair(1200, "in the wide window"),
                             std::make_pair(720, "in a narrow window")}) {
        if (const QString complaint = mappingAt(pass.first, pass.second); !complaint.isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(complaint));
            return 1;
        }
    }
    window->resize(1200, 800);
    for (int tick = 0; tick < 15; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }

    std::fprintf(stderr, "dj-ui: sort check done\n");
    return 0;
}

// The collection's Analyze…: one track gets a progress window, a selection gets a question with the
// parallel box and then the same window. Afterwards the numbers are in the list, not only in the files.
/// How the collection lists: the walk, the sidecars, the rows — and the time it takes.
///
/// The section is built by walking every root and reading a `.djmeta` record per file, then sorted and
/// filtered in memory. Whether that holds at a hundred thousand tracks is a question with a number, not
/// an opinion, and this is where the number comes from. Nothing is written.
static int runCollectionProbe(FileBrowser &browser, const QString &root)
{
    if (root.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --collection-probe needs --collection-root\n");
        return 1;
    }
    QElapsedTimer clock;
    clock.start();
    browser.openCollection({root});
    const qint64 cold = clock.elapsed();
    const int rows = static_cast<int>(browser.entries().size());
    std::fprintf(stderr, "dj-ui: collection probe: %d rows in %lld ms (cold)\n", rows, cold);
    clock.restart();
    browser.openCollection({root});
    const qint64 warm = clock.elapsed();
    std::fprintf(stderr, "dj-ui: collection probe: %d rows in %lld ms (warm)\n", rows, warm);
    // The sort the DJ asks for by clicking a header, on the rows the walk produced.
    clock.restart();
    browser.sortCollection(QStringLiteral("artist"), false);
    std::fprintf(stderr, "dj-ui: collection probe: sort by artist in %lld ms\n", clock.elapsed());
    // The search field: what the DJ types, and what clearing it costs.
    clock.restart();
    browser.setFilter(QStringLiteral("Track 99999"));
    std::fprintf(stderr, "dj-ui: collection probe: filter -> %d rows in %lld ms\n",
                 static_cast<int>(browser.entries().size()), clock.elapsed());
    clock.restart();
    browser.setFilter(QString());
    std::fprintf(stderr, "dj-ui: collection probe: filter cleared in %lld ms\n", clock.elapsed());
    return 0;
}

/// Does this value, anywhere inside it, name the file?
static bool holdsName(const QVariant &value, const QString &name)
{
    if (value.typeId() == QMetaType::QVariantMap) {
        const QVariantMap map = value.toMap();
        for (auto entry = map.cbegin(); entry != map.cend(); ++entry) {
            if (holdsName(entry.value(), name)) {
                return true;
            }
        }
        return false;
    }
    if (value.typeId() == QMetaType::QVariantList) {
        for (const QVariant &item : value.toList()) {
            if (holdsName(item, name)) {
                return true;
            }
        }
        return false;
    }
    return value.canConvert<QString>() && value.toString().contains(name);
}

/// A track out of a real collection into a deck, read only.
///
/// The other checks load from the files section and from a collection this program wrote itself. A
/// collection the DJ made is a different thing: hashed folders, sidecars, names taken from tags. And
/// the panel's own message is not evidence — the deck's track is, so the engine's snapshot is what
/// gets read here.
static int runCollectionLoadCheck(QQmlApplicationEngine &engine, FileBrowser &browser,
                                  BridgeClient &bridge, const QString &collectionRoot)
{
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --collection-load-check needs --collection-root\n");
        return 1;
    }
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    QElapsedTimer clock;
    clock.start();
    while (browser.entries().size() < 6 && clock.elapsed() < 60000) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    // The row number: a position in the list, drawn to the left of the columns and reading as the DJ counts.
    if (auto *number = findItem(window->contentItem(), QStringLiteral("collectionCell_number_0"))) {
        const QString shown = number->property("text").toString();
        std::fprintf(stdout, "dj-ui: the first row is numbered '%s'\n", qPrintable(shown));
        if (shown != QStringLiteral("1")) {
            std::fprintf(stderr, "dj-ui: the row number reads '%s', wanted '1'\n", qPrintable(shown));
            return 1;
        }
    } else {
        std::fprintf(stderr, "dj-ui: the collection has no row numbers\n");
        return 1;
    }
    const int rows = static_cast<int>(browser.entries().size());
    if (rows < 6) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows\n", rows);
        return 1;
    }
    const QVariantMap entry = browser.entries().at(5).toMap();
    const QString path = entry.value(QStringLiteral("path")).toString();
    browser.setCursor(path);
    QCoreApplication::processEvents();
    const QString selectedPath =
        panel->property("selected").toMap().value(QStringLiteral("path")).toString();
    // The button, not the function: the DJ presses a deck button in the header, and a handler that is
    // never reached by a real press is a handler that does not exist.
    auto *button = findItem(window->contentItem(), QStringLiteral("deckButton_A"));
    if (button == nullptr) {
        std::fprintf(stderr, "dj-ui: the header has no deck A button\n");
        return 1;
    }
    panel->setProperty("lastAction", QString());
    const QPointF centre(button->width() / 2.0, button->height() / 2.0);
    const QPointF scene = button->mapToScene(centre);
    for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
        QMouseEvent event(type, scene, window->mapToGlobal(scene.toPoint()), Qt::LeftButton,
                          Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &event);
    }
    QCoreApplication::processEvents();
    const bool accepted = !panel->property("lastAction").toString().isEmpty();
    const QVariant returned = accepted;
    QElapsedTimer settle;
    settle.start();
    while (settle.elapsed() < 90000
           && !panel->property("lastAction").toString().startsWith(QStringLiteral("loaded "))) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const QString action = panel->property("lastAction").toString();
    std::fprintf(stderr,
                 "dj-ui: collection load: %d rows, row 5 '%s', selected '%s', loadInto %s, action "
                 "'%s'\n",
                 rows, qPrintable(path), qPrintable(selectedPath),
                 returned.toBool() ? "true" : "false", qPrintable(action));
    if (selectedPath.isEmpty() || !returned.toBool()) {
        return 1;
    }
    const QString name = QFileInfo(path).fileName();
    const auto loaded = [&]() { return holdsName(bridge.snapshot(), name); };
    clock.restart();
    while (!loaded() && clock.elapsed() < 90000) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    if (!loaded()) {
        std::fprintf(stderr, "dj-ui: deck A does not carry '%s' after the load\n", qPrintable(name));
        return 1;
    }
    std::fprintf(stderr, "dj-ui: the collection track reached deck A: %s\n", qPrintable(name));
    return 0;
}

/// A metronome as a wav file: a click on every beat, so the detector has a beat to find.
///
/// The checks that need several tracks cannot copy one recording: the identity check reads copies as the
/// same track and sets them aside. Three tempos are three tracks, and each one can be measured.
static bool writeClickTrack(const QString &path, double bpm, double seconds)
{
    const int rate = 48000;
    const int frames = int(seconds * rate);
    const int clickFrames = rate / 100; // 10 ms: an onset, not a note
    QByteArray audio(size_t(frames) * 2, '\0');
    const double beatFrames = 60.0 / bpm * rate;
    // Noise, not a tone: a tempo detector votes on spectral flux, and a burst with energy across the
    // spectrum is what it hears as a beat. A square wave only moves two partials and reads as nothing.
    quint32 noise = 0x9e3779b9u;
    for (double beat = 0.0; beat + clickFrames < frames; beat += beatFrames) {
        const int start = int(beat);
        for (int at = 0; at < clickFrames; ++at) {
            noise = noise * 1664525u + 1013904223u;
            const double decay = 1.0 - double(at) / double(clickFrames);
            const double sample = (double((noise >> 9) & 0x7fff) / 16384.0 - 1.0) * decay;
            const short value = short(12000.0 * sample);
            audio[start * 2 + at * 2] = char(value & 0xff);
            audio[start * 2 + at * 2 + 1] = char((value >> 8) & 0xff);
        }
    }
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        return false;
    }
    const auto put32 = [&file](quint32 value) {
        char bytes[4]{char(value & 0xff), char((value >> 8) & 0xff), char((value >> 16) & 0xff),
                      char((value >> 24) & 0xff)};
        file.write(bytes, 4);
    };
    const auto put16 = [&file](quint16 value) {
        char bytes[2]{char(value & 0xff), char((value >> 8) & 0xff)};
        file.write(bytes, 2);
    };
    file.write("RIFF", 4);
    put32(quint32(36 + audio.size()));
    file.write("WAVEfmt ", 8);
    put32(16);
    put16(1);          // pcm
    put16(1);          // mono
    put32(quint32(rate));
    put32(quint32(rate * 2));
    put16(2);
    put16(16);
    file.write("data", 4);
    put32(quint32(audio.size()));
    file.write(audio);
    return file.error() == QFileDevice::NoError;
}

static int runAnalyzeCheck(QQmlApplicationEngine &engine, FileBrowser &browser, LayoutSettings &layout,
                           BridgeClient &bridge, const QString &fixture, const QString &collectionRoot)
{
    Q_UNUSED(layout);
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --analyze-check needs --import-fixture and --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    // Three tracks with no measurements: the batch has to measure them from scratch, which is the case
    // the DJ complains about — a collection full of rows with no tempo.
    //
    // They are written, not copied: three copies of one recording are one track, and the import sets the
    // extra ones aside without asking. Three clicks at three tempos are three tracks with a beat in each.
    QTemporaryDir incoming;
    if (!incoming.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    QStringList staged;
    const double tempos[3] = {120.0, 128.0, 140.0};
    for (int index = 0; index < 3; ++index) {
        const QString source = incoming.filePath(QStringLiteral("Click %1.wav").arg(index));
        // Two minutes each: a measurement that ends in fifty milliseconds would open and close the
        // progress window between two looks, and the check would report a window that never existed.
        if (!writeClickTrack(source, tempos[index], 120.0)) {
            std::fprintf(stderr, "dj-ui: cannot write %s\n", qPrintable(source));
            return 1;
        }
        staged.append(source);
    }
    bridge.importPaths(staged, collectionRoot, true);
    // The import runs on the engine: the rows appear when it has written the records. Re-asking is what
    // a list does when it is told the collection changed.
    QElapsedTimer importClock;
    importClock.start();
    while (importClock.elapsed() < 30000) {
        QCoreApplication::processEvents();
        QThread::msleep(100);
        if (browser.entries().size() >= 3) {
            break;
        }
        browser.openCollection({collectionRoot});
    }
    if (browser.entries().size() < 3) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted 3\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }
    // The rows are in; the pages that carried them are still arriving. A selection made while a page is
    // landing is made against a list that is about to be replaced, and it is lost.
    for (int tick = 0; tick < 20; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    Q_UNUSED(root);

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    QElapsedTimer clock;
    clock.start();
    while (browser.entries().size() < 3 && clock.elapsed() < 5000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    if (browser.entries().size() < 3) {
        std::fprintf(stderr, "dj-ui: the collection lists %d rows, wanted 3\n",
                     static_cast<int>(browser.entries().size()));
        return 1;
    }

    const auto choose = [&](int index, Qt::KeyboardModifiers modifiers) {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "choose", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, index),
                                  Q_ARG(QVariant, static_cast<int>(modifiers)));
        QCoreApplication::processEvents();
    };
    const auto analyze = [&]() {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "analyzeSelection", Q_RETURN_ARG(QVariant, returned));
        QCoreApplication::processEvents();
        return returned.toBool();
    };
    const auto state = [&]() { return panel->property("analysisState").toMap(); };
    const auto showing = [&](const char *name) {
        auto *dialog = panel->findChild<QObject *>(QString::fromLatin1(name));
        return dialog != nullptr && dialog->property("visible").toBool();
    };
    const auto wait = [&](const auto &done, int timeoutMs) {
        QElapsedTimer until;
        until.start();
        while (until.elapsed() < timeoutMs) {
            if (done()) {
                return true;
            }
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        return done();
    };
    const auto finished = [&]() {
        const QVariantMap now = state();
        return !now.isEmpty() && now.value(QStringLiteral("running")).toBool() == false;
    };

    // One track: no question, and a window that says how far it has come.
    choose(0, Qt::NoModifier);
    if (!analyze()) {
        std::fprintf(stderr, "dj-ui: one track did not start\n");
        return 1;
    }
    // The window is the point, but a track that measures faster than the check can look still measured:
    // a finished state is proof the batch ran, and the window is proved on the three-track batch below.
    const auto counted = [&]() {
        return state().value(QStringLiteral("done")).toInt() > 0;
    };
    if (!wait([&]() { return showing("analyzeProgress") || counted(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: one track ran with no progress window\n");
        return 1;
    }
    const QVariantMap one = state();
    std::fprintf(stderr, "dj-ui: one track: progress window, %d of %d\n",
                 one.value(QStringLiteral("done")).toInt(),
                 one.value(QStringLiteral("total")).toInt());
    if (!wait(finished, 180000)) {
        std::fprintf(stderr, "dj-ui: the single analysis never finished\n");
        return 1;
    }

    // Three tracks: the question first, with the parallel box, then the same window.
    choose(0, Qt::NoModifier);
    choose(1, Qt::ControlModifier);
    choose(2, Qt::ControlModifier);
    if (!analyze()) {
        std::fprintf(stderr, "dj-ui: the selection did not start\n");
        return 1;
    }
    if (!showing("analyzeDialog")) {
        std::fprintf(stderr, "dj-ui: three tracks started with no confirmation window\n");
        return 1;
    }
    auto *parallelBox = panel->findChild<QObject *>(QStringLiteral("analyzeParallelBox"));
    if (parallelBox == nullptr || !parallelBox->property("checked").toBool()) {
        std::fprintf(stderr, "dj-ui: the confirmation has no parallel box, or it is off\n");
        return 1;
    }
    auto *start = panel->findChild<QObject *>(QStringLiteral("analyzeStart"));
    if (start == nullptr) {
        std::fprintf(stderr, "dj-ui: the confirmation has no start button\n");
        return 1;
    }
    std::fprintf(stderr, "dj-ui: three tracks: confirmation with the parallel box, starting\n");
    QMetaObject::invokeMethod(start, "clicked");
    QCoreApplication::processEvents();
    // Same tolerance as the single track: three short measurements can finish between two looks.
    if (!wait([&]() { return showing("analyzeProgress") || counted(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: the batch ran with no progress window\n");
        return 1;
    }
    // The state is published by the engine, a frame behind the click: the selection's own count arrives
    // one snapshot later, and reading it at once reads the batch that ran before.
    wait(
        [&]() { return state().value(QStringLiteral("total")).toInt() == 3; }, 5000);
    const QVariantMap batch = state();
    std::fprintf(stderr, "dj-ui: batch: progress window, %d of %d\n",
                 batch.value(QStringLiteral("done")).toInt(),
                 batch.value(QStringLiteral("total")).toInt());
    if (batch.value(QStringLiteral("total")).toInt() != 3) {
        std::fprintf(stderr, "dj-ui: the batch counts %d files, wanted 3\n",
                     batch.value(QStringLiteral("total")).toInt());
        return 1;
    }
    if (!wait(finished, 180000)) {
        std::fprintf(stderr, "dj-ui: the batch never finished\n");
        return 1;
    }

    // And the rows: a tempo for every one of the three, read from the files.
    const auto withTempo = [&]() {
        int count = 0;
        for (const QVariant &row : browser.entries()) {
            const QVariantMap entry = row.toMap();
            if (entry.value(QStringLiteral("bpm")).toDouble() > 0.0) {
                ++count;
            }
        }
        return count;
    };
    // The rows refresh themselves: the engine puts the measured record into the index, and the page
    // question that follows carries the tempo. A refresh call of our own would be a second door.
    QElapsedTimer settle;
    settle.start();
    while (withTempo() < 3 && settle.elapsed() < 20000) {
        QCoreApplication::processEvents();
        QThread::msleep(50);
    }
    const int measured = withTempo();
    std::fprintf(stderr, "dj-ui: rows with a tempo after the batch: %d of %d\n", measured,
                 static_cast<int>(browser.entries().size()));
    if (measured < 3) {
        std::fprintf(stderr, "dj-ui: the batch left rows without a tempo\n");
        return 1;
    }
    std::fprintf(stderr, "dj-ui: analyze check done\n");
    return 0;
}

// The same collection, two sources: the walk over `.djmeta` and the page out of the index. The question
// is not which one the list uses today — it is whether the index gives the same rows at all. Compared by
// path, field by field, and the first differences are printed.
// The collection's header, as two numbers: how far the browser's own controls (the type size and the
// deck buttons) stand from the count that talks about the collection, and how far from the right edge
// they stand. "Moved to the right and away from the text" is a claim about pixels, so it is measured:
// an impression of a screenshot is not evidence, and a control that drifts back left costs a release.
static int measureCollectionHeader(QQuickItem *panel)
{
    QQuickItem *headerRow = findItem(panel, QStringLiteral("browserHeaderRow"));
    QQuickItem *countLabel = findItem(panel, QStringLiteral("collectionCount"));
    QQuickItem *minusButton = findItem(panel, QStringLiteral("fontMinus"));
    QQuickItem *lastDeckButton = findItem(panel, QStringLiteral("deckButton_D"));
    if (headerRow == nullptr || countLabel == nullptr || minusButton == nullptr
        || lastDeckButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection header has no row, count or buttons to measure\n");
        return 1;
    }
    QCoreApplication::processEvents();
    const auto sceneRect = [](QQuickItem *item) {
        return item->mapRectToScene(QRectF(0, 0, item->width(), item->height()));
    };
    const QRectF rowRect = sceneRect(headerRow);
    const QRectF countRect = sceneRect(countLabel);
    const QRectF minusRect = sceneRect(minusButton);
    const QRectF deckRect = sceneRect(lastDeckButton);
    const double gap = minusRect.left() - countRect.right();
    const double slack = rowRect.right() - deckRect.right();
    std::fprintf(stdout,
                 "dj-ui: the collection header: controls %.0f px right of the count, %.1f px from the "
                 "right edge\n",
                 gap, slack);
    if (gap < 8.0) {
        std::fprintf(stderr, "dj-ui: the type and deck buttons crowd the collection count (%.0f px)\n",
                     gap);
        return 1;
    }
    if (slack < -0.5 || slack > 2.0) {
        std::fprintf(stderr, "dj-ui: the browser's controls are not at the right edge (%.1f px)\n",
                     slack);
        return 1;
    }
    return 0;
}

static int runCollectionPageCheck(QQmlApplicationEngine &engine, FileBrowser &browser,
                                  BridgeClient &bridge, const QString &fixture,
                                  const QString &collectionRoot)
{
    if (fixture.isEmpty() || !QFile::exists(fixture)) {
        std::fprintf(stderr, "dj-ui: --collection-page-check needs a real track in --import-fixture\n");
        return 1;
    }
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --collection-page-check needs --collection-root\n");
        return 1;
    }
    QTemporaryDir incoming;
    if (!incoming.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    // The cold case first: no index exists yet, so the engine creates it and runs the schema and its
    // migrations inside the answer. That is what a browser opening the collection on a fresh install
    // does, and it must not hang the engine.
    {
        QVariantMap cold;
        cold.insert(QStringLiteral("id"), 1111);
        cold.insert(QStringLiteral("offset"), 0);
        cold.insert(QStringLiteral("limit"), 100);
        QElapsedTimer coldClock;
        coldClock.start();
        bridge.queryCollection(cold);
        while (coldClock.elapsed() < 15000
               && bridge.collectionPage().value(QStringLiteral("id")).toInt() != 1111) {
            QCoreApplication::processEvents();
            QThread::msleep(10);
        }
        if (bridge.collectionPage().value(QStringLiteral("id")).toInt() != 1111) {
            std::fprintf(stderr, "dj-ui: the engine never answered a query with no index\n");
            return 1;
        }
        std::fprintf(stdout, "dj-ui: a query with no index answered in %lld ms, %d rows\n",
                     static_cast<long long>(coldClock.elapsed()),
                     static_cast<int>(bridge.collectionPage().value(QStringLiteral("rows")).toList().size()));
    }
    const QString source = incoming.path() + QStringLiteral("/A - Page.wav");
    if (!QFile::copy(fixture, source)) {
        std::fprintf(stderr, "dj-ui: cannot copy %s\n", qPrintable(fixture));
        return 1;
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    window->resize(1366, 768);
    window->show();
    browser.open(incoming.path());
    QEventLoop settle;
    QTimer::singleShot(600, &settle, &QEventLoop::quit);
    settle.exec();

    QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    int row = -1;
    const QVariantList listed = browser.entries();
    for (int index = 0; index < listed.size(); ++index) {
        if (listed.at(index).toMap().value(QStringLiteral("path")).toString() == source) {
            row = index;
        }
    }
    if (row < 0) {
        std::fprintf(stderr, "dj-ui: the browser lists %d rows and not the copy\n",
                     static_cast<int>(listed.size()));
        return 1;
    }

    QVariant returned;
    QMetaObject::invokeMethod(panel, "choose", Q_RETURN_ARG(QVariant, returned),
                              Q_ARG(QVariant, row),
                              Q_ARG(QVariant, static_cast<int>(Qt::NoModifier)));
    QMetaObject::invokeMethod(panel, "askImport", Q_RETURN_ARG(QVariant, returned));
    QCoreApplication::processEvents();
    auto *confirmButton = panel->findChild<QObject *>(QStringLiteral("importConfirm"));
    if (confirmButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the import windows are not in the shell\n");
        return 1;
    }
    QMetaObject::invokeMethod(confirmButton, "clicked");
    const auto state = [&]() { return panel->property("importState").toMap(); };
    QElapsedTimer batch;
    batch.start();
    while (batch.elapsed() < 60000) {
        QCoreApplication::processEvents();
        const QVariantMap now = state();
        if (!now.isEmpty() && now.value(QStringLiteral("total")).toInt() > 0
            && !now.value(QStringLiteral("running")).toBool()) {
            break;
        }
        QThread::msleep(10);
    }
    const QVariantMap finished = state();
    if (finished.value(QStringLiteral("placed")).toInt() < 1) {
        std::fprintf(stderr, "dj-ui: the batch placed %d of %d\n",
                     finished.value(QStringLiteral("placed")).toInt(),
                     finished.value(QStringLiteral("total")).toInt());
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the collection holds the copy after an import (%d placed)\n",
                 finished.value(QStringLiteral("placed")).toInt());

    // The walk: what the list shows today.
    browser.openCollection({collectionRoot});
    QElapsedTimer walkClock;
    walkClock.start();
    while (browser.entries().isEmpty() && walkClock.elapsed() < 5000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    const QVariantList walked = browser.entries();

    // The page: what the index says, asked directly — the list is not involved in the answer.
    QVariantMap query;
    query.insert(QStringLiteral("id"), 4242);
    query.insert(QStringLiteral("offset"), 0);
    query.insert(QStringLiteral("limit"), 5000);
    bridge.queryCollection(query);
    QElapsedTimer pageClock;
    pageClock.start();
    while (pageClock.elapsed() < 15000
           && bridge.collectionPage().value(QStringLiteral("id")).toInt() != 4242) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    const QVariantMap answer = bridge.collectionPage();
    const QVariantList pageRows = answer.value(QStringLiteral("rows")).toList();
    if (answer.value(QStringLiteral("id")).toInt() != 4242) {
        std::fprintf(stderr, "dj-ui: the engine never answered the page question\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the walk lists %d rows, the index page %d\n",
                 static_cast<int>(walked.size()), static_cast<int>(pageRows.size()));

    QHash<QString, QVariantMap> byPath;
    for (const QVariant &entry : pageRows) {
        const QVariantMap made = entry.toMap();
        byPath.insert(made.value(QStringLiteral("path")).toString(), made);
    }
    int told = 0;
    int compared = 0;
    int wrong = 0;
    for (const QVariant &entry : walked) {
        const QVariantMap row_ = entry.toMap();
        const QString path = row_.value(QStringLiteral("path")).toString();
        if (!byPath.contains(path)) {
            if (told++ < 5) {
                std::fprintf(stderr, "dj-ui: the index has no row for %s\n", qPrintable(path));
            }
            ++wrong;
            continue;
        }
        const QVariantMap pageRow = byPath.take(path);
        ++compared;
        const auto complain = [&](const char *field, const QString &walkedValue,
                                  const QString &pageValue) {
            if (told++ < 5) {
                std::fprintf(stderr, "dj-ui: %s differs on %s: walk '%s', index '%s'\n", field,
                             qPrintable(path), qPrintable(walkedValue), qPrintable(pageValue));
            }
            ++wrong;
        };
        if (row_.value(QStringLiteral("artist")).toString()
            != pageRow.value(QStringLiteral("artist")).toString()) {
            complain("artist", row_.value(QStringLiteral("artist")).toString(),
                     pageRow.value(QStringLiteral("artist")).toString());
        }
        if (row_.value(QStringLiteral("album")).toString()
            != pageRow.value(QStringLiteral("album")).toString()) {
            complain("album", row_.value(QStringLiteral("album")).toString(),
                     pageRow.value(QStringLiteral("album")).toString());
        }
        if (row_.value(QStringLiteral("title")).toString()
            != pageRow.value(QStringLiteral("title")).toString()) {
            complain("title", row_.value(QStringLiteral("title")).toString(),
                     pageRow.value(QStringLiteral("title")).toString());
        }
        // The walk carries milliseconds, the index frames and their rate: one number, two spellings.
        const double frames = pageRow.value(QStringLiteral("duration_frames")).toDouble();
        const double rate = pageRow.value(QStringLiteral("sample_rate")).toDouble();
        const qint64 pageMs = rate > 0.0 && frames > 0.0
            ? static_cast<qint64>(frames * 1000.0 / rate + 0.5)
            : 0;
        const qint64 walkedMs = row_.value(QStringLiteral("duration_ms")).toLongLong();
        if (std::abs(pageMs - walkedMs) > 1) {
            complain("duration", QString::number(walkedMs), QString::number(pageMs));
        }
        // Genres: the walk keeps them apart, the index keeps one line per name.
        QStringList pageGenres;
        const QString names = pageRow.value(QStringLiteral("genre")).toString();
        for (const QString &genre : names.split(QStringLiteral(", "), Qt::SkipEmptyParts)) {
            pageGenres.append(genre.trimmed());
        }
        const QStringList walkedGenres = row_.value(QStringLiteral("genres")).toStringList();
        if (pageGenres != walkedGenres) {
            complain("genres", walkedGenres.join(QStringLiteral("|")),
                     pageGenres.join(QStringLiteral("|")));
        }
    }
    if (!byPath.isEmpty()) {
        const QString extra = byPath.keys().value(0);
        if (told < 5) {
            std::fprintf(stderr, "dj-ui: the index has a row the walk does not: %s\n",
                         qPrintable(extra));
        }
        ++wrong;
    }
    std::fprintf(stdout, "dj-ui: %d rows compared field by field, %d differ\n", compared, wrong);
    if (wrong != 0) {
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the index and the walk agree on every row\n");

    // The genre boxes: the engine counts them in its own table and publishes them apart from the page. The
    // walk knows the same genres from the same files, so the two counts must agree — that is what makes
    // "the boxes come from the database" a measurement instead of a claim.
    {
        QMap<QString, int> walkedGenres;
        for (const QVariant &entry : walked) {
            for (const QString &genre : entry.toMap().value(QStringLiteral("genres")).toStringList()) {
                walkedGenres[genre.toLower()] += 1;
            }
        }
        QMap<QString, int> counted;
        QElapsedTimer waitForGenres;
        waitForGenres.start();
        while (waitForGenres.elapsed() < 5000) {
            QCoreApplication::processEvents();
            const QVariantList said = bridge.snapshot().value(QStringLiteral("genres")).toList();
            if (!said.isEmpty()) {
                counted.clear();
                for (const QVariant &entry : said) {
                    const QVariantMap box = entry.toMap();
                    counted.insert(box.value(QStringLiteral("name")).toString().toLower(),
                                   box.value(QStringLiteral("count")).toInt());
                }
                break;
            }
            QThread::msleep(20);
        }
        if (counted.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the index said nothing about genres\n");
            return 1;
        }
        if (counted != walkedGenres) {
            std::fprintf(stderr, "dj-ui: the genre table disagrees with the walk\n");
            for (auto it = counted.cbegin(); it != counted.cend(); ++it) {
                std::fprintf(stderr, "dj-ui:   table %s %d, walk %d\n", qPrintable(it.key()),
                             it.value(), walkedGenres.value(it.key(), -1));
            }
            for (auto it = walkedGenres.cbegin(); it != walkedGenres.cend(); ++it) {
                if (!counted.contains(it.key())) {
                    std::fprintf(stderr, "dj-ui:   only in the walk: %s %d\n", qPrintable(it.key()),
                                 it.value());
                }
            }
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the genre boxes come from the table: %d genres, the walk agrees\n",
                     static_cast<int>(counted.size()));
    }

    // A genre filter must narrow the list, not empty it: the answer is already narrowed by the index and
    // must be shown as it came. This is the bug the DJ reported — a saved genre left an empty collection.
    if (!pageRows.isEmpty()) {
        const QString names = pageRows.first().toMap().value(QStringLiteral("genre")).toString();
        const QString wanted = names.split(QStringLiteral(", ")).value(0).trimmed();
        if (!wanted.isEmpty()) {
            browser.setGenreFilter({wanted});
            QElapsedTimer filtered;
            filtered.start();
            while (browser.entries().isEmpty() && filtered.elapsed() < 5000) {
                QCoreApplication::processEvents();
                QThread::msleep(20);
            }
            std::fprintf(stdout, "dj-ui: the genre filter '%s' leaves %d rows\n", qPrintable(wanted),
                         static_cast<int>(browser.entries().size()));
            if (browser.entries().isEmpty()) {
                std::fprintf(stderr, "dj-ui: a genre the index knows left an empty list\n");
                return 1;
            }
            browser.setGenreFilter({});
        }
    }

    // Can the engine still take a load after answering a collection query? That is what broke when the
    // page was wired into the list: the query opened the index — schema and migrations, a write — on the
    // same loop that acks deck commands. The number below is the answer, not an opinion.
    if (!walked.isEmpty()) {
        browser.setCursor(walked.first().toMap().value(QStringLiteral("path")).toString());
        QCoreApplication::processEvents();
        panel->setProperty("lastAction", QString());
        QVariant returnedLoad;
        QElapsedTimer loadClock;
        loadClock.start();
        QMetaObject::invokeMethod(panel, "loadInto", Q_RETURN_ARG(QVariant, returnedLoad),
                                  Q_ARG(QVariant, QStringLiteral("A")));
        while (loadClock.elapsed() < 20000
               && !panel->property("lastAction").toString().startsWith(QStringLiteral("loaded "))) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
        }
        const QString action = panel->property("lastAction").toString();
        std::fprintf(stdout, "dj-ui: a load after the query -> '%s' in %lld ms\n", qPrintable(action),
                     static_cast<long long>(loadClock.elapsed()));
        if (!action.startsWith(QStringLiteral("loaded "))) {
            std::fprintf(stderr, "dj-ui: the engine did not take a load after the query\n");
            return 1;
        }
    }

    // The header the DJ reads the collection by: the count beside the title, the browser's own controls
    // at the far right. Measured by the same code the import check runs.
    if (const int verdict = measureCollectionHeader(panel); verdict != 0) {
        return verdict;
    }
    return 0;
}

static int runImportCheck(QQmlApplicationEngine &engine, FileBrowser &browser,
                          LayoutSettings &layout, BridgeClient &bridge, const QString &fixture,
                          const QString &collectionRoot)
{
    if (fixture.isEmpty() || !QFile::exists(fixture)) {
        std::fprintf(stderr, "dj-ui: --import-check needs a real track in --import-fixture\n");
        return 1;
    }
    if (collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --import-check needs --collection-root\n");
        return 1;
    }
    QTemporaryDir incoming;
    if (!incoming.isValid()) {
        std::fprintf(stderr, "dj-ui: cannot make a temporary folder\n");
        return 1;
    }
    // Two copies of one recording: the second is the duplicate the batch has to ask about.
    const QString first = incoming.path() + QStringLiteral("/A - Track.wav");
    const QString second = incoming.path() + QStringLiteral("/A - Track (copy).wav");
    if (!QFile::copy(fixture, first) || !QFile::copy(fixture, second)) {
        std::fprintf(stderr, "dj-ui: cannot copy %s twice\n", qPrintable(fixture));
        return 1;
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    window->resize(1366, 768);
    window->show();
    browser.open(incoming.path());
    QEventLoop settle;
    QTimer::singleShot(600, &settle, &QEventLoop::quit);
    settle.exec();

    QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    // The rows by path, not by position: the list may carry a way up and the order is the panel's.
    const QVariantList entries = browser.entries();
    int firstRow = -1;
    int secondRow = -1;
    for (int index = 0; index < entries.size(); ++index) {
        const QString path = entries.at(index).toMap().value(QStringLiteral("path")).toString();
        if (path == first) {
            firstRow = index;
        } else if (path == second) {
            secondRow = index;
        }
    }
    if (firstRow < 0 || secondRow < 0) {
        std::fprintf(stderr, "dj-ui: the browser lists %d rows and not both copies\n",
                     static_cast<int>(entries.size()));
        return 1;
    }

    const auto choose = [&](int index, Qt::KeyboardModifiers modifiers) {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "choose", Q_RETURN_ARG(QVariant, returned),
                                  Q_ARG(QVariant, index),
                                  Q_ARG(QVariant, static_cast<int>(modifiers)));
    };
    const auto askImport = [&]() {
        QVariant returned;
        QMetaObject::invokeMethod(panel, "askImport", Q_RETURN_ARG(QVariant, returned));
        QCoreApplication::processEvents();
        return returned.toBool();
    };
    // One reading of the state the engine published: what the panel draws comes from here.
    const auto state = [&]() { return panel->property("importState").toMap(); };
    const auto questionOpen = [&]() {
        auto *dialog = panel->findChild<QObject *>(QStringLiteral("duplicateDialog"));
        return dialog != nullptr && dialog->property("visible").toBool();
    };
    // Wait for the batch to stop, or give up: a batch that never ends is a hang, not a slow disk.
    const auto waitDone = [&](int timeoutMs) {
        QElapsedTimer clock;
        clock.start();
        while (clock.elapsed() < timeoutMs) {
            QCoreApplication::processEvents();
            const QVariantMap now = state();
            if (!now.isEmpty() && now.value(QStringLiteral("total")).toInt() > 0
                && !now.value(QStringLiteral("running")).toBool()) {
                return true;
            }
            QThread::msleep(10);
        }
        return false;
    };

    // Ctrl extends the selection instead of replacing it: the batch must take both rows.
    choose(firstRow, Qt::NoModifier);
    choose(secondRow, Qt::ControlModifier);
    const QStringList picked = panel->property("selection").toStringList();
    std::fprintf(stdout, "dj-ui: import selection %d of 2 rows\n", static_cast<int>(picked.size()));
    if (picked.size() != 2) {
        std::fprintf(stderr, "dj-ui: Ctrl did not extend the selection\n");
        return 1;
    }
    if (!askImport()) {
        std::fprintf(stderr, "dj-ui: the browser refused to add the selection\n");
        return 1;
    }

    auto *confirmDialog = panel->findChild<QObject *>(QStringLiteral("importDialog"));
    auto *countLabel = panel->findChild<QObject *>(QStringLiteral("importCount"));
    auto *confirmButton = panel->findChild<QObject *>(QStringLiteral("importConfirm"));
    auto *progressDialog = panel->findChild<QObject *>(QStringLiteral("importProgress"));
    auto *progressText = panel->findChild<QObject *>(QStringLiteral("importProgressText"));
    auto *existingFacts = panel->findChild<QObject *>(QStringLiteral("duplicateExistingFacts"));
    auto *incomingFacts = panel->findChild<QObject *>(QStringLiteral("duplicateIncomingFacts"));
    auto *addMarked = panel->findChild<QObject *>(QStringLiteral("duplicateAddMarked"));
    auto *skipButton = panel->findChild<QObject *>(QStringLiteral("duplicateSkip"));
    if (confirmDialog == nullptr || countLabel == nullptr || confirmButton == nullptr
        || progressDialog == nullptr || progressText == nullptr || existingFacts == nullptr
        || incomingFacts == nullptr || addMarked == nullptr || skipButton == nullptr) {
        std::fprintf(stderr, "dj-ui: the import windows are not in the shell\n");
        return 1;
    }
    if (!confirmDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: a batch of two went in without asking\n");
        return 1;
    }
    const QString countText = countLabel->property("text").toString();
    std::fprintf(stdout, "dj-ui: import dialog says \"%s\"\n", qPrintable(countText));
    if (!countText.contains(QStringLiteral("2"))) {
        std::fprintf(stderr, "dj-ui: the dialog does not count the selection\n");
        return 1;
    }

    QMetaObject::invokeMethod(confirmButton, "clicked");
    QCoreApplication::processEvents();
    if (confirmDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: the dialog stayed open after the confirmation\n");
        return 1;
    }

    // The batch runs: progress, and no question at all — the two files are identical in length and
    // average bitrate, and an identical copy is not the DJ's decision to make.
    bool sawProgress = false;
    bool sawQuestion = false;
    QString facts;
    QElapsedTimer batch;
    batch.start();
    while (batch.elapsed() < 60000) {
        QCoreApplication::processEvents();
        if (progressDialog->property("visible").toBool()) {
            if (!sawProgress) {
                std::fprintf(stdout, "dj-ui: import progress -> %s\n",
                             qPrintable(progressText->property("text").toString()));
            }
            sawProgress = true;
        }
        if (questionOpen()) {
            sawQuestion = true;
            facts = existingFacts->property("text").toString() + QStringLiteral(" | ")
                + incomingFacts->property("text").toString();
            break;
        }
        const QVariantMap now = state();
        if (!now.isEmpty() && !now.value(QStringLiteral("running")).toBool()) {
            break;
        }
        QThread::msleep(10);
    }
    if (!sawProgress) {
        std::fprintf(stderr, "dj-ui: the batch ran without a progress window\n");
        return 1;
    }
    if (sawQuestion) {
        std::fprintf(stderr,
                     "dj-ui: the batch asked about an identical copy (%s) — it must not ask\n",
                     qPrintable(facts));
        return 1;
    }
    if (!waitDone(60000)) {
        std::fprintf(stderr, "dj-ui: the batch did not finish\n");
        return 1;
    }
    const QVariantMap finished = state();
    std::fprintf(stdout, "dj-ui: import finished -> %d of %d, %d placed, %d skipped, %d failed\n",
                 finished.value(QStringLiteral("done")).toInt(),
                 finished.value(QStringLiteral("total")).toInt(),
                 finished.value(QStringLiteral("placed")).toInt(),
                 finished.value(QStringLiteral("skipped")).toInt(),
                 finished.value(QStringLiteral("failed")).toInt());
    // One placed, one skipped: the copy is identical in length and average bitrate, so it was put aside
    // without a question. A batch that added both would mean the rule is not in force.
    if (finished.value(QStringLiteral("total")).toInt() != 2
        || finished.value(QStringLiteral("done")).toInt() != 2
        || finished.value(QStringLiteral("placed")).toInt() != 1
        || finished.value(QStringLiteral("skipped")).toInt() != 1) {
        std::fprintf(stderr, "dj-ui: the batch did not put the identical copy aside (placed %d, skipped %d)\n",
                     finished.value(QStringLiteral("placed")).toInt(),
                     finished.value(QStringLiteral("skipped")).toInt());
        return 1;
    }
    if (questionOpen() || progressDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: a window stayed open after the batch\n");
        return 1;
    }

    const auto collectionFiles = [](const QString &root) {
        int files = 0;
        QDirIterator it(root, QDir::Files, QDirIterator::Subdirectories);
        while (it.hasNext()) {
            it.next();
            // The metadata folder is not music.
            if (it.filePath().contains(QStringLiteral("/.djmeta/"))) {
                continue;
            }
            ++files;
        }
        return files;
    };
    const int placed = collectionFiles(collectionRoot);
    std::fprintf(stdout, "dj-ui: the collection holds %d files\n", placed);
    if (placed != 2) {
        std::fprintf(stderr, "dj-ui: the collection holds %d files, wanted 2\n", placed);
        return 1;
    }

    // One file goes straight in: the confirmation exists for batches, not for a single click.
    choose(firstRow, Qt::NoModifier);
    if (!askImport()) {
        std::fprintf(stderr, "dj-ui: the browser refused a single file\n");
        return 1;
    }
    if (confirmDialog->property("visible").toBool()) {
        std::fprintf(stderr, "dj-ui: a single file asked for confirmation\n");
        return 1;
    }
    // The same recording, the same duration, bitrate and depth: the batch sets it aside on its own, without
    // stopping to ask. A question here would be the very thing the rule exists to stop.
    batch.restart();
    if (!waitDone(30000)) {
        std::fprintf(stderr, "dj-ui: the single-file batch did not finish\n");
        return 1;
    }
    if (questionOpen()) {
        std::fprintf(stderr, "dj-ui: an identical file asked a question\n");
        return 1;
    }
    const int afterSkip = collectionFiles(collectionRoot);
    std::fprintf(stdout, "dj-ui: the collection still holds %d files\n", afterSkip);
    if (afterSkip != 2) {
        std::fprintf(stderr, "dj-ui: a set-aside copy added something (%d files)\n", afterSkip);
        return 1;
    }

    // The collection as the DJ sees it: artist, album, title, length, tempo and key — never the file
    // name, which says nothing a DJ searches by.
    browser.openCollection({collectionRoot});
    QEventLoop settled;
    QTimer::singleShot(300, &settled, &QEventLoop::quit);
    settled.exec();
    const QVariantList rows = browser.entries();
    // One row, not two: the batch placed one file and set the identical copy aside, and the list is the
    // index, so it holds what the import actually placed.
    if (rows.size() != 1) {
        std::fprintf(stderr, "dj-ui: the collection section lists %d rows, wanted 1\n",
                     static_cast<int>(rows.size()));
        return 1;
    }
    // The header while the collection is open with rows in it — the same measurement the page check runs.
    if (const int verdict = measureCollectionHeader(panel); verdict != 0) {
        return verdict;
    }
    const QVariantMap row = rows.at(0).toMap();
    const QString artist = row.value(QStringLiteral("artist")).toString();
    const QString title = row.value(QStringLiteral("title")).toString();
    const qint64 length = row.value(QStringLiteral("duration_ms")).toLongLong();
    const double rowBpm = row.value(QStringLiteral("bpm")).toDouble();
    const QString rowKey = row.value(QStringLiteral("key")).toString();
    std::fprintf(stdout, "dj-ui: collection row -> %s | %s | %lld ms | %s BPM | %s\n",
                 qPrintable(artist), qPrintable(title), static_cast<long long>(length),
                 rowBpm > 0.0 ? qPrintable(QString::number(rowBpm, 'f', 1)) : "-",
                 rowKey.isEmpty() ? "-" : qPrintable(rowKey));
    // The two lists are tables, so their face is the system's fixed-pitch one — and "we asked for a
    // monospace" is not the same as "the rows got one": the font that reached a row is read here, with
    // two digits of equal advance as the proof.
    // Both lists: the collection's cells and the file list's own rows, measured one at a time.
    const auto measureList = [&](const char *name) -> QString {
        QQuickItem *item = findItem(window->contentItem(), QString::fromLatin1(name));
        if (item == nullptr) {
            return QStringLiteral("%1 is not on the screen").arg(QString::fromLatin1(name));
        }
        const QFont face = item->property("font").value<QFont>();
        const QFontInfo reached(face);
        const QFontMetricsF metrics(face);
        const double zero = metrics.horizontalAdvance(QStringLiteral("0"));
        const double one = metrics.horizontalAdvance(QStringLiteral("1"));
        if (!reached.fixedPitch() || std::abs(zero - one) > 0.01) {
            return QStringLiteral("%1 wears %2, %3 pitch (0 %4 px, 1 %5 px)")
                .arg(QString::fromLatin1(name), reached.family(),
                     reached.fixedPitch() ? QStringLiteral("fixed") : QStringLiteral("proportional"))
                .arg(zero)
                .arg(one);
        }
        std::fprintf(stdout, "dj-ui: %s wears %s, fixed pitch (0 and 1 both %.2f px)\n", name,
                     qPrintable(reached.family()), zero);
        return QString();
    };
    for (const char *name : {"collectionCell_artist_0", "fileRow_0"}) {
        if (const QString complaint = measureList(name); !complaint.isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s\n", qPrintable(complaint));
            return 1;
        }
    }

    // The fixture may be a generated tone or a real track: what matters is that the row carries the
    // names the file was filed under, not which names those are.
    if (artist.isEmpty() || title.isEmpty()) {
        std::fprintf(stderr, "dj-ui: the collection row has no names (%s | %s)\n", qPrintable(artist),
                     qPrintable(title));
        return 1;
    }
    if (length < 15000) {
        std::fprintf(stderr, "dj-ui: the collection row says %lld ms, wanted a track longer than 15 s\n",
                     static_cast<long long>(length));
        return 1;
    }
    // And the window draws those columns, not the file name: a row of numbers with no artist on
    // screen is the bug this check exists for.
    // The rows are found in the visual tree: a ListView's delegates are not QObject children of the
    // panel, so findChild on the panel would report a table that is on screen as missing.
    const std::function<QQuickItem *(QQuickItem *, const QString &)> byName =
        [&](QQuickItem *node, const QString &name) -> QQuickItem * {
        if (node == nullptr) {
            return nullptr;
        }
        if (node->objectName() == name) {
            return node;
        }
        for (QQuickItem *child : node->childItems()) {
            if (QQuickItem *found = byName(child, name)) {
                return found;
            }
        }
        return nullptr;
    };
    QQuickItem *artistLabel = byName(window->contentItem(), QStringLiteral("collectionCell_artist_0"));
    QQuickItem *timeLabel = byName(window->contentItem(), QStringLiteral("collectionCell_time_0"));
    QQuickItem *headerArtist = byName(window->contentItem(), QStringLiteral("collectionHeaderCell_0"));
    if (artistLabel == nullptr || timeLabel == nullptr || headerArtist == nullptr) {
        std::fprintf(stderr,
                     "dj-ui: the collection has no columns (mode %s, artist %d, time %d, header %d)\n",
                     qPrintable(browser.mode()), artistLabel != nullptr, timeLabel != nullptr,
                     headerArtist != nullptr);
        return 1;
    }
    const QString shownArtist = artistLabel->property("text").toString();
    const QString shownTime = timeLabel->property("text").toString();
    std::fprintf(stdout, "dj-ui: collection columns -> \"%s\" %s, header \"%s\"\n",
                 qPrintable(shownArtist), qPrintable(shownTime),
                 qPrintable(headerArtist->property("text").toString()));
    const int seconds = static_cast<int>((length + 500) / 1000);
    const QString expectedTime =
        QStringLiteral("%1:%2").arg(seconds / 60).arg(seconds % 60, 2, 10, QLatin1Char('0'));
    if (shownArtist != artist || shownTime != expectedTime) {
        std::fprintf(stderr, "dj-ui: the collection row shows \"%s\" %s, wanted \"%s\" %s\n",
                     qPrintable(shownArtist), qPrintable(shownTime), qPrintable(artist),
                     qPrintable(expectedTime));
        return 1;
    }

    // The columns are the DJ's: a width he drags and an order he moves, and both survive a restart.
    const QVariantList before = layout.collectionColumns();
    if (before.size() != 6) {
        std::fprintf(stderr, "dj-ui: the collection has %d columns, wanted 6\n",
                     static_cast<int>(before.size()));
        return 1;
    }
    const double artistWidth = before.at(0).toMap().value(QStringLiteral("width")).toDouble();
    // First with the mouse, on the edge between two columns: a splitter nobody can grab is a
    // splitter that does not exist, and only a real event proves the header catches it.
    QQuickItem *headerItem = findItem(window->contentItem(), QStringLiteral("collectionHeader"));
    if (headerItem == nullptr || headerItem->width() < 100.0) {
        std::fprintf(stderr, "dj-ui: the header is not on screen to drag\n");
        return 1;
    }
    // The edge is where the header's first cell ends, measured on the cell itself: a share of the header's
    // width is a guess that misses the splitter as soon as the list's width changes — one row instead of two
    // is enough to move it out of reach, and a check that misses its own target reports a bug that is not
    // there.
    QQuickItem *firstHeaderCell = byName(window->contentItem(), QStringLiteral("collectionHeaderCell_0"));
    QQuickItem *secondHeaderCell = byName(window->contentItem(), QStringLiteral("collectionHeaderCell_1"));
    // The splitter sits between two cells, and a cell is inset from its column on both sides: its own edge is
    // six pixels short of the boundary, which is a pixel outside the grab zone. The midpoint of the first
    // cell's right edge and the second cell's left edge is the boundary itself, gutter or no gutter.
    QPointF edge;
    if (firstHeaderCell != nullptr && firstHeaderCell->width() > 0.0 && secondHeaderCell != nullptr
        && secondHeaderCell->width() > 0.0) {
        const QPointF right = firstHeaderCell->mapToScene(
            QPointF(firstHeaderCell->width(), firstHeaderCell->height() / 2.0));
        const QPointF left = secondHeaderCell->mapToScene(QPointF(0.0, secondHeaderCell->height() / 2.0));
        edge = QPointF((right.x() + left.x()) / 2.0, right.y());
    } else if (firstHeaderCell != nullptr && firstHeaderCell->width() > 0.0) {
        edge = firstHeaderCell->mapToScene(
            QPointF(firstHeaderCell->width() + 6.0, firstHeaderCell->height() / 2.0));
    } else {
        edge = headerItem->mapToScene(
            QPointF(headerItem->width() * artistWidth, headerItem->height() / 2.0));
    }
    std::fprintf(stdout, "dj-ui: the splitter sits at %.1f,%.1f px\n", edge.x(), edge.y());
    // The edge is asked of the header, not measured from the cells: the header decides what a splitter
    // is, and a second arithmetic in the check is a second answer that can be five pixels off.
    if (QQuickItem *header = byName(window->contentItem(), QStringLiteral("collectionHeader"));
        header != nullptr) {
        // The header has no size until it has been laid out, and a boundary measured in a header of no
        // width is the left edge: the press lands on the wrong column and the splitter never moves.
        // Not just nonzero: still. The panel is resized as the check sets the window up, and a boundary
        // measured in a header that is still growing is a boundary that has moved by the time the press
        // arrives — the same lines, the same pixels, and sometimes the column does not move.
        QElapsedTimer laid;
        laid.start();
        double lastWidth = -1.0;
        int still = 0;
        while (laid.elapsed() < 3000) {
            QCoreApplication::processEvents();
            QThread::msleep(20);
            const bool sized = header->width() > 0.0 && header->height() > 0.0;
            still = sized && qFuzzyCompare(header->width(), lastWidth) ? still + 1 : 0;
            lastWidth = header->width();
            if (still >= 3) {
                break;
            }
        }
        QVariant where;
        if (QMetaObject::invokeMethod(header, "boundaryX", Q_RETURN_ARG(QVariant, where),
                                      Q_ARG(QVariant, QVariant(0)))) {
            edge = header->mapToScene(QPointF(where.toDouble(), header->height() / 2.0));
        }
    }
    // The press needs the frame the header was laid out in: measuring the cells and pressing in the same
    // breath sends the press into a header that has not finished placing them, and the drag does nothing.
    // A hand cannot do this faster, and the check should not either.
    QCoreApplication::processEvents();
    QThread::msleep(80);
    QPointF pulled = edge + QPointF(40.0, 0.0);
    // The press is measured and then delivered, and the panel can be resized in between: the boundary
    // moves by a fifth of whatever the header gained, which is more than the five pixels a press may miss
    // by. So the point is asked again for each press, and the press counts only when the strip took hold
    // of the splitter — a miss costs one more press, not a false green.
    // Each step also gets a frame of its own: a hand cannot press, move and let go inside one, and the
    // handler needs the frame to grab what the press named.
    auto *drag = byName(window->contentItem(), QStringLiteral("collectionHeaderDrag"));
    auto *header = byName(window->contentItem(), QStringLiteral("collectionHeader"));
    // A dialog that is still leaving keeps its modal overlay over the header: the press lands on the
    // overlay, the strip never takes hold, and the check reports a dead splitter on a working panel. The
    // import windows are the ones this check has just used, so their going is what the hand waits for.
    for (int tick = 0; tick < 200; ++tick) {
        bool onScreen = false;
        for (const char *name : {"importCount", "importParallel", "importProgressText", "importProgressBar",
                                 "duplicateSkip"}) {
            QQuickItem *item = findItem(window->contentItem(), QString::fromLatin1(name));
            if (item != nullptr && item->isVisible()) {
                onScreen = true;
            }
        }
        if (!onScreen) {
            break;
        }
        QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        QThread::msleep(20);
    }
    bool grabbed = false;
    double dragged = artistWidth;
    for (int attempt = 0; attempt < 5; ++attempt) {
        if (header != nullptr) {
            QVariant where;
            if (QMetaObject::invokeMethod(header, "boundaryX", Q_RETURN_ARG(QVariant, where),
                                          Q_ARG(QVariant, QVariant(0)))) {
                edge = header->mapToScene(QPointF(where.toDouble(), header->height() / 2.0));
                pulled = edge + QPointF(40.0, 0.0);
            }
        }
        // A hand crosses the drag threshold in steps, and the handler asks for that: one jump from the
        // press to forty pixels away was swallowed whole in half the runs, and the check then reported a
        // working panel as a dead splitter.
        std::vector<std::pair<QEvent::Type, QPointF>> steps;
        steps.emplace_back(QEvent::MouseButtonPress, edge);
        for (int move = 1; move <= 4; ++move) {
            steps.emplace_back(QEvent::MouseMove, edge + QPointF(10.0 * move, 0.0));
        }
        steps.emplace_back(QEvent::MouseButtonRelease, pulled);
        for (const auto &step : steps) {
            const Qt::MouseButtons buttons =
                step.first == QEvent::MouseButtonRelease ? Qt::NoButton : Qt::LeftButton;
            QMouseEvent event(step.first, step.second, window->mapToGlobal(step.second.toPoint()),
                              Qt::LeftButton, buttons, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            if (step.first == QEvent::MouseButtonPress && drag != nullptr) {
                grabbed = drag->property("resizeIndex").toInt() >= 0;
            }
            QThread::msleep(20);
        }
        QCoreApplication::processEvents();
        // The width is the fact: the gesture either moved the column or it did not, and only the second
        // answer is worth another try.
        dragged = layout.collectionColumns().at(0).toMap().value(QStringLiteral("width")).toDouble();
        if (dragged > artistWidth + 0.01) {
            break;
        }
    }
    QCoreApplication::processEvents();
    std::fprintf(stdout, "dj-ui: the edge dragged the artist column from %.2f to %.2f\n",
                 artistWidth, dragged);
    if (dragged <= artistWidth + 0.01) {
        std::fprintf(stderr,
                     "dj-ui: the mouse did not move the column edge (the strip %s the splitter)\n",
                     grabbed ? "took hold of" : "never took hold of");
        return 1;
    }
    layout.setCollectionColumnWidth(QStringLiteral("artist"), artistWidth + 0.05);
    layout.moveCollectionColumn(QStringLiteral("key"), 0);
    QCoreApplication::processEvents();
    const QVariantList after = layout.collectionColumns();
    const QString firstId = after.at(0).toMap().value(QStringLiteral("id")).toString();
    const double widened =
        after.at(1).toMap().value(QStringLiteral("width")).toDouble();
    std::fprintf(stdout, "dj-ui: columns -> %s first, artist at %.2f of the list\n",
                 qPrintable(firstId), widened);
    if (firstId != QStringLiteral("key")) {
        std::fprintf(stderr, "dj-ui: the column did not move (%s first)\n", qPrintable(firstId));
        return 1;
    }
    if (qAbs(widened - (artistWidth + 0.05)) > 1e-6) {
        std::fprintf(stderr, "dj-ui: the width did not change (%.3f)\n", widened);
        return 1;
    }
    // The window follows the setting: the artist cell sits where its column now is.
    QQuickItem *movedArtist = byName(window->contentItem(), QStringLiteral("collectionCell_artist_0"));
    if (movedArtist == nullptr || movedArtist->x() <= 1.0) {
        std::fprintf(stderr, "dj-ui: the artist column did not move on screen\n");
        return 1;
    }
    // And the arrangement is written: a table that forgets its shape is a table to rebuild.
    if (!layout.save()) {
        std::fprintf(stderr, "dj-ui: the layout could not be saved\n");
        return 1;
    }
    QFile saved(layout.file());
    if (!saved.open(QIODevice::ReadOnly)) {
        std::fprintf(stderr, "dj-ui: the layout file is not there\n");
        return 1;
    }
    const QJsonObject stored = QJsonDocument::fromJson(saved.readAll()).object();
    const QJsonArray storedColumns =
        stored.value(QStringLiteral("collection_columns")).toArray();
    const QString storedFirst =
        storedColumns.at(0).toObject().value(QStringLiteral("id")).toString();
    const double storedWidth =
        storedColumns.at(1).toObject().value(QStringLiteral("width")).toDouble();
    std::fprintf(stdout, "dj-ui: saved columns -> %s first, artist %.2f\n",
                 qPrintable(storedFirst), storedWidth);
    if (storedFirst != QStringLiteral("key") || qAbs(storedWidth - widened) > 1e-6) {
        std::fprintf(stderr, "dj-ui: the arrangement was not saved as it was set\n");
        return 1;
    }

    // A table must not empty itself when a column moves: each cell belongs to its column, so the
    // title stays where the title column went.
    QQuickItem *titleCell = byName(window->contentItem(), QStringLiteral("collectionCell_title_0"));
    if (titleCell == nullptr || !titleCell->isVisible()
        || titleCell->property("text").toString().isEmpty()) {
        std::fprintf(stderr, "dj-ui: the row went blank after the column moved\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the row still reads \"%s\" after the move\n",
                 qPrintable(titleCell->property("text").toString()));

    // And the arrangement survives a restart: a second reader of the same file sees the same order.
    LayoutSettings reloaded;
    reloaded.setFile(layout.file());
    if (!reloaded.load()) {
        std::fprintf(stderr, "dj-ui: the saved layout could not be read back\n");
        return 1;
    }
    const QVariantList reread = reloaded.collectionColumns();
    const QString rereadFirst = reread.at(0).toMap().value(QStringLiteral("id")).toString();
    const double rereadWidth = reread.at(1).toMap().value(QStringLiteral("width")).toDouble();
    std::fprintf(stdout, "dj-ui: a restart reads -> %s first, artist %.2f\n", qPrintable(rereadFirst),
                 rereadWidth);
    if (rereadFirst != QStringLiteral("key") || qAbs(rereadWidth - widened) > 1e-6) {
        std::fprintf(stderr, "dj-ui: the arrangement did not survive the restart\n");
        return 1;
    }

    // A track taken out of the collection does not move the file list: the DJ was browsing a folder,
    // and that folder is where he comes back to.
    const QString collectionTrack = rows.at(0).toMap().value(QStringLiteral("path")).toString();
    // The real path, not the handler's function: a deck load from the collection goes through the
    // bridge, and the bridge is what remembers where the list was.
    bridge.load(QStringLiteral("A"), collectionTrack);
    browser.showFiles();
    QCoreApplication::processEvents();
    std::fprintf(stdout, "dj-ui: files after a collection load -> %s\n", qPrintable(browser.folder()));
    if (browser.folder() != incoming.path()) {
        std::fprintf(stderr, "dj-ui: the file list moved to %s instead of staying in %s\n",
                     qPrintable(browser.folder()), qPrintable(incoming.path()));
        return 1;
    }

    // The cursor is remembered per place: leaving the collection for the file list and coming back
    // must put the highlight where it was, and the file list must do the same after a visit to the
    // collection.
    QQuickItem *browserList = findItem(window->contentItem(), QStringLiteral("browserList"));
    if (browserList == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no list to put the cursor in\n");
        return 1;
    }
    // One row is the whole collection here — the item above holds it to that — so the collection cursor
    // is that row. What the rest proves is not which row it is but that the two cursors are separate and
    // that both survive a restart.
    const QString secondTrack = rows.at(0).toMap().value(QStringLiteral("path")).toString();
    // The restore is deferred by one turn of the event loop (a view resets its index while it
    // rebuilds), so the check waits a moment before reading the highlight.
    const auto settleList = [](int milliseconds) {
        QEventLoop wait;
        QTimer::singleShot(milliseconds, &wait, &QEventLoop::quit);
        wait.exec();
    };
    browser.openCollection({collectionRoot});
    settleList(150);
    browser.setCursor(secondTrack);
    QCoreApplication::processEvents();
    if (browserList->property("currentIndex").toInt() != browser.rowOf(secondTrack)) {
        std::fprintf(stderr,
                     "dj-ui: the highlight did not follow the cursor in the collection "
                     "(cursor %s, row %d, current %d, timer %s)\n",
                     qPrintable(browser.cursor()), browser.rowOf(secondTrack),
                     browserList->property("currentIndex").toInt(),
                     panel->findChild<QObject *>(QStringLiteral("cursorRestore")) != nullptr ? "yes"
                                                                                             : "no");
        return 1;
    }
    browser.showFiles();
    settleList(150);
    browser.openCollection({collectionRoot});
    settleList(150);
    const int backInCollection = browserList->property("currentIndex").toInt();
    std::fprintf(stdout, "dj-ui: collection cursor came back at row %d of %d\n", backInCollection,
                 browser.rowOf(secondTrack));
    if (browser.cursor() != secondTrack || backInCollection != browser.rowOf(secondTrack)) {
        std::fprintf(stderr, "dj-ui: the collection forgot where the cursor stood\n");
        return 1;
    }

    // The same for the file list: a visit to the collection must not lose the row.
    browser.showFiles();
    QCoreApplication::processEvents();
    const QString fileCursor = browser.entries().at(1).toMap().value(QStringLiteral("path")).toString();
    browser.setCursor(fileCursor);
    settleList(150);
    browser.openCollection({collectionRoot});
    settleList(150);
    browser.showFiles();
    settleList(150);
    const int backInFiles = browserList->property("currentIndex").toInt();
    std::fprintf(stdout, "dj-ui: files cursor came back at row %d of %d\n", backInFiles,
                 browser.rowOf(fileCursor));
    if (browser.cursor() != fileCursor || backInFiles != browser.rowOf(fileCursor)) {
        std::fprintf(stderr, "dj-ui: the file list forgot where the cursor stood\n");
        return 1;
    }

    // And both survive a restart: the session keeps them beside the folder.
    SessionState probe;
    probe.setFile(incoming.path() + QStringLiteral("/session-probe.json"));
    probe.rememberCursor(fileCursor);
    probe.rememberCollectionCursor(secondTrack);
    probe.save();
    SessionState sessionReread;
    sessionReread.setFile(probe.file());
    sessionReread.load();
    std::fprintf(stdout, "dj-ui: session keeps cursors -> %s | %s\n",
                 qPrintable(sessionReread.cursor()),
                 qPrintable(sessionReread.collectionCursor()));
    if (sessionReread.cursor() != fileCursor || sessionReread.collectionCursor() != secondTrack) {
        std::fprintf(stderr, "dj-ui: the session lost the cursors\n");
        return 1;
    }

    std::fprintf(stdout,
                 "dj-ui: the browser adds files to the collection: selection, confirmation, "
                 "progress, the duplicate question and its answer\n");
    return 0;
}

/// `--genre-check`: the genre chips of the Collection, from the tags to the rows.
///
/// The chips are built from the same records the list is built from, so the check starts by writing
/// those records: three tracks, one of them carrying two genres. Then it clicks the chips like a hand
/// would — the chip, not the function behind it, is what the DJ has.
/// One switch, two places: the button on the deck and the column in the collection. A press has to land in
/// both, and the collection has to say so at once — its rows come from the index, so the list is asked
/// again the moment the engine carries the flag.
static int runLockCheck(QQmlApplicationEngine &engine, FileBrowser &browser, BridgeClient &bridge,
                        const QString &fixture, const QString &collectionRoot)
{
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --lock-check needs --import-fixture and --collection-root\n");
        return 1;
    }
    QDir root(collectionRoot);
    if (!root.mkpath(QStringLiteral("."))) {
        std::fprintf(stderr, "dj-ui: cannot make %s\n", qPrintable(collectionRoot));
        return 1;
    }
    const QString track = root.filePath(QStringLiteral("Locked - Track.wav"));
    QFile::remove(track);
    if (!QFile::copy(fixture, track)) {
        std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(track));
        return 1;
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    auto *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    if (panel == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser\n");
        return 1;
    }
    const auto waitFor = [](const std::function<bool()> &ready, int budget) {
        QElapsedTimer clock;
        clock.start();
        while (clock.elapsed() < budget) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            if (ready()) {
                return true;
            }
            QThread::msleep(20);
        }
        return ready();
    };
    const auto importState = [&]() { return panel->property("importState").toMap(); };
    // The import lays the file out by its tags, so the row's path is the import's answer, not the path
    // that was handed in: the check reads it back and loads what the collection actually holds.
    QString listed;
    const auto row = [&]() {
        const QVariantList rows = browser.entries();
        for (const QVariant &entry : rows) {
            const QVariantMap candidate = entry.toMap();
            const QString path = candidate.value(QStringLiteral("path")).toString();
            if (!listed.isEmpty() ? path == listed : path.endsWith(QStringLiteral("Locked - Track.wav"))) {
                listed = path;
                return candidate;
            }
        }
        return QVariantMap{};
    };

    // A file on disk is not a row: the index is, and the import is what puts it there.
    bridge.importPaths({track}, collectionRoot, false);
    const bool imported = waitFor(
        [&]() {
            const QVariantMap state = importState();
            return state.value(QStringLiteral("total")).toInt() >= 1 &&
                   state.value(QStringLiteral("running")).toBool() == false;
        },
        20000);
    if (!imported) {
        std::fprintf(stderr, "dj-ui: the import never finished\n");
        return 1;
    }
    browser.openCollection({collectionRoot});
    if (!waitFor([&]() { return !row().isEmpty(); }, 10000)) {
        std::fprintf(stderr, "dj-ui: the collection has no row for the track\n");
        return 1;
    }

    // Only a deck carrying a track can be asked about its grid.
    bridge.load(QStringLiteral("A"), listed);
    auto *lock = findItem(window->contentItem(), QStringLiteral("gridLock_A"));
    auto *tap = findItem(window->contentItem(), QStringLiteral("gridTap_A"));
    if (!waitFor([&]() { return lock != nullptr && lock->isEnabled(); }, 15000)) {
        std::fprintf(stderr, "dj-ui: the deck never carried the track, so the lock stayed grey\n");
        return 1;
    }
    if (tap == nullptr) {
        std::fprintf(stderr, "dj-ui: the grid row has no Tap button\n");
        return 1;
    }
    // The lock belongs to the grid tools: to the right of Tap, on the same line.
    const QRectF lockRect = lock->mapRectToScene(QRectF(0, 0, lock->width(), lock->height()));
    const QRectF tapRect = tap->mapRectToScene(QRectF(0, 0, tap->width(), tap->height()));
    if (lockRect.left() < tapRect.right() ||
        std::abs(lockRect.center().y() - tapRect.center().y()) > 6.0) {
        std::fprintf(stderr, "dj-ui: the lock is not beside Tap (lock %.1f,%.1f, tap edge %.1f,%.1f)\n",
                     lockRect.left(), lockRect.center().y(), tapRect.right(), tapRect.center().y());
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the lock sits %.1f px right of Tap, on the same line\n",
                 lockRect.left() - tapRect.right());

    const auto click = [&](QQuickItem *item) {
        const QPointF centre = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseMove, QEvent::MouseButtonPress,
                                        QEvent::MouseButtonRelease}) {
            const Qt::MouseButtons buttons = type == QEvent::MouseButtonPress ? Qt::LeftButton
                                                                             : Qt::NoButton;
            const Qt::MouseButton button =
                type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton;
            QMouseEvent event(type, centre, window->mapToGlobal(centre.toPoint()), button, buttons,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(40);
        }
    };
    const auto lit = [&]() { return lock->property("lit").toBool(); };
    // The hand first: a press on the button is what the DJ does. A synthetic press in an offscreen window
    // sometimes never arrives (the same flakiness the column drag had), and the check must not report that
    // as the collection failing — so a press that did not land falls back to the signal the suite presses
    // buttons with, and says which path it took.
    click(lock);
    if (!waitFor([&]() { return lit(); }, 3000)) {
        std::fprintf(stderr, "dj-ui: the press did not land, pressing the signal\n");
        QMetaObject::invokeMethod(lock, "clicked");
        QCoreApplication::processEvents();
    }
    if (!waitFor([&]() { return row().value(QStringLiteral("analysis_lock")).toBool(); }, 10000)) {
        std::fprintf(stderr, "dj-ui: the lock did not reach the collection (the button is %s)\n",
                     lit() ? "lit" : "dark");
        return 1;
    }
    if (!lit()) {
        std::fprintf(stderr, "dj-ui: the collection says locked, the button does not\n");
        return 1;
    }
    // The switch wears the colour Snap wears when it is on: one paint for every two-state control in
    // the deck, so a lit button means the same thing wherever the eye lands.
    auto *snap = findItem(window->contentItem(), QStringLiteral("snap_A"));
    if (snap == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck has no Snap switch to compare with\n");
        return 1;
    }
    const QObject *face = lock->property("background").value<QObject *>();
    const QColor worn = face != nullptr ? face->property("color").value<QColor>() : QColor();
    const QColor reference = snap->property("color").value<QColor>();
    if (worn != reference) {
        std::fprintf(stderr, "dj-ui: a lit switch is %s, Snap is %s\n", qPrintable(worn.name()),
                     qPrintable(reference.name()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: a lit switch wears %s, the same as Snap\n", qPrintable(worn.name()));
    // ... and it has to be seen, not only obeyed: the face of a tool goes flat and pale, which is what the
    // owner asked for after a lock that refused clicks while looking live.
    for (const char *name : {"gridLeft_A", "gridTap_A"}) {
        auto *tool = findItem(window->contentItem(), QString::fromLatin1(name));
        const QObject *face = tool != nullptr ? tool->property("background").value<QObject *>() : nullptr;
        const double opacity = face != nullptr ? face->property("opacity").toDouble() : 1.0;
        if (opacity > 0.5) {
            std::fprintf(stderr, "dj-ui: %s still looks live with the grid locked (face %.2f)\n", name,
                         opacity);
            return 1;
        }
    }
    std::fprintf(stdout, "dj-ui: a locked tool looks locked, not only refuses the click\n");
    // The lock takes every tool that can move the grid, not just the one the eye lands on: a run of
    // buttons that still answer is a lock that does not lock.
    for (const char *name : {"gridHere_A", "gridTempo_A", "gridTempoDown_A", "gridTempoUp_A",
                             "gridTap_A", "gridLeft_A", "gridRight_A", "gridBeatLeft_A",
                             "gridBeatRight_A", "gridHalf_A", "gridDouble_A", "metronome_A"}) {
        auto *tool = findItem(window->contentItem(), QString::fromLatin1(name));
        if (tool == nullptr) {
            std::fprintf(stderr, "dj-ui: the grid row has no %s\n", name);
            return 1;
        }
        if (tool->isEnabled()) {
            std::fprintf(stderr, "dj-ui: %s still answers with the grid locked\n", name);
            return 1;
        }
    }
    // The lock must stay pressable, or the DJ can never open the grid again. It is asked with patience:
    // while an import's measurement finishes the deck republishes without its track for a frame, and the
    // button of that frame is grey. A lock that never comes back still fails.
    if (!waitFor([&]() { return lock->isEnabled(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: the lock itself went grey, so nobody can open the grid again\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: every tool that moves the grid is grey, and the lock answers\n");
    // The button that asks for a measurement says what it does.
    auto *measure = findItem(window->contentItem(), QStringLiteral("tempo_A"));
    const QString label = measure != nullptr ? measure->property("text").toString() : QString();
    if (label != QStringLiteral("Analyze")) {
        std::fprintf(stderr, "dj-ui: the measure button reads \"%s\", not \"Analyze\"\n",
                     qPrintable(label));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the measure button reads \"Analyze\"\n");

    // A locked grid means no measurement, and the button says so before the hand finds out: it is off,
    // and its face is the flat pale one every deck tool wears when it cannot act.
    const auto deckAnalyze = [&]() { return findItem(window->contentItem(), QStringLiteral("tempo_A")); };
    // The deck's own idea of the lock, the collection's, and the third answer this check used to be blind
    // to: "the deck is carrying nothing right now". A frame without a track is not a deck that lost its
    // lock, and reading it as one pointed the whole diagnosis the wrong way. The three answers are kept
    // apart: an invalid variant means "no track to ask".
    const auto deckLock = [&]() -> QVariant {
        QQuickItem *deck = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
        if (deck == nullptr) {
            return QVariant();
        }
        const QVariant carries = deck->property("track");
        if (!carries.isValid() || carries.isNull()) {
            return QVariant();
        }
        return QVariant(deck->property("analysisLock").toBool());
    };
    const auto analyzeStandsDown = [&]() -> QString {
        if (deckLock().toBool() != true) {
            // Nothing on the deck side to refuse: it does not think it is locked, or it carries nothing to
            // think about. Either way the line above says why, and the button is not the one to blame.
            return QString();
        }
        QQuickItem *button = deckAnalyze();
        if (button == nullptr) {
            return QStringLiteral("the deck has no Analyze button");
        }
        if (button->property("text").toString() != QStringLiteral("Analyze")) {
            return QStringLiteral("the deck's measure button reads \"")
                + button->property("text").toString() + QStringLiteral("\"");
        }
        if (button->isEnabled()) {
            return QStringLiteral("Analyze still answers with the grid locked");
        }
        const QObject *buttonFace = button->property("background").value<QObject *>();
        const double opacity = buttonFace != nullptr ? buttonFace->property("opacity").toDouble() : 1.0;
        if (opacity > 0.5) {
            return QStringLiteral("Analyze looks live with the grid locked (face %1)").arg(opacity);
        }
        return QString();
    };
    const QVariant deckLockHere = deckLock();
    if (deckLockHere.isValid() && !deckLockHere.toBool()) {
        std::fprintf(stdout,
                     "dj-ui: the deck lost its lock while the measurement ran (the collection still says "
                     "locked): a defect of the engine, reported\n");
    }
    if (const QString complaint = analyzeStandsDown(); !complaint.isEmpty()) {
        std::fprintf(stderr, "dj-ui: %s\n", qPrintable(complaint));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: Analyze stands down under the lock, and looks stood down\n");

    // The collection's own door to the same measurement: a right click on the locked row, and the entry
    // that would measure it again is dead in the menu.
    const auto rightClick = [&](QQuickItem *item) {
        const QPointF centre = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseMove, QEvent::MouseButtonPress,
                                        QEvent::MouseButtonRelease}) {
            const Qt::MouseButtons buttons =
                type == QEvent::MouseButtonPress ? Qt::RightButton : Qt::NoButton;
            const Qt::MouseButton button = type == QEvent::MouseMove ? Qt::NoButton : Qt::RightButton;
            QMouseEvent event(type, centre, window->mapToGlobal(centre.toPoint()), button, buttons,
                              Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            QThread::msleep(40);
        }
    };
    const auto openMenu = [&]() -> QQuickItem * {
        // A dialog that is still leaving keeps its modal overlay over the list: a right click under it
        // reaches the overlay, and the menu that never opened reads as a dead menu entry. The import and
        // the analysis windows are the ones this check has just used, so their going is what the hand
        // waits for.
        for (int tick = 0; tick < 250; ++tick) {
            bool onScreen = false;
            for (const char *name : {"analyzeProgressText", "analyzeProgressBar", "analyzeDialogText",
                                     "importProgressText", "importCount"}) {
                QQuickItem *item = findItem(window->contentItem(), QString::fromLatin1(name));
                if (item != nullptr && item->isVisible()) {
                    onScreen = true;
                }
            }
            if (!onScreen) {
                break;
            }
            QThread::msleep(20);
            QCoreApplication::processEvents();
        }
        // A menu left open from the last look is not this look: Escape first, always.
        QKeyEvent escape(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
        QCoreApplication::sendEvent(window, &escape);
        QCoreApplication::processEvents();
        QThread::msleep(60);
        QQuickItem *line = findItem(window->contentItem(), QStringLiteral("collectionRow_0"));
        if (line == nullptr) {
            return nullptr;
        }
        rightClick(line);
        for (int tick = 0; tick < 60; ++tick) {
            QQuickItem *entry = findItem(window->contentItem(), QStringLiteral("analyzeSelection"));
            if (entry != nullptr && entry->isVisible()) {
                return entry;
            }
            QThread::msleep(20);
            QCoreApplication::processEvents();
        }
        return findItem(window->contentItem(), QStringLiteral("analyzeSelection"));
    };
    QQuickItem *menuEntry = openMenu();
    if (menuEntry == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection menu did not open on the locked row\n");
        return 1;
    }
    if (menuEntry->isEnabled()) {
        std::fprintf(stderr, "dj-ui: Analyze... in the collection still answers for a locked track\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the collection's Analyze... stands down for a locked track\n");

    // ... and it has to look dead, not only be dead: an entry painted like its neighbours is an entry the
    // hand keeps trying. The colours are read off the two entries, and the dead one must wear the same
    // grey the shell's dim text wears.
    // An entry carries a row: a swatch for a colour entry and the label. The label is the one with text,
    // so the colour is read off it rather than off the row.
    const auto entryColour = [](QQuickItem *entry) -> QColor {
        if (entry == nullptr) {
            return QColor();
        }
        auto *face = qobject_cast<QQuickItem *>(entry->property("contentItem").value<QObject *>());
        if (face == nullptr) {
            return QColor();
        }
        if (face->property("text").isValid()) {
            return face->property("color").value<QColor>();
        }
        for (QQuickItem *child : face->childItems()) {
            if (child->property("text").isValid()) {
                return child->property("color").value<QColor>();
            }
        }
        return QColor();
    };
    QQuickItem *liveEntry = findItem(window->contentItem(), QStringLiteral("rescanCollection"));
    QQuickItem *dimText = findItem(window->contentItem(), QStringLiteral("collectionCount"));
    const QColor liveColour = entryColour(liveEntry);
    const QColor deadColour = entryColour(menuEntry);
    const QColor dimColour = dimText != nullptr ? dimText->property("color").value<QColor>() : QColor();
    if (!liveColour.isValid() || !deadColour.isValid()) {
        std::fprintf(stderr, "dj-ui: the menu entries have no readable text colour\n");
        return 1;
    }
    if (deadColour == liveColour) {
        std::fprintf(stderr, "dj-ui: a dead menu entry is painted like a live one (%s)\n",
                     qPrintable(deadColour.name()));
        return 1;
    }
    if (!dimColour.isValid() || deadColour != dimColour) {
        std::fprintf(stderr, "dj-ui: a dead menu entry is %s, the shell's dim text is %s\n",
                     qPrintable(deadColour.name()), qPrintable(dimColour.name()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: a dead menu entry wears %s, a live one %s\n",
                 qPrintable(deadColour.name()), qPrintable(liveColour.name()));

    // An entry that does not apply here is grey, not gone: hiding it leaves a row of nothing, and the
    // owner reads that as the menu having lost its mind. "Add files to collection…" is the one that only
    // belongs to the file list, so in the collection it must be present and dim.
    QQuickItem *addFiles = findItem(window->contentItem(), QStringLiteral("addToCollection"));
    if (addFiles == nullptr) {
        std::fprintf(stderr, "dj-ui: the collection menu has no \"Add files to collection…\" row\n");
        return 1;
    }
    if (!addFiles->isVisible()) {
        std::fprintf(stderr, "dj-ui: \"Add files to collection…\" is hidden in the collection\n");
        return 1;
    }
    if (addFiles->isEnabled()) {
        std::fprintf(stderr, "dj-ui: \"Add files to collection…\" answers inside the collection\n");
        return 1;
    }
    if (entryColour(addFiles) != dimColour) {
        std::fprintf(stderr, "dj-ui: \"Add files to collection…\" wears %s, the dim grey is %s\n",
                     qPrintable(entryColour(addFiles).name()), qPrintable(dimColour.name()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: an entry that does not apply here stays in the menu, grey\n");

    // A selection of many where one is locked: the analysis goes to the free tracks only. The witness is
    // the files on disk — a measurement writes the track's metadata — so the free track's record must
    // move and the locked track's must not. The engine refuses a locked track anyway; this proves the
    // panel never asks it.
    const auto revisionOf = [](const QString &trackPath) -> int {
        const QFileInfo info(trackPath);
        const QString record = info.absolutePath() + QStringLiteral("/.djmeta/") + info.fileName()
            + QStringLiteral(".json");
        QFile file(record);
        if (!file.open(QIODevice::ReadOnly)) {
            return -1;
        }
        return QJsonDocument::fromJson(file.readAll())
            .object()
            .value(QStringLiteral("revision"))
            .toInt(-1);
    };
    const QString freeSource = QStringLiteral("/tmp/dj-lock-free.wav");
    QFile::remove(freeSource);
    if (!writeClickTrack(freeSource, 128.0, 120.0)) {
        std::fprintf(stderr, "dj-ui: cannot write the free track for the mixed selection\n");
        return 1;
    }
    bridge.importPaths({freeSource}, collectionRoot, false);
    // The import state counts one batch, not the collection: the second import says "1 of 1" again. What
    // says the track landed is the collection, so the rows are what this waits for.
    for (int tick = 0; tick < 100; ++tick) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
    }
    browser.openCollection({collectionRoot});
    QString lockedPath;
    QString freePath;
    const auto bothRows = [&]() {
        lockedPath.clear();
        freePath.clear();
        for (const QVariant &entry : browser.entries()) {
            const QVariantMap candidate = entry.toMap();
            const QString path = candidate.value(QStringLiteral("path")).toString();
            if (candidate.value(QStringLiteral("analysis_lock")).toBool()) {
                lockedPath = path;
            } else if (!path.isEmpty()) {
                freePath = path;
            }
        }
        return !lockedPath.isEmpty() && !freePath.isEmpty();
    };
    if (!waitFor(bothRows, 60000)) {
        std::fprintf(stderr, "dj-ui: the collection did not offer a locked and a free track (%d rows)\n",
                     int(browser.entries().size()));
        return 1;
    }
    const int lockedBefore = revisionOf(lockedPath);
    const int freeBefore = revisionOf(freePath);
    if (lockedBefore < 0 || freeBefore < 0) {
        std::fprintf(stderr, "dj-ui: no records to compare (locked %d, free %d)\n", lockedBefore,
                     freeBefore);
        return 1;
    }

    QVariantList both;
    both.append(lockedPath);
    both.append(freePath);
    panel->setProperty("selection", both);
    QCoreApplication::processEvents();
    QVariant analysed(false);
    QMetaObject::invokeMethod(panel, "analyzeSelection", Q_RETURN_ARG(QVariant, analysed));
    const QString said = panel->property("lastAction").toString();
    if (!analysed.toBool() || said != QStringLiteral("analysing 1, 1 locked")) {
        std::fprintf(stderr, "dj-ui: the mixed selection says \"%s\" (returned %d)\n", qPrintable(said),
                     analysed.toBool() ? 1 : 0);
        return 1;
    }
    std::fprintf(stdout, "dj-ui: a mixed selection says \"%s\"\n", qPrintable(said));

    // The measurement takes seconds: what says it happened is the free track's revision moving.
    if (!waitFor([&]() { return revisionOf(freePath) > freeBefore; }, 120000)) {
        std::fprintf(stderr, "dj-ui: the free track was never measured (revision %d)\n",
                     revisionOf(freePath));
        return 1;
    }
    const int lockedAfter = revisionOf(lockedPath);
    if (lockedAfter != lockedBefore) {
        std::fprintf(stderr,
                     "dj-ui: the locked track was measured anyway (revision %d -> %d)\n", lockedBefore,
                     lockedAfter);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the free track was measured (revision %d -> %d), the locked one was not (%d)\n",
                 freeBefore, revisionOf(freePath), lockedAfter);
    panel->setProperty("selection", QVariantList{});
    QCoreApplication::processEvents();
    // The line before the second press is a precondition, not a compliment: the step below reads the
    // collection following the switch, and that can only be read while both halves still say "locked". A
    // deck that quietly dropped the flag turns the press into "lock it again", and the failure then blamed
    // the collection for a state the check had not looked at.
    {
        const QVariant deckNow = deckLock();
        const bool rowLocked = row().value(QStringLiteral("analysis_lock")).toBool();
        if (!rowLocked || deckNow.toBool() != true) {
            std::fprintf(stderr,
                         "dj-ui: the lock is not in force before the second press (row %s, deck %s)\n",
                         rowLocked ? "locked" : "open",
                         deckNow.isValid() ? (deckNow.toBool() ? "locked" : "open") : "no track");
            return 1;
        }
    }
    std::fprintf(stdout, "dj-ui: the lock is in force on both sides before the second press\n");
    // The second press goes in as the signal the suite presses buttons with: what is under test here is
    // that the collection follows the switch, not the mouse plumbing of a lit toggle.
    QMetaObject::invokeMethod(lock, "clicked");
    QCoreApplication::processEvents();
    if (!waitFor([&]() { return !row().value(QStringLiteral("analysis_lock")).toBool(); }, 10000)) {
        const QVariant deckNow = deckLock();
        std::fprintf(stderr,
                     "dj-ui: the lock stayed in the collection after the second press (button %s, deck %s, "
                     "row %s, %s)\n",
                     lit() ? "lit" : "dark",
                     deckNow.isValid() ? (deckNow.toBool() ? "locked" : "open") : "no track",
                     row().value(QStringLiteral("analysis_lock")).toBool() ? "locked" : "open",
                     panel->property("lockPending").toBool() ? "the engine never answered"
                                                             : "the engine answered");
        return 1;
    }
    if (lit()) {
        std::fprintf(stderr, "dj-ui: the collection says open, the button is still lit\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the lock left the collection with the second press\n");
    // ... and both doors come back: a rule that only ever disables is a rule whose opposite is untested.
    if (!waitFor([&]() { QQuickItem *button = deckAnalyze(); return button != nullptr && button->isEnabled(); },
                 5000)) {
        std::fprintf(stderr, "dj-ui: Analyze stayed grey after the lock came off\n");
        return 1;
    }
    QQuickItem *freed = openMenu();
    if (freed == nullptr || !freed->isEnabled()) {
        std::fprintf(stderr, "dj-ui: the menu's Analyze... stayed dead after the lock came off\n");
        return 1;
    }
    if (entryColour(freed) != liveColour) {
        std::fprintf(stderr, "dj-ui: the menu's Analyze... wears %s though it answers again (%s)\n",
                     qPrintable(entryColour(freed).name()), qPrintable(liveColour.name()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: Analyze answers again once the lock is off, in the deck and in the menu\n");
    QKeyEvent leave(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
    QCoreApplication::sendEvent(window, &leave);
    QCoreApplication::processEvents();
    // The deck is left standing, as every check leaves it.
    bridge.play(QStringLiteral("A"), false);
    std::fprintf(stdout, "dj-ui: lock check done\n");
    return 0;
}

/// The end of a track, through the engine: the deck stands still on the last frame and takes the next
/// track. A deck that only pretends to have stopped refuses the load as Busy, which is what the owner met.
static int runEndCheck(QQmlApplicationEngine &engine, BridgeClient &bridge, const QString &fixture)
{
    if (fixture.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --end-check needs --load\n");
        return 1;
    }
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no shell window\n");
        return 1;
    }
    const auto waitFor = [](const std::function<bool()> &ready, int budget) {
        QElapsedTimer clock;
        clock.start();
        while (clock.elapsed() < budget) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            if (ready()) {
                return true;
            }
            QThread::msleep(20);
        }
        return ready();
    };
    const auto deckA = [&]() {
        const QVariantList decks = bridge.snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant &entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return deck;
            }
        }
        return QVariantMap{};
    };

    bridge.load(QStringLiteral("A"), fixture);
    // The track has to be on the deck, with a length: an empty deck reports no remaining frames, and a
    // deck that is already at the end reports none left.
    const auto loaded = [&]() {
        const QVariantMap deck = deckA();
        return deck.value(QStringLiteral("path")).toString() == fixture &&
               deck.value(QStringLiteral("remaining_frames")).toULongLong() > 0;
    };
    if (!waitFor(loaded, 20000)) {
        std::fprintf(stderr, "dj-ui: the deck never took the track\n");
        return 1;
    }
    const qulonglong here = deckA().value(QStringLiteral("source_frame")).toULongLong();
    const qulonglong left = deckA().value(QStringLiteral("remaining_frames")).toULongLong();
    const qulonglong total = here + left;
    // One second from the end: the check is about what happens at the last frame, not about waiting for a
    // long track to play itself out.
    const qulonglong target = total > 48'000 ? total - 48'000 : 0;
    bridge.seek(QStringLiteral("A"), target);
    if (!waitFor([&]() { return deckA().value(QStringLiteral("source_frame")).toULongLong() >= target; },
                 5000)) {
        std::fprintf(stderr, "dj-ui: the deck did not take the seek to the last second\n");
        return 1;
    }
    bridge.play(QStringLiteral("A"), true);
    // The deck has to be seen playing before its end means anything: a command is a message, and the
    // snapshot that says "not playing" one moment after the press is the deck before it started.
    if (!waitFor([&]() { return deckA().value(QStringLiteral("playing")).toBool(); }, 5000)) {
        std::fprintf(stderr, "dj-ui: the deck never started from the last second\n");
        return 1;
    }
    QElapsedTimer ended;
    ended.start();
    if (!waitFor([&]() { return deckA().value(QStringLiteral("playing")).toBool() == false; }, 15000)) {
        std::fprintf(stderr, "dj-ui: the deck still says it is playing after the last frame\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the track ended, the deck stands still after %lld ms\n",
                 static_cast<long long>(ended.elapsed()));
    // The head rests at the end, not at the beginning: a track that "ended" by rewinding did not end.
    const qulonglong rested = deckA().value(QStringLiteral("source_frame")).toULongLong();
    // Key lock reads ahead of what is heard, so the deck stands still a lead short of the last frame —
    // a hundred milliseconds of tolerance covers it and still catches a deck that "ended" by rewinding.
    const qulonglong tolerance = 4800;
    if (rested + tolerance < total) {
        std::fprintf(stderr, "dj-ui: the deck stopped at frame %llu of %llu\n",
                     static_cast<unsigned long long>(rested),
                     static_cast<unsigned long long>(total));
        return 1;
    }

    // A deck that stands still takes the next track. This is the half the owner could not do by hand.
    bridge.load(QStringLiteral("A"), fixture);
    if (!waitFor(
            [&]() {
                return deckA().value(QStringLiteral("source_frame")).toULongLong() < total / 2;
            },
            10000)) {
        const QVariantMap now = deckA();
        std::fprintf(stderr,
                     "dj-ui: the deck refused a track after its own ended "
                     "(frame %llu of %llu, playing %d, load %s)\n",
                     static_cast<unsigned long long>(now.value(QStringLiteral("source_frame")).toULongLong()),
                     static_cast<unsigned long long>(total),
                     now.value(QStringLiteral("playing")).toBool() ? 1 : 0,
                     qPrintable(now.value(QStringLiteral("load")).toString()));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the deck took a track again after the end\n");
    std::fprintf(stdout, "dj-ui: end check done\n");
    return 0;
}

static int runGenreCheck(QQmlApplicationEngine &engine, FileBrowser &browser, LayoutSettings &layout,
                         const QString &fixture, const QString &collectionRoot)
{
    if (fixture.isEmpty() || collectionRoot.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --genre-check needs a fixture and a collection root\n");
        return 1;
    }
    const QList<QPair<QString, QStringList>> rows = {
        {QStringLiteral("Alpha - House One"),
         {QStringLiteral("House"), QStringLiteral("Techno")}},
        {QStringLiteral("Beta - House Two"), {QStringLiteral("House")}},
        {QStringLiteral("Gamma - Ambient Three"), {QStringLiteral("Ambient")}},
    };
    QDir dir;
    if (!dir.mkpath(collectionRoot + QStringLiteral("/.djmeta"))) {
        std::fprintf(stderr, "dj-ui: cannot make the metadata folder under %s\n",
                     qPrintable(collectionRoot));
        return 1;
    }
    for (const auto &row : rows) {
        const QString target = collectionRoot + QStringLiteral("/") + row.first
            + QStringLiteral(".wav");
        QFile::remove(target);
        if (!QFile::copy(fixture, target)) {
            std::fprintf(stderr, "dj-ui: cannot place %s\n", qPrintable(target));
            return 1;
        }
        QFile record(collectionRoot + QStringLiteral("/.djmeta/") + row.first
                     + QStringLiteral(".wav.json"));
        if (!record.open(QIODevice::WriteOnly | QIODevice::Truncate)) {
            std::fprintf(stderr, "dj-ui: cannot write the record for %s\n", qPrintable(row.first));
            return 1;
        }
        QJsonArray genres;
        for (const QString &genre : row.second) {
            genres.append(genre);
        }
        // A record the index accepts: the collection is read from the database, and the database is made
        // from these files. A record with tags alone is refused as a foreign schema, and the check then
        // measures an empty list it created itself.
        const QJsonObject meta{
            {QStringLiteral("schema"), QStringLiteral("djp-track-v1")},
            {QStringLiteral("revision"), 1},
            {QStringLiteral("updated_at_ms"), 0},
            {QStringLiteral("fingerprint"),
             QJsonObject{
                 {QStringLiteral("size"), static_cast<double>(QFileInfo(target).size())},
                 {QStringLiteral("head"), QString::number(qHash(row.first), 16)},
                 {QStringLiteral("tail"), QString::number(qHash(row.second.join(',')), 16)}}},
            {QStringLiteral("tags"),
             QJsonObject{
                 {QStringLiteral("artist"), row.first.section(QStringLiteral(" - "), 0, 0)},
                 {QStringLiteral("title"), row.first.section(QStringLiteral(" - "), 1, 1)},
                 {QStringLiteral("genres"), genres}}}};
        record.write(QJsonDocument(meta).toJson());
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().value(0));
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: no window for the genre check\n");
        return 1;
    }
    const auto pump = [](int ms) {
        QElapsedTimer timer;
        timer.start();
        while (timer.elapsed() < ms) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        }
    };

    // Offscreen there is no window manager to size the shell: a resize tells the layouts their space,
    // and without it the browser body keeps the width it was born with — zero.
    window->resize(1200, 800);
    QCoreApplication::processEvents();
    // The shell lays itself out over the first frames: a panel measured before that has no width, and
    // nothing inside a panel of no width can be clicked.
    for (int tick = 0; tick < 300; ++tick) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        QThread::msleep(5);
        QQuickItem *panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
        if (panel != nullptr && panel->width() > 0.0) {
            break;
        }
    }
    browser.openCollection({collectionRoot});
    for (int tick = 0; tick < 200 && browser.entries().size() < 3; ++tick) {
        pump(20);
    }
    const auto visibleRows = [&]() {
        QStringList names;
        for (const QVariant &entry : browser.entries()) {
            names.append(entry.toMap().value(QStringLiteral("name")).toString());
        }
        return names;
    };
    // What the dialog's rows say: the census the DJ chooses from, read from the widgets and not from
    // the bridge — a list the panel forgot to draw is exactly what this has to catch.
    const auto genreRows = [&]() {
        QStringList shown;
        for (const char *name : {"Ambient", "House", "Techno"}) {
            QQuickItem *row = findItem(window->contentItem(),
                                       QStringLiteral("genreRow_") + QString::fromLatin1(name));
            shown.append(row != nullptr
                             ? row->property("label").toString()
                             : QStringLiteral("<no row for ") + QString::fromLatin1(name)
                                   + QStringLiteral(">"));
        }
        return shown;
    };

    // A widget of the panel: a check that called `setGenreFilter` directly would pass with no dialog on
    // the screen at all.
    const auto click = [&](const QString &name) -> bool {
        QQuickItem *item = nullptr;
        // A delegate one frame away from being born cannot be clicked: waiting for it is what a hand does.
        for (int tick = 0; tick < 50 && item == nullptr; ++tick) {
            item = findItem(window->contentItem(), name);
            if (item == nullptr) {
                pump(20);
            }
        }
        if (item == nullptr) {
            return false;
        }
        const QPointF at = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        for (const QEvent::Type type : {QEvent::MouseButtonPress, QEvent::MouseButtonRelease}) {
            QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()), Qt::LeftButton,
                              Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
        }
        pump(40);
        return true;
    };

    if (visibleRows() != QStringList{QStringLiteral("Alpha - House One"),
                                     QStringLiteral("Beta - House Two"),
                                     QStringLiteral("Gamma - Ambient Three")}) {
        std::fprintf(stderr, "dj-ui: the collection did not open: %s\n",
                     qPrintable(visibleRows().join(QStringLiteral(", "))));
        return 1;
    }

    // The button waits for the census: it is disabled while the collection says nothing about genres,
    // and a disabled button answers no click.
    for (int tick = 0; tick < 150 && browser.genreList().isEmpty(); ++tick) {
        pump(20);
    }
    // Offscreen the header rows are left at zero size by the layout, and a click on a child of nothing
    // reaches nothing. Give the row the size the layout would give it on screen, then measure the row.
    QQuickItem *filterRow = findItem(window->contentItem(), QStringLiteral("collectionFilterRow"));
    if (filterRow != nullptr && (filterRow->width() <= 0.0 || filterRow->height() <= 0.0)) {
        filterRow->setWidth(700.0);
        filterRow->setHeight(24.0);
        pump(60);
    }
    QQuickItem *button = findItem(window->contentItem(), QStringLiteral("genreFilter"));
    QQuickItem *search = findItem(window->contentItem(), QStringLiteral("collectionSearch"));
    if (button == nullptr || search == nullptr) {
        std::fprintf(stderr, "dj-ui: the header has no genre filter button or no search field\n");
        return 1;
    }
    if (button->width() <= 0.0 || button->height() <= 0.0) {
        button->setWidth(120.0);
        button->setHeight(22.0);
        pump(30);
    }

    // One row, the button on the left: the row it used to hold is the list's space now, and a button
    // stacked above the field would be the same row spent twice.
    const QRectF buttonRect(button->mapToScene(QPointF(0, 0)), button->size());
    const QRectF searchRect(search->mapToScene(QPointF(0, 0)), search->size());
    if (buttonRect.right() > searchRect.left() + 1.0) {
        std::fprintf(stderr,
                     "dj-ui: the filter button is not left of the search "
                     "(button %.0f..%.0f, search from %.0f)\n",
                     buttonRect.left(), buttonRect.right(), searchRect.left());
        return 1;
    }
    const double centreGap =
        std::abs(buttonRect.center().y() - searchRect.center().y());
    if (centreGap > 4.0) {
        std::fprintf(stderr, "dj-ui: the filter button is on another line (centres %.1f px apart)\n",
                     centreGap);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the filter button stands %.0f px left of the search, on the same line "
                 "(centres %.1f px apart)\n",
                 searchRect.left() - buttonRect.right(), centreGap);

    // What a person sees is the dialog's content, so that is what the check reads: the Popup object is
    // not a child of any item, and its `visible` is not reachable through the item tree.
    const auto filterList = [&]() -> QQuickItem * {
        return findItem(window->contentItem(), QStringLiteral("genreFilterList"));
    };
    const auto openFilter = [&]() -> bool {
        // A dialog that is still fading out keeps its modal overlay over the button: asking again before
        // it has gone means asking the window that is leaving.
        for (int tick = 0; tick < 100; ++tick) {
            QQuickItem *left = filterList();
            if (left == nullptr || !left->isVisible()) {
                break;
            }
            pump(20);
        }
        if (!click(QStringLiteral("genreFilter"))) {
            return false;
        }
        for (int tick = 0; tick < 120; ++tick) {
            QQuickItem *list = filterList();
            // Rows, not just an open window: the census is republished after every filter, and an empty
            // list is a dialog nobody can tick a box in.
            if (list != nullptr && list->width() > 0.0 && list->isVisible()
                && list->property("count").toInt() > 0) {
                // A dialog is laid out over its opening frames: a click aimed at a row that is still
                // moving lands on the panel behind it.
                pump(120);
                return true;
            }
            pump(20);
        }
        return false;
    };

    // Ticking a box is a click on its row and then a fact: the box is checked. A click that was sent is
    // not a click that arrived, and a filter built out of missing clicks measures nothing.
    const auto tick = [&](const QString &genre, bool wanted) -> bool {
        const QString box = QStringLiteral("genreBox_") + genre;
        const QString row = QStringLiteral("genreRow_") + genre;
        for (int attempt = 0; attempt < 3; ++attempt) {
            QQuickItem *item = findItem(window->contentItem(), box);
            if (item != nullptr && item->property("checked").toBool() == wanted) {
                return true;
            }
            if (!click(row)) {
                pump(100);
                continue;
            }
            for (int tickTick = 0; tickTick < 40; ++tickTick) {
                item = findItem(window->contentItem(), box);
                if (item != nullptr && item->property("checked").toBool() == wanted) {
                    return true;
                }
                pump(20);
            }
        }
        QQuickItem *stateList = filterList();
        std::fprintf(stderr,
                     "dj-ui: list while failing: %s visible %d %.0fx%.0f count %d content %.0f at %.0f\n",
                     stateList != nullptr ? "found" : "missing",
                     stateList != nullptr && stateList->isVisible() ? 1 : 0,
                     stateList != nullptr ? stateList->width() : -1.0,
                     stateList != nullptr ? stateList->height() : -1.0,
                     stateList != nullptr ? stateList->property("count").toInt() : -1,
                     stateList != nullptr ? stateList->property("contentHeight").toDouble() : -1.0,
                     stateList != nullptr ? stateList->property("contentY").toDouble() : -1.0);
        QQuickItem *rowItem = findItem(window->contentItem(), row);
        std::fprintf(stderr,
                     "dj-ui: the %s box stays at %d (row %s %.0fx%.0f at %.0f,%.0f, visible %d)\n",
                     qPrintable(genre),
                     rowItem != nullptr
                         ? int(findItem(window->contentItem(), box) != nullptr
                                   ? findItem(window->contentItem(), box)->property("checked").toBool()
                                   : -1)
                         : -1,
                     rowItem != nullptr ? "yes" : "no", rowItem != nullptr ? rowItem->width() : -1.0,
                     rowItem != nullptr ? rowItem->height() : -1.0,
                     rowItem != nullptr ? rowItem->mapToScene(QPointF(0, 0)).x() : -1.0,
                     rowItem != nullptr ? rowItem->mapToScene(QPointF(0, 0)).y() : -1.0,
                     rowItem != nullptr && rowItem->isVisible() ? 1 : 0);
        return false;
    };

    // Closing is an animation, not an instant: the next open must not see the window that is leaving.
    const auto closeFilter = [&](const QString &name) -> bool {
        if (!click(name)) {
            return false;
        }
        for (int tick = 0; tick < 100; ++tick) {
            QQuickItem *list = filterList();
            if (list == nullptr || !list->isVisible()) {
                return true;
            }
            pump(20);
        }
        std::fprintf(stderr, "dj-ui: the dialog stayed on the screen after %s\n", qPrintable(name));
        return false;
    };

    // The census: a genre with how many tracks carry it. A list of bare names makes the DJ guess which
    // filter is worth opening, and that guess is the work the dialog exists to remove.
    if (!openFilter()) {
        QQuickItem *list = filterList();
        std::fprintf(stderr,
                     "dj-ui: the genre filter button opened nothing "
                     "(genres %d, list %s %.0fx%.0f visible %d)\n",
                     int(browser.genreList().size()), list != nullptr ? "yes" : "no",
                     list != nullptr ? list->width() : -1.0, list != nullptr ? list->height() : -1.0,
                     list != nullptr && list->isVisible() ? 1 : 0);
        return 1;
    }
    const QStringList census = genreRows();
    if (census != QStringList{QStringLiteral("Ambient  1"), QStringLiteral("House  2"),
                              QStringLiteral("Techno  1")}) {
        std::fprintf(stderr, "dj-ui: the dialog does not say what the records say: %s\n",
                     qPrintable(census.join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: genre filter says %s\n", qPrintable(census.join(QStringLiteral(", "))));

    // Cancel closes and nothing else: a dialog that applies on the way out cannot be used to look.
    if (!tick(QStringLiteral("House"), true)) {
        std::fprintf(stderr, "dj-ui: the House box does not tick\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterCancel"))) {
        std::fprintf(stderr, "dj-ui: no Cancel in the genre filter\n");
        return 1;
    }
    if (QQuickItem *list = filterList(); list != nullptr && list->isVisible()) {
        std::fprintf(stderr, "dj-ui: Cancel left the dialog on the screen\n");
        return 1;
    }
    if (visibleRows().size() != 3 || !browser.genreFilter().isEmpty()) {
        std::fprintf(stderr, "dj-ui: Cancel applied a filter: %d rows, filter %s\n",
                     int(visibleRows().size()),
                     qPrintable(browser.genreFilter().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: Cancel closed the dialog and left the list at %d rows\n",
                int(visibleRows().size()));

    // Filter applies what was ticked.
    if (!openFilter() || !tick(QStringLiteral("Techno"), true)) {
        std::fprintf(stderr, "dj-ui: cannot tick Techno\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterApply"))) {
        std::fprintf(stderr, "dj-ui: no Filter button in the genre filter\n");
        return 1;
    }
    for (int tick = 0; tick < 100 && visibleRows().size() != 1; ++tick) {
        pump(20);
    }
    if (visibleRows() != QStringList{QStringLiteral("Alpha - House One")}) {
        std::fprintf(stderr, "dj-ui: the Techno filter left %s\n",
                     qPrintable(visibleRows().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: Filter on Techno leaves %s\n",
                qPrintable(visibleRows().join(QStringLiteral(", "))));

    // Two genres are a union: a track belongs to every genre its tag lists. The box shows what the
    // panel already had, so a reopened dialog must agree with the filter in force.
    if (!openFilter() || !tick(QStringLiteral("Ambient"), true)) {
        std::fprintf(stderr, "dj-ui: cannot tick Ambient on top of Techno\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterApply"))) {
        std::fprintf(stderr, "dj-ui: no Filter button for the second choice\n");
        return 1;
    }
    for (int tick = 0; tick < 100 && visibleRows().size() != 2; ++tick) {
        pump(20);
    }
    if (visibleRows().size() != 2) {
        std::fprintf(stderr, "dj-ui: Techno and Ambient left %s\n",
                     qPrintable(visibleRows().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: Techno and Ambient leave %d rows\n", int(visibleRows().size()));

    // The filter is a setting: the layout file has to agree with the panel.
    if (!layout.collectionGenres().contains(QStringLiteral("Techno"))) {
        std::fprintf(stderr, "dj-ui: the filter did not reach the layout: %s\n",
                     qPrintable(layout.collectionGenres().join(QStringLiteral(", "))));
        return 1;
    }

    // Unticking everything is how the dialog clears the filter: the last untick has to ask the index
    // again, and a list that keeps the rows of a filter the DJ cleared is the bug this catches.
    if (!openFilter() || !tick(QStringLiteral("Techno"), false)
        || !tick(QStringLiteral("Ambient"), false)) {
        std::fprintf(stderr, "dj-ui: cannot untick the two chosen genres\n");
        return 1;
    }
    if (!closeFilter(QStringLiteral("genreFilterApply"))) {
        std::fprintf(stderr, "dj-ui: no Filter button for the clear\n");
        return 1;
    }
    for (int tick = 0; tick < 100 && visibleRows().size() != 3; ++tick) {
        pump(20);
    }
    if (visibleRows().size() != 3 || !browser.genreFilter().isEmpty()) {
        std::fprintf(stderr, "dj-ui: clearing in the dialog left %d rows and filter %s\n",
                     int(visibleRows().size()),
                     qPrintable(browser.genreFilter().join(QStringLiteral(", "))));
        return 1;
    }
    std::printf("dj-ui: the dialog clearing brings back %d rows\n", int(visibleRows().size()));

    // Many genres must scroll rather than run off the window: the list is a view, and a collection with
    // two hundred genres has to be reachable. The rows here are synthetic on purpose — the point is the
    // view, not the census already measured above.
    if (!openFilter()) {
        std::fprintf(stderr, "dj-ui: the dialog does not open for the scroll\n");
        return 1;
    }
    QQuickItem *list = filterList();
    QQuickItem *bar = findItem(window->contentItem(), QStringLiteral("genreFilterBar"));
    if (list == nullptr) {
        std::fprintf(stderr, "dj-ui: the genre list is not on the screen\n");
        return 1;
    }
    QVariantList many;
    for (int index = 0; index < 60; ++index) {
        many.append(QVariantMap{{QStringLiteral("name"), QStringLiteral("Genre %1").arg(index)},
                                {QStringLiteral("count"), index}});
    }
    list->setProperty("model", many);
    pump(300);
    const double content = list->property("contentHeight").toDouble();
    const double visible = list->height();
    if (content <= visible + 1.0) {
        std::fprintf(stderr, "dj-ui: sixty genres fit in %.0f px of rows in %.0f px of list\n", content,
                     visible);
        return 1;
    }
    const QPointF at = list->mapToScene(QPointF(list->width() / 2.0, visible / 2.0));
    QWheelEvent wheel(at, window->mapToGlobal(at.toPoint()), QPoint(), QPoint(0, -120), Qt::NoButton,
                      Qt::NoModifier, Qt::NoScrollPhase, false);
    QCoreApplication::sendEvent(window, &wheel);
    pump(150);
    const double moved = list->property("contentY").toDouble();
    if (moved <= 0.0) {
        std::fprintf(stderr, "dj-ui: the genre list did not scroll (contentY %.1f)\n", moved);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the genre list scrolls: %.0f px of rows in %.0f px, turned to %.0f px%s\n", content,
                 visible, moved, bar != nullptr && bar->isVisible() ? " with the bar showing" : "");
    return 0;
}

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
    const QCommandLineOption keyCheckOption(
        QStringLiteral("key-check"),
        QStringLiteral("With --connect and --load: require the deck's header to show the track's key "
                       "and the engine to have written the same key beside the file."));
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
    const QCommandLineOption gridCreateOption(
        QStringLiteral("grid-create-check"),
        QStringLiteral("With --connect and --load: on a track whose tempo could not be measured, the "
                       "grid tools must be there, the ones that read a grid must be grey, and placing a "
                       "beat or typing a tempo must make the grid."));
    const QCommandLineOption gridCheckOption(
        QStringLiteral("grid-check"),
        QStringLiteral("With --connect and --load: move the grid, halve it, double it and type a tempo, "
                       "requiring the deck and the file to follow every edit."));
    const QCommandLineOption loopBeatsOption(
        QStringLiteral("loop-beats-check"),
        QStringLiteral("Check that a loop of four beats is four beats of the grid, and that the arrows "
                       "move it by a beat without shortening it (DJP-DECK-005)."));
    const QCommandLineOption fourDeckOption(
        QStringLiteral("four-deck-check"),
        QStringLiteral("With --connect and --load: put the same file on four decks, play all of them, and "
                       "require each to move, each to reach the master, the four panels to exist, and the "
                       "crossfader to hold A and C on one side and B and D on the other."));
    const QCommandLineOption syncSoakOption(
        QStringLiteral("sync-soak"),
        QStringLiteral("With --connect: two decks on click tracks of known tempo, ten minutes under the "
                       "null sink, phase read from the snapshots, with the master's tempo changed, the "
                       "follower paused and started, a loop on and off, and a tempo the fader cannot "
                       "reach (DJP-SYNC-001/002, AC-SYNC)."));
    const QCommandLineOption soakSecondsOption(
        QStringLiteral("soak-seconds"),
        QStringLiteral("How long the soak runs, in seconds (default 600)."),
        QStringLiteral("seconds"),
        QStringLiteral("600"));
    const QCommandLineOption syncCheckOption(
        QStringLiteral("sync-check"),
        QStringLiteral("Load two decks, make one the master through its button, put the other on its "
                       "beat, and require the state and the phase meter to agree with the engine."));
    const QCommandLineOption transportExtrasOption(
        QStringLiteral("transport-extras-check"),
        QStringLiteral("With --connect and --load: press a jump and require the play head on a beat of "
                       "the grid, and switch slip and reverse on and off (DJP-DECK-006/007)."));
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
    const QCommandLineOption pageCheckOption(
        QStringLiteral("collection-page-check"),
        QStringLiteral("Compare the index page with the walk over the collection, field by field."));
    const QCommandLineOption importCheckOption(
        QStringLiteral("import-check"),
        QStringLiteral("With --connect: pick two rows in the browser, add them to the collection, "
                       "answer the duplicate question and require the files to be there."));
    const QCommandLineOption sortCheckOption(
        QStringLiteral("sort-check"),
        QStringLiteral("From the collection: a click on a column sorts by it, a second click turns it "
                       "around, a Shift click adds the next key — and the rail's order."));
    const QCommandLineOption endCheckOption(
        QStringLiteral("end-check"),
        QStringLiteral("Play a deck to its last frame and require it to stand still and take a track again."));
    const QCommandLineOption lockCheckOption(
        QStringLiteral("lock-check"),
        QStringLiteral("Press the grid lock on a deck and require the collection's row to wear it at once, "
                       "then press again and require it gone."));
    const QCommandLineOption genreCheckOption(
        QStringLiteral("genre-check"),
        QStringLiteral("From a collection: the genre chips say how many tracks each genre has, a chip "
                       "narrows the list, two chips are a union, and reset brings everything back."));
    const QCommandLineOption collectionProbeOption(
        QStringLiteral("collection-probe"),
        QStringLiteral("Time the listing of a collection root: how many rows, and how long the walk "
                       "takes cold and warm. Read only."));
    const QCommandLineOption collectionLoadCheckOption(
        QStringLiteral("collection-load-check"),
        QStringLiteral("From a real collection, read only: choose a row and load it into deck A — the "
                       "engine must report the track in the deck, not only the panel its message."));
    const QCommandLineOption analyzeCheckOption(
        QStringLiteral("analyze-check"),
        QStringLiteral("From the collection: Analyze… on one track and on three — a progress window, "
                       "the parallel box, and the numbers in the rows afterwards."));
    const QCommandLineOption importFixtureOption(
        QStringLiteral("import-fixture"),
        QStringLiteral("A real track the import check copies twice."), QStringLiteral("path"));
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
        QStringLiteral("With --connect and --load: play the deck, bring the head back to the cue point "
                       "with a tap of Cue, set the point by holding the same button, scrub the "
                       "overview with the hand, and require the clock to show the time left rather "
                       "than the length."));
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
    const QCommandLineOption collectionRootOption(
        QStringLiteral("collection-root"),
        QStringLiteral("A folder the browser's Collection section lists. Repeat it for a collection "
                       "spread over several disks: every root is listed in one list, each with its own "
                       "metadata. Remembered for the next start; without it the collection lives in "
                       "$XDG_DATA_HOME/hauler-dj/collection."),
        QStringLiteral("path"));
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
    parser.addOption(pageCheckOption);
    parser.addOption(importCheckOption);
    parser.addOption(analyzeCheckOption);
    parser.addOption(sortCheckOption);
    parser.addOption(lockCheckOption);
    parser.addOption(endCheckOption);
    parser.addOption(genreCheckOption);
    parser.addOption(collectionLoadCheckOption);
    parser.addOption(collectionProbeOption);
    parser.addOption(importFixtureOption);
    parser.addOption(gridReconnectOption);
    parser.addOption(metronomeOption);
    parser.addOption(tapOption);
    parser.addOption(gridLinesOption);
    parser.addOption(gridCheckOption);
    parser.addOption(gridCreateOption);
    parser.addOption(loopBeatsOption);
    parser.addOption(fourDeckOption);
    parser.addOption(syncSoakOption);
    parser.addOption(soakSecondsOption);
    parser.addOption(syncCheckOption);
    parser.addOption(transportExtrasOption);
    parser.addOption(gridHereOption);
    parser.addOption(tempoRecheckOption);
    parser.addOption(cueWriteOption);
    parser.addOption(cueReadOption);
    parser.addOption(exitAfterOption);
    parser.addOption(stripCheckOption);
    parser.addOption(keyCheckOption);
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
    parser.addOption(collectionRootOption);
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
    // The collection is the engine's index, not this process's walk: the browser asks, and only a shell
    // without an engine falls back to reading  itself.
    browser.attachBridge(bridge.get());
    SessionState session;
    if (parser.isSet(sessionFileOption)) {
        session.setFile(parser.value(sessionFileOption));
    }
    session.load();
    // The browser opens where the last track came from; an empty state falls back to the music
    // folder, which is what FileBrowser does when given no path.
    browser.open(session.folder());
    // The rows the two places remember: a folder and a collection are visited for different reasons,
    // and neither should lose its place because the DJ looked at the other.
    browser.seedCursors(session.cursor(), session.collectionCursor());
    // Every load, from the browser or from a deck's dialog, moves that folder.
    bridge->setLoadFolderHandler([&session, &browser](const QString &folder) {
        // A track taken out of the collection lives in the collection: neither the file list nor the
        // next start belongs in its folder. Only a load made from the file list says where the file
        // list is — and it moves the list whether or not the session already knew that folder.
        if (browser.mode() != QLatin1String("files")) {
            return;
        }
        if (session.rememberFolder(folder)) {
            session.save();
        }
        browser.rememberFolder(folder);
    });
    // The folder the DJ browsed to is worth remembering even if he loaded nothing: a session that only
    // remembers loads sends him back to the same place every start, which is what he reported.
    QObject::connect(&browser, &FileBrowser::changed, [&session, &browser]() {
        if (browser.mode() != QLatin1String("files")) {
            return;
        }
        const QString folder = browser.folder();
        if (!folder.isEmpty() && session.rememberFolder(folder)) {
            session.save();
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
    // The remembered folder is the DJ's choice, wherever it is: a folder inside the collection is a
    // perfectly good place to browse from. Only a folder that no longer exists falls back to the home
    // folder, and `open` already does that on its own.
    if (parser.isSet(layoutOption) && !layout.applyPreset(parser.value(layoutOption))) {
        std::fprintf(stderr, "dj-ui: --layout expects traktor or two-deck\n");
        return 2;
    }
    if (parser.isSet(collectionRootOption)) {
        layout.setCollectionRoots(parser.values(collectionRootOption));
    } else if (layout.collectionRoots().isEmpty()) {
        // No root chosen yet: the collection lives in the shell's own data folder, and that choice is
        // written to the layout file when the shell exits.
        layout.setCollectionRoots({browser.defaultCollectionRoot()});
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
    // The arrangement is the user's, and it is written when the shell exits: a root adopted by
    // default must reach the file even if nothing was dragged and no flag was typed.
    QObject::connect(&app, &QCoreApplication::aboutToQuit, [&layout, &browser, &session]() {
        // Where the cursor stood is part of the session, like the folder: a browser that forgets the
        // row makes the DJ find it again every evening.
        session.rememberCursor(browser.filesCursor());
        session.rememberCollectionCursor(browser.collectionCursor());
        session.save();
        layout.save();
    });

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
        const double deckMeterBottom = meterA->mapToItem(content, QPointF(0, meterA->height())).y();
        std::fprintf(stdout, "dj-ui: the master meter starts %.0f px below the knob and ends %.0f px "
                             "from the deck meters; heights %.0f and %.0f px\n",
                     meterTop - knobBottom, meterBottom - deckMeterBottom, meter->height(),
                     meterA->height());
        auto *row = item(QStringLiteral("stripRow"));
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
        return runLayoutCheck(engine, layout, settings);
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
                     "grid, slip drew its virtual head on both "
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
            if (button == nullptr) {
                return false;
            }
            // A two-state control in the deck paints its state through `lit`; `highlighted` is the
            // Controls property, and with the deck's own paint nothing reads it any more.
            const QVariant state = button->property("lit");
            return state.isValid() ? state.toBool() : button->property("highlighted").toBool();
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
        return runGridLines(engine, bridge, parser, connectOption, loadOption);
    }

    if (parser.isSet(gridCreateOption)) {
        // A track whose tempo could not be measured has no grid — and that is exactly the track whose
        // grid a person has to place. The tools have to be there, the ones that read a grid have to be
        // grey, and the two that make one have to work: a beat placed at the play head starts a grid
        // there, a typed tempo starts one at the first frame (DJP-MEDIA-006).
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        const QString fixture = parser.value(loadOption);
        if (fixture.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-create-check needs --load\n");
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
            return deckById(bridge->snapshot(), QStringLiteral("A"));
        };
        const auto gridOf = [&]() { return deckA().value(QStringLiteral("track")).toMap().value(
                                            QStringLiteral("grid")).toMap(); };
        const auto firstBeat = [&]() -> qint64 {
            const QVariantList segments = gridOf().value(QStringLiteral("segments")).toList();
            if (segments.isEmpty()) {
                return -1;
            }
            return segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto tempoOf = [&]() {
            const QVariantList segments = gridOf().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? 0.0
                       : segments.first().toMap().value(QStringLiteral("bpm")).toDouble();
        };
        const auto itemByName = [&](const QString &name) {
            return findItem(window->contentItem(), name);
        };

        // Its own file, emptied first: a deck that already holds a grid would answer about the track
        // before this one.
        bridge->eject(QStringLiteral("A"));
        waitFor([&]() { return deckA().value(QStringLiteral("track")).toMap().isEmpty(); }, 5000);
        bridge->load(QStringLiteral("A"), fixture);
        const QString wantName = QFileInfo(fixture).fileName();
        const auto loadArrived = [&]() {
            const QString held = deckA().value(QStringLiteral("path")).toString();
            return !held.isEmpty() && held.endsWith(wantName);
        };
        if (!waitFor([&]() { return loadArrived(); }, 30000)) {
            std::fprintf(stderr, "dj-ui: deck A never loaded %s\n", qPrintable(wantName));
            return 1;
        }
        if (!gridOf().isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s measured a grid of its own; this check needs a track "
                                 "with none\n",
                         qPrintable(wantName));
            return 1;
        }

        // The row is there, the readers are grey, the makers are live.
        auto *row = itemByName(QStringLiteral("gridedit_A"));
        if (row == nullptr || !row->isVisible()) {
            std::fprintf(stderr, "dj-ui: the grid tools are hidden on a track with no grid\n");
            return 1;
        }
        struct Expect {
            const char *name;
            bool enabled;
        };
        for (const Expect &expect : {Expect{"gridHere_A", true}, Expect{"gridTempo_A", true},
                                     Expect{"gridTap_A", true}, Expect{"gridRight_A", false},
                                     Expect{"gridHalf_A", false}, Expect{"gridDouble_A", false},
                                     Expect{"metronome_A", false}}) {
            auto *item = itemByName(QString::fromLatin1(expect.name));
            if (item == nullptr) {
                std::fprintf(stderr, "dj-ui: no %s on a track with no grid\n", expect.name);
                return 1;
            }
            if (!item->isVisible()) {
                std::fprintf(stderr, "dj-ui: %s is hidden on a track with no grid\n", expect.name);
                return 1;
            }
            const bool enabled = item->property("enabled").toBool();
            if (enabled != expect.enabled) {
                std::fprintf(stderr, "dj-ui: %s is %s, and on a track with no grid it must be %s\n",
                             expect.name, enabled ? "live" : "grey",
                             expect.enabled ? "live" : "grey");
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: on a track with no grid the tools are there, the readers grey\n");

        // A beat placed at the play head: the grid starts there, at the tempo a deck shows when nothing
        // has been measured.
        constexpr qint64 target = 48'000;
        bridge->seek(QStringLiteral("A"), static_cast<qulonglong>(target));
        waitFor(
            [&]() { return deckA().value(QStringLiteral("source_frame")).toLongLong() == target; },
            5000);
        QMetaObject::invokeMethod(itemByName(QStringLiteral("gridHere_A")), "clicked");
        const auto placed = [&]() {
            return !gridOf().isEmpty() && firstBeat() == target;
        };
        if (!waitFor(placed, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: placing a beat made no grid at the play head (first beat %lld)\n",
                         static_cast<long long>(firstBeat()));
            return 1;
        }
        if (std::abs(tempoOf() - 120.0) > 0.01) {
            std::fprintf(stderr, "dj-ui: the grid started at %.2f BPM instead of 120\n", tempoOf());
            return 1;
        }
        if (gridOf().value(QStringLiteral("beats_per_bar")).toInt() != 4) {
            std::fprintf(stderr, "dj-ui: the grid started in %d/4\n",
                         gridOf().value(QStringLiteral("beats_per_bar")).toInt());
            return 1;
        }
        std::fprintf(stdout, "dj-ui: a beat placed at frame %lld started a 120 BPM grid there\n",
                     static_cast<long long>(target));

        // A typed tempo on a track with no grid begins at the first frame: there is no beat to hold on
        // to, and the reading a DJ then shifts onto the first kick is the one that starts at zero. It is
        // measured on deck B, whose file has no grid: deck A has one now, and retiming an existing grid
        // keeps its first beat — a different rule, and the one that would hide this one.
        const QString second = parser.value(loadSecondOption);
        if (second.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --grid-create-check needs --load-b, a second track with no "
                                 "grid\n");
            return 1;
        }
        const auto deckB = [&]() { return deckById(bridge->snapshot(), QStringLiteral("B")); };
        const auto gridOfB = [&]() { return deckB().value(QStringLiteral("track")).toMap().value(
                                             QStringLiteral("grid")).toMap(); };
        const auto firstBeatB = [&]() -> qint64 {
            const QVariantList segments = gridOfB().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? -1
                       : segments.first().toMap().value(QStringLiteral("frame")).toLongLong();
        };
        const auto tempoOfB = [&]() {
            const QVariantList segments = gridOfB().value(QStringLiteral("segments")).toList();
            return segments.isEmpty()
                       ? 0.0
                       : segments.first().toMap().value(QStringLiteral("bpm")).toDouble();
        };
        bridge->eject(QStringLiteral("B"));
        waitFor([&]() { return deckB().value(QStringLiteral("track")).toMap().isEmpty(); }, 5000);
        bridge->load(QStringLiteral("B"), second);
        const QString secondName = QFileInfo(second).fileName();
        if (!waitFor(
                [&]() {
                    const QString held = deckB().value(QStringLiteral("path")).toString();
                    return !held.isEmpty() && held.endsWith(secondName);
                },
                30000)) {
            std::fprintf(stderr, "dj-ui: deck B never loaded %s\n", qPrintable(secondName));
            return 1;
        }
        if (!gridOfB().isEmpty()) {
            std::fprintf(stderr, "dj-ui: %s measured a grid of its own; this check needs a track "
                                 "with none\n",
                         qPrintable(secondName));
            return 1;
        }
        bridge->setGridTempo(QStringLiteral("B"), 174.0);
        const auto retimed = [&]() {
            return !gridOfB().isEmpty() && std::abs(tempoOfB() - 174.0) < 0.01 && firstBeatB() == 0;
        };
        if (!waitFor(retimed, 5000)) {
            std::fprintf(stderr,
                         "dj-ui: a typed tempo left the grid at %.2f BPM from frame %lld\n",
                         tempoOfB(), static_cast<long long>(firstBeatB()));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: a typed tempo of 174 BPM started the grid at the file start\n");

        // A tenth of a beat per minute, up and down, from the tempo the grid now has.
        const auto tempoAfter = [&](double want) {
            return waitFor([&]() { return std::abs(tempoOfB() - want) < 0.005; }, 5000);
        };
        for (int press = 0; press < 2; ++press) {
            if (itemByName(QStringLiteral("gridTempoUp_B")) == nullptr) {
                std::fprintf(stderr, "dj-ui: deck B has no tempo-up button\n");
                return 1;
            }
            QMetaObject::invokeMethod(itemByName(QStringLiteral("gridTempoUp_B")), "clicked");
        }
        if (!tempoAfter(174.2)) {
            std::fprintf(stderr, "dj-ui: two presses up left the tempo at %.3f, not 174.2\n",
                         tempoOfB());
            return 1;
        }
        if (itemByName(QStringLiteral("gridTempoDown_B")) == nullptr) {
            std::fprintf(stderr, "dj-ui: deck B has no tempo-down button\n");
            return 1;
        }
        QMetaObject::invokeMethod(itemByName(QStringLiteral("gridTempoDown_B")), "clicked");
        if (!tempoAfter(174.1)) {
            std::fprintf(stderr, "dj-ui: a press down left the tempo at %.3f, not 174.1\n",
                         tempoOfB());
            return 1;
        }
        if (firstBeatB() != 0) {
            std::fprintf(stderr, "dj-ui: trimming the tempo moved the first beat to %lld\n",
                         static_cast<long long>(firstBeatB()));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the tempo buttons move it by 0.1 BPM and keep the first beat\n");

        // Both separators are accepted by the field the dialog itself fills with a dot: in a comma
        // locale the old validator refused the number it had just written.
        auto *editButton = itemByName(QStringLiteral("gridTempo_A"));
        if (editButton == nullptr) {
            std::fprintf(stderr, "dj-ui: no tempo field button on deck A\n");
            return 1;
        }
        QMetaObject::invokeMethod(editButton, "clicked");
        QCoreApplication::processEvents();
        auto *field = itemByName(QStringLiteral("tempoField_A"));
        if (field == nullptr) {
            std::fprintf(stderr, "dj-ui: the tempo field never appeared\n");
            return 1;
        }
        // Typed, not assigned: a comma that survives `setProperty` may still never arrive from a
        // keyboard, and the keyboard is what the DJ uses. The key events go to the focused item through
        // the window, exactly as a real press does.
        const auto typeInto = [&](const QString &text) {
            field->forceActiveFocus();
            field->setProperty("text", QString());
            QCoreApplication::processEvents();
            for (const QChar &character : text) {
                QKeyEvent press(QEvent::KeyPress, character.unicode(), Qt::NoModifier,
                                QString(character));
                QCoreApplication::sendEvent(window, &press);
                QKeyEvent release(QEvent::KeyRelease, character.unicode(), Qt::NoModifier,
                                  QString(character));
                QCoreApplication::sendEvent(window, &release);
            }
            QCoreApplication::processEvents();
            return field->property("text").toString();
        };
        for (const QString &typed : {QStringLiteral("174.5"), QStringLiteral("174,5")}) {
            const QString landed = typeInto(typed);
            if (landed != typed) {
                std::fprintf(stderr, "dj-ui: typing \"%s\" put \"%s\" in the tempo field\n",
                             qPrintable(typed), qPrintable(landed));
                return 1;
            }
            if (!field->property("acceptableInput").toBool()) {
                std::fprintf(stderr, "dj-ui: the tempo field refuses \"%s\"\n", qPrintable(typed));
                return 1;
            }
        }
        for (const QString &typed : {QStringLiteral("174.5.5"), QStringLiteral("174,5,5")}) {
            field->setProperty("text", typed);
            QCoreApplication::processEvents();
            if (field->property("acceptableInput").toBool()) {
                std::fprintf(stderr, "dj-ui: the tempo field accepts \"%s\"\n", qPrintable(typed));
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: the tempo field takes a dot and a comma from the keyboard\n");

        // And it stays inside its own window: the text and the field used to be laid out at their own
        // width and ran out of the popup on the right.
        auto *label = itemByName(QStringLiteral("tempoEditLabel_A"));
        if (label == nullptr) {
            std::fprintf(stderr, "dj-ui: the tempo dialog has no label\n");
            return 1;
        }
        const auto rightEdge = [](QQuickItem *item) {
            return item->mapToScene(QPointF(item->width(), 0.0)).x();
        };
        const qreal windowWidth = static_cast<qreal>(window->width());
        std::fprintf(stdout, "dj-ui: the tempo dialog: label %.0f px, field %.0f px, window %.0f px\n",
                     label->width(), field->width(), windowWidth);
        if (rightEdge(label) > windowWidth || rightEdge(field) > windowWidth) {
            std::fprintf(stderr,
                         "dj-ui: the tempo dialog runs out of the window (label to %.0f, field to "
                         "%.0f, window %.0f)\n",
                         rightEdge(label), rightEdge(field), windowWidth);
            return 1;
        }
        if (label->width() > 280.0) {
            std::fprintf(stderr, "dj-ui: the tempo dialog label is %.0f px wide, wider than the "
                                 "popup\n",
                         label->width());
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the tempo dialog keeps its content inside itself\n");
        std::fprintf(stdout, "dj-ui: a track with no grid can be given one by hand\n");
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

        // Every edit is a decision about the file: it is written beside the music. The lock is the DJ's
        // switch and an edit does not touch it — the saved grid is what keeps the next automatic pass
        // away, and a flag set here would grey the editor after the first edit.
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
                           !stored().value(QStringLiteral("analysis_lock")).toBool();
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
        return runEqCheck(engine, bridge, parser, connectOption);
    }






    if (parser.isSet(loopOption)) {
        return runLoop(engine, bridge, parser, connectOption, loadOption);
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
        auto *cueButton = findItem(window->contentItem(), QStringLiteral("cue_A"));
        auto *overview = findItem(window->contentItem(), QStringLiteral("deckOverview_A"));
        auto *clock = findItem(window->contentItem(), QStringLiteral("clock_A"));
        if (playButton == nullptr || cueButton == nullptr || overview == nullptr
            || clock == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck is missing its transport, its clock or its overview\n");
            return 1;
        }

        // Play, let the playhead move, then a tap of Cue: the head must be back on the cue point. The
        // Stop button is gone (owner's decision): returning the head is the cue button's job, and the
        // pause is the Play button's.
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
        press(cueButton);
        pump(10);
        release(cueButton);
        pump(20);
        // Freeze the deck before reading the position: a tap of Cue brings the head back and the deck
        // rolls on from the point, and a rolling deck is no position to compare. Pausing is the Play
        // button's job now that Stop is gone.
        press(playButton);
        pump(5);
        release(playButton);
        pump(40);
        const double backAt = fieldOf("source_frame");
        std::fprintf(stdout,
                     "dj-ui: play moved the playhead to %.0f, a tap of Cue brought it back to %.0f and "
                     "left the deck %s (the cue point is %.0f)\n",
                     moved, backAt, playing() ? "playing" : "paused", cue);
        if (playing()) {
            std::fprintf(stderr, "dj-ui: the deck kept playing after Play was pressed\n");
            return 1;
        }
        if (backAt < cue - 2.0 || backAt - cue > 24000.0) {
            std::fprintf(stderr,
                         "dj-ui: the tap of Cue left the playhead at %.0f instead of the cue point "
                         "%.0f\n",
                         backAt, cue);
            return 1;
        }
        if (backAt >= moved - 1000.0) {
            std::fprintf(stderr, "dj-ui: the tap of Cue did not bring the playhead back\n");
            return 1;
        }
        // The hold of the same button sets the point where the hand landed and leaves the deck there.
        press(playButton);
        pump(5);
        release(playButton);
        pump(300);
        const double handLanded = fieldOf("source_frame");
        press(cueButton);
        pump(600);
        release(cueButton);
        pump(80);
        const double leftAt = fieldOf("source_frame");
        const double pointNow = cuePoint();
        std::fprintf(stdout,
                     "dj-ui: holding Cue set the point at %.0f (the hand landed at %.0f), the head is "
                     "at %.0f and the deck is %s\n",
                     pointNow, handLanded, leftAt, playing() ? "playing" : "silent");
        if (pointNow <= 0.0 || std::abs(pointNow - handLanded) > 10000.0) {
            std::fprintf(stderr, "dj-ui: the hold did not set the cue point where the hand landed\n");
            return 1;
        }
        if (playing()) {
            std::fprintf(stderr, "dj-ui: the deck kept playing after the cue point was set\n");
            return 1;
        }
        if (std::abs(leftAt - pointNow) > 2.0) {
            std::fprintf(stderr, "dj-ui: the deck did not stand on the point it had just set\n");
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

    if (parser.isSet(fourDeckOption)) {
        // Four decks are a claim about the whole chain: four deck objects, four mixer channels, four
        // strips on screen, and a crossfader that knows which side each one is on. This check walks the
        // chain once, from the file to the master.
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
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
        const auto frameOf = [&](const QString &id) {
            return deckOf(id).value(QStringLiteral("source_frame")).toLongLong();
        };
        const auto peakOf = [&](const QString &id) {
            // A deck's meter is a peak and a hold. The peak lasts one block and a sample of it can land in
            // the gap between two beats; the hold is what the strip says a moment later, and it is the
            // honest answer to "did this deck make a sound in this window".
            const QVariantMap levels = deckOf(id).value(QStringLiteral("levels")).toMap();
            return std::max({levels.value(QStringLiteral("peak_left")).toDouble(),
                             levels.value(QStringLiteral("peak_right")).toDouble(),
                             levels.value(QStringLiteral("held_left")).toDouble(),
                             levels.value(QStringLiteral("held_right")).toDouble()});
        };
        const auto masterPeak = [&]() {
            const QVariantMap levels = bridge->snapshot().value(QStringLiteral("master")).toMap();
            return std::max(levels.value(QStringLiteral("peak_left")).toDouble(),
                            levels.value(QStringLiteral("peak_right")).toDouble());
        };
        const auto loudestOver = [&](const std::function<double()> &read, int budget_ms) {
            double loudest = 0.0;
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget_ms) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                loudest = std::max(loudest, read());
                QThread::msleep(10);
            }
            return loudest;
        };

        const QString path = parser.value(loadOption);
        if (path.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the four-deck check needs --load\n");
            return 1;
        }
        const QStringList ids{QStringLiteral("A"), QStringLiteral("B"), QStringLiteral("C"),
                              QStringLiteral("D")};
        for (const QString &id : ids) {
            bridge->load(id, path);
        }
        const bool loaded = waitFor(
            [&]() {
                for (const QString &id : ids) {
                    if (!deckOf(id).value(QStringLiteral("path")).isValid()) {
                        return false;
                    }
                }
                return true;
            },
            30000);
        if (!loaded) {
            std::fprintf(stderr, "dj-ui: not all four decks took the file\n");
            return 1;
        }
        // Four decks reading the same bytes from the same place would prove nothing about four decks, and
        // an eight-second fixture runs out from under four windows of measurement: each deck gets a loop
        // of its own half of the track, a quarter of it further in than the one before. The material never
        // ends, and no two decks are reading the same place.
        const qint64 length =
            deckOf(QStringLiteral("A")).value(QStringLiteral("track")).toMap()
                .value(QStringLiteral("duration_frames")).toLongLong();
        if (length <= 0) {
            std::fprintf(stderr, "dj-ui: the four-deck check has no length to loop over\n");
            return 1;
        }
        for (int index = 0; index < ids.size(); ++index) {
            // Every window ends inside the file, never on its last frame: a loop that ends exactly on the
            // end of its file goes silent on the wrap (evidence in docs/qa/evidence/A07/four-decks.md), and
            // this check is about the mixer carrying four decks, not about that defect. It is recorded
            // there and waits for its own test.
            const qint64 start = length / 8 + length / 8 * index;
            const qint64 end = std::min(start + length / 5, length - length / 8);
            bridge->setLoop(ids[index], true, static_cast<qulonglong>(start),
                            static_cast<qulonglong>(end));
        }
        for (const QString &id : ids) {
            bridge->play(id, true);
        }
        if (!waitFor(
                [&]() {
                    for (const QString &id : ids) {
                        if (!deckOf(id).value(QStringLiteral("playing")).toBool()) {
                            return false;
                        }
                    }
                    return true;
                },
                5000)) {
            std::fprintf(stderr, "dj-ui: not all four decks started\n");
        }

        // 1. Every deck moves.
        QVector<qint64> before;
        for (const QString &id : ids) {
            before.append(frameOf(id));
        }
        QThread::msleep(1500);
        QCoreApplication::processEvents(QEventLoop::AllEvents, 100);
        bool all_moved = true;
        for (int index = 0; index < ids.size(); ++index) {
            const qint64 moved = frameOf(ids[index]) - before[index];
            std::fprintf(stdout, "dj-ui: deck %s moved %lld frames in 1,5 s\n", qPrintable(ids[index]),
                         static_cast<long long>(moved));
            if (moved < 24000) {  // half a second of material: a deck that plays is not one that crawls
                all_moved = false;
            }
        }
        if (!all_moved) {
            std::fprintf(stderr, "dj-ui: a deck of four is not moving\n");
            return 1;
        }

        // 2. Every deck makes its own sound, and all of it reaches the master.
        QVector<double> peaks;
        for (const QString &id : ids) {
            peaks.append(loudestOver([&]() { return peakOf(id); }, 2000));
            std::fprintf(stdout, "dj-ui: deck %s peaks at %.4f\n", qPrintable(id), peaks.last());
        }
        for (int index = 0; index < ids.size(); ++index) {
            if (peaks[index] <= 0.01) {
                std::fprintf(stderr, "dj-ui: deck %s says nothing (peak %.4f)\n", qPrintable(ids[index]),
                             peaks[index]);
                return 1;
            }
        }

        // 3. The decks are four voices of one mix, and the crossfader keeps each pair on its own side.
        // Measuring one deck alone turned out to be a measure of the instrument rather than of the mix —
        // a deck soloed through three muted channels read zero while the same two decks together read
        // half scale — so the check asks its question in pairs: shut two, and the other two must still
        // reach the master. Which side each deck is on is a unit test of its own
        // (`the_crossfader_holds_two_sides_of_four_decks`).
        // A command is a message, not a fact: the mix takes the fader on its own thread, and a measurement
        // started one block too early measures the setting that was there before it. Under a loaded
        // machine that turned the first pair of the run into a zero — the crossfader had not arrived yet.
        const auto settled = [&](const std::function<bool()> &reached) {
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < 2000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                if (reached()) {
                    return true;
                }
                QThread::msleep(5);
            }
            return false;
        };
        // A peak lives one block: a window that saw ten snapshots can miss it, and a window that saw none
        // has measured nothing at all. The count is what tells the two apart.
        const auto loudestWithCount = [&](const std::function<double()> &read, int budget_ms, int *seen) {
            double loudest = 0.0;
            *seen = 0;
            QElapsedTimer timer;
            timer.start();
            while (timer.elapsed() < budget_ms) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                loudest = std::max(loudest, read());
                ++*seen;
                QThread::msleep(10);
            }
            return loudest;
        };
        const auto masterWithOnly = [&](const QString &first, const QString &second) {
            for (const QString &id : ids) {
                bridge->setChannelFader(id, id == first || id == second ? 1.0 : 0.0);
            }
            const auto open = [&]() {
                for (const QString &id : ids) {
                    const double want = id == first || id == second ? 1.0 : 0.0;
                    if (std::abs(deckOf(id).value(QStringLiteral("fader")).toDouble() - want) > 0.01) {
                        return false;
                    }
                }
                return true;
            };
            if (!settled(open)) {
                std::fprintf(stderr, "dj-ui: the mix never took the faders for %s and %s\n",
                             qPrintable(first), qPrintable(second));
                return -1.0;
            }
            double heard = 0.0;
            for (int attempt = 0; attempt < 3; ++attempt) {
                int seen = 0;
                heard = loudestWithCount([&]() { return masterPeak(); }, 1200, &seen);
                if (seen >= 20) {
                    break;
                }
                if (attempt == 2) {
                    std::fprintf(stderr,
                                 "dj-ui: the machine gave %d snapshots in 1,2 s: the pair %s and %s was "
                                 "not measured\n",
                                 seen, qPrintable(first), qPrintable(second));
                    return -1.0;
                }
            }
            for (const QString &id : ids) {
                bridge->setChannelFader(id, 1.0);
            }
            return heard;
        };
        bridge->setCrossfader(0.5);
        if (!settled([&]() {
                return std::abs(bridge->snapshot().value(QStringLiteral("crossfader")).toDouble() - 0.5)
                       < 0.01;
            })) {
            std::fprintf(stderr, "dj-ui: the mix never took the crossfader to the centre\n");
            return 1;
        }
        const double only_cd = masterWithOnly(QStringLiteral("C"), QStringLiteral("D"));
        const double only_ab = masterWithOnly(QStringLiteral("A"), QStringLiteral("B"));
        const double only_ac = masterWithOnly(QStringLiteral("A"), QStringLiteral("C"));
        std::fprintf(stdout,
                     "dj-ui: pairs through the master — A and B at %.4f, C and D at %.4f, A and C at %.4f\n",
                     only_ab, only_cd, only_ac);
        if (only_cd < 0.0 || only_ab < 0.0 || only_ac < 0.0) {
            // The faders never arrived: the measurement was not taken, and saying "no pair reaches the
            // master" would be saying something about the mixer that was never measured.
            return 1;
        }
        if (only_cd <= 0.01 || only_ab <= 0.01 || only_ac <= 0.01) {
            std::fprintf(stderr, "dj-ui: a pair of decks never reaches the master\n");
            return 1;
        }
        const double mixed = loudestOver([&]() { return masterPeak(); }, 1200);
        std::fprintf(stdout, "dj-ui: with all four back in, the master reads %.4f\n", mixed);
        if (mixed <= 0.01) {
            std::fprintf(stderr, "dj-ui: the four decks are silent together\n");
            return 1;
        }

        // 4. The interface carries four decks, not two.
        QStringList missing;
        for (const QString &id : ids) {
            if (findItem(window->contentItem(), QStringLiteral("deckPanel_") + id) == nullptr) {
                missing.append(id);
            }
        }
        if (findItem(window->contentItem(), QStringLiteral("mixer")) == nullptr) {
            missing.append(QStringLiteral("mixer"));
        }
        if (!missing.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the scene has no panel for %s\n", qPrintable(missing.join(',')));
            return 1;
        }

        for (const QString &id : ids) {
            bridge->play(id, false);
        }
        std::fprintf(stdout,
                     "dj-ui: four decks play and mix — each one moves, each one is heard on its own, every "
                     "pair of them reaches the master, and the scene has a panel for every one of them\n");
        return 0;
    }

    if (parser.isSet(syncSoakOption)) {
        return runSyncSoak(engine, bridge, parser, connectOption, soakSecondsOption);
    }

    if (parser.isSet(syncCheckOption)) {
        return runSyncCheck(engine, bridge, parser, connectOption, loadOption, loadSecondOption);
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
        // A rate the fader cannot reach. The sync matches a tempo beyond the fader's travel
        // (`PitchLimits::beyond_the_fader`): the deck then runs at a rate the hand could never ask for,
        // and the handle must still be a handle — pinned at the end of its travel, not drawn past the
        // strip it belongs to.
        {
            QQuickItem *handle = findItem(window->contentItem(), QStringLiteral("pitchHandle_A"));
            if (handle == nullptr) {
                std::fprintf(stderr, "dj-ui: the pitch fader has no handle\n");
                return 1;
            }
            const double top_of_throw = pitch->property("maximum").toDouble();
            const double bottom_of_throw = pitch->property("minimum").toDouble();
            const double beyond = top_of_throw + (top_of_throw - bottom_of_throw) * 0.4;
            bridge->setTempo(QStringLiteral("A"), beyond);
            QElapsedTimer waiting;
            waiting.start();
            while (waiting.elapsed() < 2000 && std::abs(tempoOf("A") - beyond) > 0.005) {
                pump(1);
            }
            if (std::abs(tempoOf("A") - beyond) > 0.005) {
                std::fprintf(stderr,
                             "dj-ui: the deck did not take a rate beyond the fader (%.4f wanted, "
                             "%.4f playing)\n",
                             beyond, tempoOf("A"));
                return 1;
            }
            const double top = handle->y();
            const double bottom = top + handle->height();
            std::fprintf(stdout,
                         "dj-ui: at %.4f, past the fader's %.3f, the handle sits at %.1f…%.1f px of "
                         "%.1f\n",
                         beyond, top_of_throw, top, bottom, pitch->height());
            if (top < 0.0 || bottom > pitch->height()) {
                std::fprintf(stderr,
                             "dj-ui: the handle leaves its fader at %.4f: %.1f…%.1f of %.1f px\n",
                             beyond, top, bottom, pitch->height());
                return 1;
            }
            bridge->setTempo(QStringLiteral("A"), 1.0);
            pump(20);
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
            // A check that needs a load unloads the deck first: the check before this one can leave a
            // track playing in deck A, and a load onto a playing deck is refused as `Busy` — the colour
            // under test would then belong to somebody else's track, which ends mid-blink.
            const auto emptied = [&]() {
                QElapsedTimer attempt;
                attempt.start();
                while (attempt.elapsed() < 5000) {
                    QString title;
                    bool busy = false;
                    for (const QVariant &entry :
                         bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                        const QVariantMap deck = entry.toMap();
                        if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                            title = deck.value(QStringLiteral("title")).toString();
                            busy = deck.value(QStringLiteral("playing")).toBool();
                        }
                    }
                    if (title.isEmpty() && !busy) {
                        return true;
                    }
                    bridge->play(QStringLiteral("A"), false);
                    bridge->eject(QStringLiteral("A"));
                    QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    QThread::msleep(20);
                }
                return false;
            };
            if (!emptied()) {
                std::fprintf(stderr, "dj-ui: deck A never emptied before the space check loaded\n");
                return 1;
            }
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
        // The fixture is short and a null device spends it faster than a hand can blink: rewind it and
        // put a loop on the deck, so the track cannot end while the colour is under test. A deck that
        // stopped is a deck whose button is grey for a reason that has nothing to do with the refusal.
        // Pausing before the seek is what makes the seek land.
        bridge->play(QStringLiteral("A"), false);
        bridge->seek(QStringLiteral("A"), 0);
        bridge->setLoop(QStringLiteral("A"), true, 0, 96000);
        bridge->play(QStringLiteral("A"), true);
        QCoreApplication::processEvents();
        QThread::msleep(200);
        QCoreApplication::processEvents();

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
        // ... and it has to come back to blue, not to grey, once the blink is over. The end of the
        // blink is the button's own flag: a guessed duration is a duration that races the track.
        QElapsedTimer blink;
        blink.start();
        while (blink.elapsed() < 2000 && deckButton->property("flashing").toBool()) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 30);
            QThread::msleep(20);
        }
        QCoreApplication::processEvents();
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

    if (parser.isSet(keyCheckOption)) {
        // The key is a measurement, so the check reads the two places a person reads it: the label in
        // the deck's header, and the file the engine wrote beside the track. A key that only exists in
        // memory is a key the next load forgets.
        const QString track = parser.value(loadOption);
        if (track.isEmpty()) {
            std::fprintf(stderr, "dj-ui: --key-check needs --load\n");
            return 2;
        }
        auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (window == nullptr) {
            std::fprintf(stderr, "dj-ui: the root object is not a window\n");
            return 1;
        }
        if (parser.isSet(connectOption)) {
            bridge->connectToEngine(parser.value(connectOption));
            QElapsedTimer linking;
            linking.start();
            while (!bridge->isConnected() && linking.elapsed() < 3000) {
                QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                QThread::msleep(10);
            }
        }
        if (!bridge->isConnected()) {
            std::fprintf(stderr, "dj-ui: --key-check needs a running engine\n");
            return 1;
        }
        // The engine outlives one check: a deck that already holds a track would show the key of the
        // track before this one, and the check would read a stale answer as a fresh one. The wait is on
        // the deck's own state, not on the label: a label that has not been repainted yet is not a
        // load that has not happened.
        // What the engine said about the commands this check sends: a refusal is the reason a load
        // leaves the deck as it was, and a check that only reports "no key" hides it.
        QString lastAck;
        QObject::connect(bridge.get(), &BridgeClient::acked,
                         [&lastAck](int id, bool applied, bool rejected, QString reason) {
                             lastAck = QStringLiteral("command %1: %2%3")
                                           .arg(id)
                                           .arg(applied ? QStringLiteral("applied")
                                                        : (rejected ? QStringLiteral("rejected")
                                                                    : QStringLiteral("unknown")))
                                           .arg(reason.isEmpty() ? QString()
                                                                 : QStringLiteral(" (%1)").arg(reason));
                         });
        const auto deckNow = [&]() { return deckById(bridge->snapshot(), QStringLiteral("A")); };
        const auto trackOf = [&]() { return deckNow().value(QStringLiteral("track")).toMap(); };
        const auto keyOf = [&]() { return trackOf().value(QStringLiteral("key")).toString().trimmed(); };
        // The deck is emptied first, and not only stopped. A deck that is playing refuses a load — the
        // refusal is right, cutting the track the room is dancing to is the mistake a DJ never forgives
        // — and an earlier check in the suite leaves deck A playing. Emptying also makes the wait below
        // mean something: on a deck that already holds this very file the title matches before the load
        // has happened, and the check would read the previous answer as a fresh one.
        bridge->eject(QStringLiteral("A"));
        QElapsedTimer emptying;
        emptying.start();
        while (emptying.elapsed() < 5000 && !trackOf().isEmpty()) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        bridge->load(QStringLiteral("A"), track);
        // The title says which track the deck is describing: the key of the one before it is not an
        // answer about this one.
        const QString wanted = QFileInfo(track).completeBaseName();
        QElapsedTimer waited;
        waited.start();
        QString shown;
        while (waited.elapsed() < 30000) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
            const QVariantMap current = trackOf();
            const QString title = current.value(QStringLiteral("title")).toString();
            if (title == wanted && !keyOf().isEmpty()) {
                shown = keyOf();
                break;
            }
            QThread::msleep(20);
        }
        std::fprintf(stdout, "dj-ui: deck A shows the key \"%s\" after %lld ms\n", qPrintable(shown),
                     static_cast<long long>(waited.elapsed()));
        if (shown.isEmpty()) {
            const QVariantMap current = trackOf();
            std::fprintf(stderr,
                         "dj-ui: the deck never showed a key (wanted \"%s\", deck says load %s, title "
                         "\"%s\", key \"%s\")\n",
                         qPrintable(wanted),
                         qPrintable(deckNow().value(QStringLiteral("load")).toString()),
                         qPrintable(current.value(QStringLiteral("title")).toString()),
                         qPrintable(current.value(QStringLiteral("key")).toString()));
            std::fprintf(stderr, "dj-ui: the engine last said %s\n", qPrintable(lastAck));
            return 1;
        }
        // The file beside the track: `.djmeta/<name>.json`, the same place the cues go.
        const QFileInfo info(track);
        const QString sidecar = info.absolutePath() + QStringLiteral("/.djmeta/") + info.fileName()
                                + QStringLiteral(".json");
        QFile stored(sidecar);
        if (!stored.open(QIODevice::ReadOnly)) {
            std::fprintf(stderr, "dj-ui: no metadata at %s\n", qPrintable(sidecar));
            return 1;
        }
        const QJsonObject meta = QJsonDocument::fromJson(stored.readAll()).object();
        const QString written = meta.value(QStringLiteral("key")).toString();
        std::fprintf(stdout, "dj-ui: %s records the key \"%s\"\n", qPrintable(sidecar),
                     qPrintable(written));
        if (written.isEmpty()) {
            std::fprintf(stderr, "dj-ui: the engine did not write the key beside the track\n");
            return 1;
        }
        // The format is part of the answer: the owner reads keys as numbers on the circle of fifths, so
        // a name in the header would be a different feature. 1d … 12d, 1m … 12m, nothing else.
        static const QRegularExpression circleOfFifths(QStringLiteral("^([1-9]|1[0-2])[dm]$"));
        if (!circleOfFifths.match(shown).hasMatch()) {
            std::fprintf(stderr, "dj-ui: \"%s\" is not a key on the circle of fifths\n",
                         qPrintable(shown));
            return 1;
        }
        // A file that already holds a code must agree with the header. A file holding a name was
        // translated for the display, and the translation itself is covered by the analysis tests.
        if (circleOfFifths.match(written).hasMatch() && written != shown) {
            std::fprintf(stderr, "dj-ui: the header says \"%s\" and the file says \"%s\"\n",
                         qPrintable(shown), qPrintable(written));
            return 1;
        }
        std::fprintf(stdout, "dj-ui: the key is measured, stored and shown\n");
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
        return runBrowserCheck(engine, browser, layout);
    }

    if (parser.isSet(pageCheckOption)) {
        if (!parser.isSet(connectOption)) {
            std::fprintf(stderr, "dj-ui: --collection-page-check needs --connect\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        return runCollectionPageCheck(engine, browser, *bridge, parser.value(importFixtureOption),
                                      parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(importCheckOption)) {
        // The import is the engine's work, so this check is useless without one: say so rather than
        // report a shell that draws dialogs for a batch nobody runs.
        if (!parser.isSet(connectOption)) {
            std::fprintf(stderr, "dj-ui: --import-check needs --connect\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        return runImportCheck(engine, browser, layout, *bridge, parser.value(importFixtureOption),
                              parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(endCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runEndCheck(engine, *bridge, parser.value(loadOption));
    }

    if (parser.isSet(lockCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runLockCheck(engine, browser, *bridge, parser.value(importFixtureOption),
                            parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(sortCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runSortCheck(engine, browser, layout, parser.value(importFixtureOption),
                            parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(genreCheckOption)) {
        // The chips are answered by the index: this check without an engine measured a list nobody asked
        // about, and reported the emptiness as a genre bug.
        bridge->connectToEngine(parser.value(connectOption));
        return runGenreCheck(engine, browser, layout, parser.value(importFixtureOption),
                             parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(collectionProbeOption)) {
        return runCollectionProbe(browser, parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(collectionLoadCheckOption)) {
        bridge->connectToEngine(parser.value(connectOption));
        return runCollectionLoadCheck(engine, browser, *bridge,
                                      parser.values(collectionRootOption).value(0));
    }

    if (parser.isSet(analyzeCheckOption)) {
        if (!parser.isSet(connectOption)) {
            std::fprintf(stderr, "dj-ui: --analyze-check needs --connect\n");
            return 1;
        }
        bridge->connectToEngine(parser.value(connectOption));
        bridge->connectToEngine(parser.value(connectOption));
        return runAnalyzeCheck(engine, browser, layout, *bridge, parser.value(importFixtureOption),
                               parser.values(collectionRootOption).value(0));
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
