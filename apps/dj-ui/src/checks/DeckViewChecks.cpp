// Проверки: DeckViewChecks.
//
// Каждая функция — один прогон оболочки: окно, нажатия, требования к движку.

#include "Checks.h"

namespace checks {

int runSyncCheck(QQmlApplicationEngine &engine,
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

int runSyncSoak(QQmlApplicationEngine &engine,
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

int runLoop(QQmlApplicationEngine &engine,
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

int runEqCheck(QQmlApplicationEngine &engine,
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

int runGridLines(QQmlApplicationEngine &engine,
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

// The rows under the wave, measured by the check that loads a track with a grid. The wave is the only
// child of the deck with fillHeight, so every row takes its height from the wave: folding a row has to
// give that height back, and the two groups of the folding strip have to sit on the panel's edges.
// The window is given a size first: in a headless run it is 0x0, and every measurement is then about
// an arrangement nobody has.

int runLayoutCheck(QQmlApplicationEngine &engine,
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

    // The two global modes stand in the master column, centred under its knob: they belong to the whole
    // mix, and the master is where the whole mix is decided (owner's decision, 14.11).
    {
        auto *masterColumn = findItem(content, QStringLiteral("masterColumn"));
        auto *masterKnobItem = findItem(content, QStringLiteral("masterKnob"));
        auto *snapGlobal = findItem(content, QStringLiteral("snapGlobal"));
        auto *quantizeGlobal = findItem(content, QStringLiteral("quantizeGlobal"));
        if (masterColumn == nullptr || masterKnobItem == nullptr || snapGlobal == nullptr
            || quantizeGlobal == nullptr) {
            std::fprintf(stderr, "dj-ui: the mixer has no master column or no global switches\n");
            return 1;
        }
        const double knobAxis = masterKnobItem->mapToItem(content, QPointF(masterKnobItem->width() / 2.0, 0.0)).x();
        const double switchAxis = snapGlobal->mapToItem(content, QPointF(snapGlobal->width() / 2.0, 0.0)).x();
        const double otherAxis = quantizeGlobal->mapToItem(content, QPointF(quantizeGlobal->width() / 2.0, 0.0)).x();
        const double knobBottom = masterKnobItem->mapToItem(content, QPointF(0.0, masterKnobItem->height())).y();
        const double switchTop = snapGlobal->mapToItem(content, QPointF(0.0, 0.0)).y();
        const double switchBottom = quantizeGlobal->mapToItem(content, QPointF(0.0, quantizeGlobal->height())).y();
        std::fprintf(stdout,
                     "dj-ui: the global switches stand %.1f px off the master's axis, %.1f px under its "
                     "knob, %.1f px tall together, and end %.1f px above the column's bottom\n",
                     std::abs(switchAxis - knobAxis), switchTop - knobBottom, switchBottom - switchTop,
                     masterColumn->mapToItem(content, QPointF(0.0, masterColumn->height())).y() - switchBottom);
        if (std::abs(switchAxis - knobAxis) > 2.0 || std::abs(otherAxis - knobAxis) > 2.0) {
            std::fprintf(stderr, "dj-ui: the global switches are not centred under the master's knob\n");
            return 1;
        }
        if (switchTop < knobBottom) {
            std::fprintf(stderr, "dj-ui: the global switches are not below the master's knob\n");
            return 1;
        }
        if (switchBottom > masterColumn->mapToItem(content, QPointF(0.0, masterColumn->height())).y()) {
            std::fprintf(stderr, "dj-ui: the global switches hang past the master column\n");
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

} // namespace checks
