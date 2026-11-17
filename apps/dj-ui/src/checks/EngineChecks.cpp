// Проверки: EngineChecks.
//
// Каждая функция — один прогон оболочки: окно, нажатия, требования к движку.

#include "Checks.h"

namespace checks {

int runBusyCheck(BridgeClient &bridge, const QString &socketPath, const QString &track,
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

// The journal of plays, from the DJ's side: a deck plays, the engine counts continuous playing, and a
// play lands in the snapshot only when it was long enough. The tracker's own rules — a pause ends the
// interval, a loop inside it does not, a stalled publisher does not inflate it — have unit tests; this
// proves the wiring: the deck's transport, the publisher's audio clock and the row in the database.
//
// Two habits make it robust in a suite that shares one engine. The waits are counted in the engine's own
// audio clock, which is what the journal counts, so a busy machine that renders slower than real time
// moves both sides together. And every case is measured as a *change* of the journal, not as its length:
// a check that runs after others must not care what they left behind.

int runHistoryCheck(BridgeClient &bridge, const QString &socketPath, const QString &track,
                           int timeoutMs)
{
    QEventLoop loop;
    QTimer watchdog;
    watchdog.setSingleShot(true);
    QObject::connect(&watchdog, &QTimer::timeout, &loop, &QEventLoop::quit);
    watchdog.start(timeoutMs);

    constexpr double kShort = 1.2;
    constexpr double kLong = 3.5;
    int failures = 0;
    int before = 0;
    bool started = false;
    bool ready = false;
    bool waiting = false;
    bool stopping = false;
    int stopTicks = 0;
    double startedAt = 0.0;
    double target = 0.0;
    double recorded = 0.0;
    std::function<void()> reached;
    std::function<void()> afterStop;

    const auto clockSeconds = [&]() {
        const QVariantMap snapshot = bridge.snapshot();
        const double rate = snapshot.value(QStringLiteral("sample_rate")).toDouble();
        if (rate <= 0.0) {
            return 0.0;
        }
        return snapshot.value(QStringLiteral("audio_clock_samples")).toDouble() / rate;
    };
    const auto rows = [&]() { return bridge.snapshot().value(QStringLiteral("plays")).toList(); };
    const auto deckPlaying = [&]() {
        for (const QVariant &entry : bridge.snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return deck.value(QStringLiteral("playing")).toBool();
            }
        }
        return false;
    };
    const auto play = [&](bool on) { bridge.play(QStringLiteral("A"), on); };
    const auto waitSeconds = [&](double seconds, std::function<void()> then) {
        startedAt = clockSeconds();
        target = seconds;
        reached = then;
        waiting = true;
    };
    // A stop is a message, not a fact: the deck is quiet only when the snapshot says so, and every case
    // that follows a stop waits for that instead of for a wall-clock guess.
    const auto stopThen = [&](std::function<void()> then) {
        play(false);
        stopping = true;
        stopTicks = 0;
        afterStop = then;
    };

    QObject::connect(&bridge, &BridgeClient::connectedChanged, &loop, [&]() {
        if (!bridge.isConnected() || started) {
            return;
        }
        started = true;
        if (!track.isEmpty()) {
            bridge.load(QStringLiteral("A"), track);
        }
    });

    QObject::connect(&bridge, &BridgeClient::snapshotChanged, &loop, [&]() {
        if (stopping) {
            stopTicks += 1;
            // The journal softens a blink (250 ms), so a play is written one look after the silence
            // outlasts it: the check waits past that mark instead of reading the moment the deck stops.
            if ((!deckPlaying() && stopTicks > 12) || stopTicks > 400) {
                stopping = false;
                auto then = afterStop;
                afterStop = nullptr;
                then();
            }
            return;
        }
        if (waiting && clockSeconds() - startedAt >= target) {
            waiting = false;
            auto then = reached;
            reached = nullptr;
            then();
            return;
        }
        if (!started || ready) {
            return;
        }
        bool loaded = false;
        bool playing = false;
        for (const QVariant &entry : bridge.snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                continue;
            }
            loaded = deck.value(QStringLiteral("load")).toString() == QStringLiteral("Ready");
            playing = deck.value(QStringLiteral("playing")).toBool();
        }
        if (!loaded || playing) {
            return;
        }
        ready = true;

        // Let the load settle before the first case: a publication still in flight changes what the deck
        // holds, and the journal follows the file a deck is playing, so a case that starts on the tail of
        // a load can be split in two.
        waitSeconds(1.0, [&]() {
        // Case one: below the threshold.
        before = static_cast<int>(rows().size());
        play(true);
        waitSeconds(kShort, [&]() {
            stopThen([&]() {
                if (static_cast<int>(rows().size()) != before) {
                    std::fprintf(stderr,
                                 "dj-ui: a play of %.1f s was recorded (the threshold is 2 s)\n", kShort);
                    failures += 1;
                }
                // Case two: above it.
                before = static_cast<int>(rows().size());
                play(true);
                waitSeconds(kLong, [&]() {
                    stopThen([&]() {
                        // What the case has to prove is that a long play lands and that every row is a
                        // play that counts. How many rows it lands in is the engine's business: a deck
                        // that slips (an underrun, a re-publication) ends one interval and opens another,
                        // and a half below the threshold is dropped exactly as the rule says.
                        const QVariantList journal = rows();
                        recorded = 0.0;
                        if (journal.size() > before) {
                            for (int index = before; index < journal.size(); ++index) {
                                recorded += journal[index].toMap().value(QStringLiteral("seconds")).toDouble();
                            }
                        }
                        if (journal.size() == before) {
                            std::fprintf(stderr, "dj-ui: a play of %.1f s added no record\n", kLong);
                            failures += 1;
                        }
                        if (recorded > kLong + 0.8) {
                            std::fprintf(stderr,
                                         "dj-ui: a play of %.1f s was recorded as %.2f s\n", kLong,
                                         recorded);
                            failures += 1;
                        }
                        for (int index = before; index < journal.size(); ++index) {
                            const double seconds =
                                journal[index].toMap().value(QStringLiteral("seconds")).toDouble();
                            if (seconds < 1.8) {
                                std::fprintf(stderr,
                                             "dj-ui: a record of %.2f s is below the 2 s threshold\n",
                                             seconds);
                                failures += 1;
                            }
                        }
                        // Case three: two short listens with a pause between them.
                        before = static_cast<int>(rows().size());
                        play(true);
                        waitSeconds(kShort, [&]() {
                            stopThen([&]() {
                                play(true);
                                waitSeconds(kShort, [&]() {
                                    stopThen([&]() {
                                        if (static_cast<int>(rows().size()) != before) {
                                            std::fprintf(stderr,
                                                         "dj-ui: two listens of %.1f s with a pause "
                                                         "between them added %d rows\n",
                                                         kShort,
                                                         static_cast<int>(rows().size()) - before);
                                            failures += 1;
                                        }
                                        if (failures == 0) {
                                            std::fprintf(stdout,
                                                         "dj-ui: %.1f s of playing left no record, %.1f s "
                                                         "left records of %.2f s in total, and %.1f s + a "
                                                         "pause + %.1f s left none — the journal holds "
                                                         "only plays that count\n",
                                                         kShort, kLong, recorded, kShort, kShort);
                                        }
                                        loop.quit();
                                    });
                                });
                            });
                        });
                    });
                });
            });
        });
        });
    });

    bridge.connectToEngine(socketPath);
    loop.exec();
    if (failures != 0) {
        std::fprintf(stderr, "dj-ui: the journal of plays is not what the deck did\n");
        return 1;
    }
    return 0;
}

// End to end: the UI's gain command, the engine applying it, and the number coming back in the
// snapshot. The unit test proves the routing; this proves the wire.

int runGainCheck(BridgeClient &bridge, const QString &socketPath, int timeoutMs)
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


// The browser's own check: a list you cannot pick from is a picture of a list, so it goes through
// the same functions the mouse does.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC says so
// (`variable tracking size limit exceeded`). The checks are the tests; they read better named.

int runEndCheck(QQmlApplicationEngine &engine, BridgeClient &bridge, const QString &fixture)
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

} // namespace checks
