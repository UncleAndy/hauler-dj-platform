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
#include "UiSettings.h"
#include "LayoutSettings.h"
#include "SessionState.h"
#include "Waveform.h"
#include "WaveformItem.h"
#include "WaveformPainter.h"

#include <QCommandLineOption>
#include <QCommandLineParser>
#include <QElapsedTimer>
#include <QJsonDocument>
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
#include <QTimer>

#include <cstdio>
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

    bool sent = false;
    bool done = false;
    bool seen = false;
    double gainC = 0.0;
    double gainB = 0.0;

    QObject::connect(&bridge, &BridgeClient::snapshotChanged, &loop, [&]() {
        if (!sent) {
            sent = true;
            bridge.setChannelGain(QStringLiteral("C"), 0.25);
            std::fprintf(stdout, "dj-ui: sent gain 0.25 to deck C\n");
            return;
        }
        const QVariantList decks =
            bridge.snapshot().value(QStringLiteral("decks")).toList();
        if (!seen) {
            seen = true;
            QStringList names;
            for (const QVariant &entry : decks) {
                names.append(entry.toMap().value(QStringLiteral("deck")).toString());
            }
            std::fprintf(stdout, "dj-ui: snapshot with %d decks: %s\n", int(decks.size()),
                         qPrintable(names.join(QStringLiteral(", "))));
        }
        for (const QVariant &entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("C")) {
                gainC = deck.value(QStringLiteral("linear_gain")).toDouble();
            } else if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("B")) {
                gainB = deck.value(QStringLiteral("linear_gain")).toDouble();
            }
        }
        if (std::abs(gainC - 0.25) < 0.01) {
            done = true;
            loop.quit();
        }
    });

    QObject::connect(&bridge, &BridgeClient::connectedChanged, &loop, [&]() {
        if (!bridge.isConnected()) {
            return;
        }
        std::fprintf(stdout, "dj-ui: connected to %s for the gain check\n", qPrintable(socketPath));
        bridge.setChannelGain(QStringLiteral("C"), 0.25);
        sent = true;
    });

    bridge.connectToEngine(socketPath);
    loop.exec();

    if (!done) {
        std::fprintf(stderr, "dj-ui: deck C did not reach gain 0.25 (it is %.3f)\n", gainC);
        return 1;
    }
    if (std::abs(gainB - 1.0) > 0.01) {
        std::fprintf(stderr, "dj-ui: deck B moved too (%.3f); the command must touch only C\n", gainB);
        return 1;
    }
    std::fprintf(stdout, "dj-ui: gain 0.25 landed on deck C, deck B still at %.2f\n", gainB);
    bridge.setChannelGain(QStringLiteral("C"), 1.0);
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
            if (state.isEmpty() || state == QStringLiteral("Loading")) {
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
    parser.addOption(busyCheckOption);
    parser.addOption(refusalCheckOption);
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
    parser.addOption(browserStartOption);
    parser.addOption(themeCheckOption);
    parser.addOption(benchOption);
    parser.addOption(frameRateOption);
    parser.addOption(zoomOption);
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
    if (parser.isSet(zoomOption)) {
        bool ok = false;
        const double factor = parser.value(zoomOption).toDouble(&ok);
        if (!ok || !settings.setWaveformZoom(factor)) {
            std::fprintf(stderr, "dj-ui: --waveform-zoom needs 1..100\n");
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
            paintWaveform(painter, envelope, style, image.rect(), start, 0.1, start + 0.05, {}, false,
                          0.0, 0.0);
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

        // A knob must be as tall as the meter beside it, or "full height" is only a phrase.
        auto *knob = findItem(content, QStringLiteral("knob_A"));
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
        // Knobs mirror: the left-hand decks keep theirs on the left of the meter, the right-hand ones
        // on the right.
        for (const char *id : {"A", "B", "C", "D"}) {
            auto *stripKnob = findItem(content, QStringLiteral("knob_%1").arg(QString::fromLatin1(id)));
            auto *stripMeter = findItem(content, QStringLiteral("meter_%1").arg(QString::fromLatin1(id)));
            if (stripKnob == nullptr || stripMeter == nullptr) {
                std::fprintf(stderr, "dj-ui: deck %s has no knob or meter\n", id);
                return 1;
            }
            const double knobX = stripKnob->mapToScene(QPointF(0, 0)).x();
            const double meterX = stripMeter->mapToScene(QPointF(0, 0)).x();
            const bool left = QString::fromLatin1(id) == QStringLiteral("A")
                || QString::fromLatin1(id) == QStringLiteral("C");
            std::fprintf(stdout, "dj-ui: deck %s knob at %.0f, meter at %.0f, %s\n", id, knobX, meterX,
                         (knobX < meterX) == left ? "as placed" : "WRONG SIDE");
            if ((knobX < meterX) != left) {
                std::fprintf(stderr, "dj-ui: deck %s has its knob on the wrong side\n", id);
                return 1;
            }
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
            const double lastGain = knob->property("lastGain").toDouble();
            std::fprintf(stdout, "dj-ui: knob A press at %.0f px -> y %.1f, gain %.3f\n",
                         point.y(), lastY, lastGain);
            if (lastY < 0.0) {
                std::fprintf(stderr, "dj-ui: the knob did not receive the mouse\n");
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: layout %s applied as set\n", qPrintable(layout.preset()));
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
        auto *deckLoad = findItem(window->contentItem(), QStringLiteral("deckLoad_A"));
        if (deckButton == nullptr || deckLoad == nullptr) {
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
        QMetaObject::invokeMethod(deckLoad, "clicked");
        QCoreApplication::processEvents();
        const QColor whileFlashing = faceOf(deckButton);
        const bool flashingNow = deckButton->property("flashing").toBool()
            || deckLoad->property("flashing").toBool();
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
        const int loadRefusals = deckLoad->property("refusals").toInt();
        const bool flashing = flashingNow;
        std::fprintf(stdout, "dj-ui: refused load: browser button %d, deck button %d, flashing %s\n",
                     buttonRefusals, loadRefusals, flashing ? "yes" : "no");
        if (!refused.toBool() && buttonRefusals > 0 && loadRefusals > 0 && flashing) {
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
        std::fprintf(stdout, "dj-ui: browser selects and acts, selection follows the click\n");
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

    return app.exec();
}
