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
#include "Waveform.h"

#include <QCommandLineOption>
#include <QCommandLineParser>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QGuiApplication>
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
int runSelfCheck(BridgeClient &bridge, Waveform &waveform, const QString &socketPath,
                 int timeoutMs, const QString &loadPath)
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
    enum Stage { SendingLoad, SendingPlay, SendingSetCue, SendingJumpCue, Done };
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
            // A load is acknowledged as accepted; the deck is playable when the state says Ready.
            const QVariantMap deckState = deckById(snapshot, QStringLiteral("A"));
            const QString load = deckState.value(QStringLiteral("load")).toString();
            if (!load.isEmpty() && load != QStringLiteral("Ready")) {
                return;
            }
            if (load.isEmpty()) {
                // The deck is still `Empty` — wait for the loader, it is off the audio path.
                if (!deckState.contains(QStringLiteral("load"))) {
                    return;
                }
            }
            if (loadTitle.isEmpty()) {
                // Already requested on connect; wait for the loader to publish `Ready`.
                loadTitle = QStringLiteral("requested");
                return;
            }
            const QVariantMap track = deckState.value(QStringLiteral("track")).toMap();
            std::fprintf(stdout, "dj-ui: deck A is ready: %s\n",
                         qPrintable(track.value(QStringLiteral("title")).toString()));
            // The engine publishes a reference to the envelope; the UI must be able to read it, or
            // the deck has no picture however good the snapshot is.
            const QVariantMap reference = deckState.value(QStringLiteral("waveform")).toMap();
            if (reference.isEmpty()) {
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
            std::fprintf(stdout,
                         "dj-ui: waveform loaded: %d columns of %d frames at %d Hz, covering %.0f "
                         "of %.0f frames\n",
                         waveform.buckets(), waveform.framesPerBucket(), waveform.sampleRate(),
                         covered, duration);
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
    const QCommandLineOption selfCheckOption(
        QStringLiteral("self-check"),
        QStringLiteral("Connect, press Play, require the ack and the snapshot, then exit."));
    const QCommandLineOption dialogCheckOption(
        QStringLiteral("dialog-check"),
        QStringLiteral("Check the in-app file browser without a display: the QML module loads and "
                       "lists a directory."));
    const QCommandLineOption loadOption(
        QStringLiteral("load"),
        QStringLiteral("With --self-check: load this file into deck A first."),
        QStringLiteral("path"));
    parser.addOption(smokeOption);
    parser.addOption(connectOption);
    parser.addOption(selfCheckOption);
    parser.addOption(loadOption);
    parser.addOption(dialogCheckOption);
    parser.process(app);

    auto bridge = std::make_unique<BridgeClient>();
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "Bridge", bridge.get());
    FileBrowser browser;
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "Browser", &browser);
    Waveform waveform;
    qmlRegisterSingletonInstance("DjPlatform.Ui", 1, 0, "Waveform", &waveform);

    if (parser.isSet(selfCheckOption)) {
        const QString path = parser.isSet(connectOption) ? parser.value(connectOption)
                                                         : defaultSocketPath();
        return runSelfCheck(*bridge, waveform, path, 15000,
                            parser.isSet(loadOption) ? parser.value(loadOption) : QString());
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
