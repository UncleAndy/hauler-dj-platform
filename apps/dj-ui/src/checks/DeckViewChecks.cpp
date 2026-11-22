// Проверки: DeckViewChecks.
//
// Каждая функция — один прогон оболочки: окно, нажатия, требования к движку.

#include "Checks.h"

#include <QRegularExpression>

namespace checks {

int runSyncCheck(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
                 QCommandLineParser& parser, const QCommandLineOption& connectOption,
                 const QCommandLineOption& loadOption, const QCommandLineOption& loadSecondOption) {
    // The engine's own measure of sync is in S2 and lives in cargo tests; what is checked here is
    // that a hand can reach it and read it: two buttons, a snapshot and a needle
    // (DJP-SYNC-001/002).
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
    const auto deckOf = [&](const QString& id) {
        const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == id) {
                return deck;
            }
        }
        return QVariantMap();
    };
    // A mode of `off` is left off the wire, so an absent field is the deck following nothing — not
    // a missing answer. Reading it as an empty string would make "stop following" untestable.
    const auto modeOf = [&](const QString& id) {
        const QString mode = deckOf(id).value(QStringLiteral("sync")).toString();
        return mode.isEmpty() ? QStringLiteral("off") : mode;
    };
    const auto press = [&](const QString& name) {
        auto* item = findItem(window->contentItem(), name);
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
            return deckOf(QStringLiteral("A"))
                       .value(QStringLiteral("path"))
                       .toString()
                       .endsWith(first_name) &&
                   deckOf(QStringLiteral("B"))
                       .value(QStringLiteral("path"))
                       .toString()
                       .endsWith(second_name) &&
                   deckOf(QStringLiteral("A")).value(QStringLiteral("beat")).isValid();
        },
        20000);
    if (!loaded) {
        std::fprintf(stderr, "dj-ui: the two decks did not both come up with a beat\n");
        return 1;
    }

    auto* stripA = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
    auto* stripB = findItem(window->contentItem(), QStringLiteral("deckDetail_B"));
    if (stripA == nullptr || stripB == nullptr) {
        std::fprintf(stderr, "dj-ui: a deck has no zoomed strip to measure\n");
        return 1;
    }
    // A known window, smaller than the fixture: four beats are two seconds at 120 BPM, and a window that fits
    // inside the track is the only one that can show its tempo.
    QMetaObject::invokeMethod(stripA, "zoomToBeats", Q_ARG(QVariant, QVariant(4.0)));
    QMetaObject::invokeMethod(stripB, "zoomToBeats", Q_ARG(QVariant, QVariant(4.0)));
    QCoreApplication::processEvents();

    if (!press(QStringLiteral("syncMaster_A")) || !press(QStringLiteral("syncBeat_B"))) {
        return 1;
    }
    const bool locked = waitFor(
        [&]() {
            return deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool() &&
                   modeOf(QStringLiteral("B")) == QStringLiteral("beat");
        },
        5000);
    if (!locked) {
        std::fprintf(stderr,
                     "dj-ui: the master and sync buttons did not reach the engine (A master %d, "
                     "B sync %s)\n",
                     deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool(),
                     qPrintable(modeOf(QStringLiteral("B"))));
        return 1;
    }
    // The window is counted in beats, and a beat is `60 / BPM` seconds: two decks that follow one tempo show
    // one picture, whatever their lengths. This is the whole point of counting in beats — a window that is a
    // fraction of the track shows a different span of music on every track, and two synced decks then scroll
    // at two speeds with their bars at two widths (owner's decision, 2026-11-20).
    const auto heardOf = [&](const QString& id) {
        const QVariantMap state = deckOf(id);
        return state.value(QStringLiteral("local_bpm")).toDouble() *
               state.value(QStringLiteral("tempo_ratio")).toDouble();
    };
    const auto windowOf = [](QQuickItem* strip) {
        const double span = strip->property("zoomSpan").toDouble();
        const double frames = strip->property("totalFrames").toDouble();
        const double rate = strip->property("sourceRate").toDouble();
        const double ratio = strip->property("tempoRatio").toDouble();
        if (!(span > 0.0) || !(frames > 0.0) || !(rate > 0.0) || !(ratio > 0.0)) {
            return 0.0;
        }
        return span * frames / (rate * ratio);
    };
    // The window can never be longer than the track: beats at a slow tempo ask for more music than a short
    // file holds, and the strip then shows the file.
    const auto trackSeconds = [](QQuickItem* strip) {
        const double frames = strip->property("totalFrames").toDouble();
        const double rate = strip->property("sourceRate").toDouble();
        return rate > 0.0 ? frames / rate : 0.0;
    };
    const auto wantedWindow = [&](QQuickItem* strip, double bpm) {
        return std::min(strip->property("beatsOnScreen").toDouble() * 60.0 / bpm, trackSeconds(strip));
    };
    const bool settled = waitFor(
        [&]() {
            return std::abs(heardOf(QStringLiteral("A")) - heardOf(QStringLiteral("B"))) < 0.05 &&
                   windowOf(stripA) > 0.0 && windowOf(stripB) > 0.0;
        },
        15000);
    const double secondsA = windowOf(stripA);
    const double secondsB = windowOf(stripB);
    const double beats = stripA->property("beatsOnScreen").toDouble();
    const double heard = heardOf(QStringLiteral("A"));
    std::fprintf(stdout,
                 "dj-ui: the strips show %.3f and %.3f s of music, %.0f beats at %.2f BPM (%.3f s wanted)\n",
                 secondsA, secondsB, beats, heard, heard > 0.0 ? beats * 60.0 / heard : 0.0);
    if (!settled) {
        std::fprintf(stderr,
                     "dj-ui: the strips never settled (%.3f and %.3f s, heard %.2f and %.2f BPM)\n", secondsA,
                     secondsB, heardOf(QStringLiteral("A")), heardOf(QStringLiteral("B")));
        return 1;
    }
    if (!(beats > 0.0) || !(heard > 0.0)) {
        std::fprintf(stderr, "dj-ui: no beats or no tempo to measure the window against (%.1f, %.2f)\n",
                     beats, heard);
        return 1;
    }
    // A beat and a half of slack: the window is built from the tempo the engine publishes, and a deck still
    // trimming its tempo is a deck whose window is a shade off.
    const double wanted = wantedWindow(stripA, heard);
    if (std::abs(secondsA - wanted) > wanted * 0.02) {
        std::fprintf(stderr,
                     "dj-ui: the master's strip shows %.3f s where %0.f beats at %.2f BPM are %.3f s\n",
                     secondsA, beats, heard, wanted);
        return 1;
    }
    if (std::abs(secondsA - secondsB) > wanted * 0.02) {
        std::fprintf(stderr,
                     "dj-ui: the synced strips show %.3f and %.3f s of music (%.1f %% apart)\n", secondsA,
                     secondsB, std::abs(secondsA - secondsB) / secondsA * 100.0);
        return 1;
    }

    // And the window is counted in beats, not in fractions of the track. A fraction would not move when the
    // tempo moves — a tenth of a track is a tenth of a track at any speed. A beat is `60 / BPM` seconds, so
    // the window has to shrink when the master speeds up, and the follower's has to shrink with it.
    const double before = windowOf(stripB);
    bridge->setTempo(QStringLiteral("A"), 1.05);
    const bool followed = waitFor(
        [&]() {
            return std::abs(heardOf(QStringLiteral("A")) - heardOf(QStringLiteral("B"))) < 0.05 &&
                   windowOf(stripB) > 0.0 && std::abs(windowOf(stripB) - before) > before * 0.01;
        },
        10000);
    const double afterB = windowOf(stripB);
    const double heardAfter = heardOf(QStringLiteral("B"));
    const double wantedAfter = wantedWindow(stripB, heardAfter);

    // And the tempo is taken in one step, not walked to. A master that moves is a follower that has to be at the
    // new speed before the ear notices the old one: the deck's own glide is thirty milliseconds, so a quarter of
    // a second is generous. A trim of four percent a second would need a second and a quarter for this jump, and
    // while it walks, the phase walks too and the beat corrector fires seek after seek (owner's report,
    // 2026-11-20).
    const double heardBefore = heardOf(QStringLiteral("A"));
    bridge->setTempo(QStringLiteral("A"), 1.09);
    // The command is not the sound: the master has its own glide, and measuring the follower before the master
    // has moved would compare a new setpoint with an old tempo and call it a step.
    const bool masterMoved = waitFor(
        [&]() { return heardOf(QStringLiteral("A")) > heardBefore * 1.02; }, 3000);
    QElapsedTimer reach;
    reach.start();
    bool arrived = false;
    double ratioB = 0.0;
    double wantedRatio = 0.0;
    while (reach.elapsed() < 250) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 5);
        ratioB = deckOf(QStringLiteral("B")).value(QStringLiteral("tempo_ratio")).toDouble();
        const double localB = deckOf(QStringLiteral("B")).value(QStringLiteral("local_bpm")).toDouble();
        wantedRatio = localB > 0.0 ? heardOf(QStringLiteral("A")) / localB : 0.0;
        if (wantedRatio > 0.0 && std::abs(ratioB - wantedRatio) <= wantedRatio * 0.001) {
            arrived = true;
            break;
        }
        QThread::msleep(2);
    }
    std::fprintf(stdout, "dj-ui: the follower reached ratio %.6f of %.6f in %lld ms\n", ratioB,
                 wantedRatio, static_cast<long long>(reach.elapsed()));
    if (!masterMoved) {
        std::fprintf(stderr, "dj-ui: the master's tempo never moved, so nothing was measured\n");
        return 1;
    }
    if (!arrived) {
        std::fprintf(stderr,
                     "dj-ui: the follower walked to the tempo instead of taking it (ratio %.6f of %.6f "
                     "after %lld ms)\n",
                     ratioB, wantedRatio, static_cast<long long>(reach.elapsed()));
        return 1;
    }
    bridge->setTempo(QStringLiteral("A"), 1.05);
    std::fprintf(stdout, "dj-ui: at %.2f BPM the follower's strip shows %.3f s (was %.3f s, %.3f wanted)\n",
                 heardAfter, afterB, before, wantedAfter);
    bridge->setTempo(QStringLiteral("A"), 1.0);
    if (!followed) {
        std::fprintf(stderr,
                     "dj-ui: the window did not follow the tempo (%.3f s before, %.3f s after, wanted %.3f)\n",
                     before, afterB, wantedAfter);
        return 1;
    }
    if (std::abs(afterB - wantedAfter) > wantedAfter * 0.02) {
        std::fprintf(stderr, "dj-ui: the follower's window shows %.3f s where %.0f beats are %.3f s\n", afterB,
                     beats, wantedAfter);
        return 1;
    }

    // A deck that stands still publishes no phase: the distance from a beat that is not moving is a
    // clock, not a phase, and the meter would sweep. So the meter's presence is checked here and
    // the number itself is checked once the deck plays.
    auto* meter = findItem(window->contentItem(), QStringLiteral("phaseMeter_B"));
    auto* needle = findItem(window->contentItem(), QStringLiteral("phaseNeedle_B"));
    // The strip takes a frame or two to re-bind after the mode reaches the deck, and the meter is
    // only drawn while a mode is on: wait for it rather than assert on the first look.
    const bool meter_shown = waitFor(
        [&]() { return meter != nullptr && needle != nullptr && meter->isVisible(); }, 5000);
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
        std::fprintf(stderr, "dj-ui: the phase meter was never laid out (%.0f px)\n",
                     meter->width());
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
    // is useless in a mix. The meter is a peak that decays between the hits of a drum loop, so it
    // has to be watched over a window: one reading in a gap says nothing about a deck.
    const auto peakOf = [&](const QString& id) {
        return deckOf(id)
            .value(QStringLiteral("levels"))
            .toMap()
            .value(QStringLiteral("peak_left"))
            .toDouble();
    };
    const auto loudestOver = [&](const QString& id, int budget_ms) {
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
    const auto barErrorOf = [&](const QString& id) {
        const QVariant bar = deckOf(id).value(QStringLiteral("bar_error_beats"));
        return bar.isValid() ? bar.toDouble() : 0.0;
    };
    const auto frameOf = [&](const QString& id) {
        return deckOf(id).value(QStringLiteral("source_frame")).toDouble();
    };

    // The follower first, alone: a deck that cannot sound on its own makes the rest of this stage
    // measure nothing. The master has not played yet, so this is the deck and nothing else.
    bridge->play(QStringLiteral("B"), true);
    const double bare = loudestOver(QStringLiteral("B"), 2500);
    if (bare <= 0.01) {
        std::fprintf(stderr, "dj-ui: the follower makes no sound on its own (deck peak %.4f)\n",
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
            const QVariant phase =
                deckOf(QStringLiteral("B")).value(QStringLiteral("phase_error_ms"));
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
        [&]() { return !deckOf(QStringLiteral("B")).value(QStringLiteral("playing")).toBool(); },
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
        std::fprintf(
            stderr, "dj-ui: the stopped follower was dragged %.0f frames while the master played\n",
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
            const QVariant phase =
                deckOf(QStringLiteral("B")).value(QStringLiteral("phase_error_ms"));
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
    const bool tempo_for_the_arm =
        waitFor([&]() { return modeOf(QStringLiteral("B")) == QStringLiteral("tempo"); }, 5000);
    if (!tempo_for_the_arm) {
        std::fprintf(stderr, "dj-ui: the tempo button did not take\n");
        return 1;
    }
    const QVariantMap follower = deckOf(QStringLiteral("B"));
    const double rate = follower.value(QStringLiteral("source_rate")).toDouble();
    const double beat_ms = 60000.0 / follower.value(QStringLiteral("local_bpm")).toDouble();
    const double here = frameOf(QStringLiteral("B"));
    bridge->seek(QStringLiteral("B"), static_cast<qulonglong>(here + rate * beat_ms / 1000.0));
    const bool bar_moved =
        waitFor([&]() { return std::abs(barErrorOf(QStringLiteral("B"))) > 0.8; }, 5000);
    if (!bar_moved) {
        std::fprintf(stderr, "dj-ui: a beat along the bar reads %.3f beats\n",
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
    const bool released =
        waitFor([&]() { return modeOf(QStringLiteral("B")) == QStringLiteral("off"); }, 5000);
    if (!released) {
        std::fprintf(stderr, "dj-ui: the deck would not stop following\n");
        return 1;
    }

    // Putting the master down: the mode goes with it and the deck plays on exactly as it was — the
    // tempo a hand set is a decision, not a state to undo (owner's rule).
    if (!press(QStringLiteral("syncBeat_B"))) {
        return 1;
    }
    const bool following_again =
        waitFor([&]() { return modeOf(QStringLiteral("B")) == QStringLiteral("beat"); }, 5000);
    if (!following_again) {
        std::fprintf(stderr, "dj-ui: the sync button did not take before the master went\n");
        return 1;
    }
    const double ratio_before =
        deckOf(QStringLiteral("B")).value(QStringLiteral("tempo_ratio")).toDouble();
    if (!press(QStringLiteral("syncMaster_A"))) {
        return 1;
    }
    const bool leader_gone = waitFor(
        [&]() {
            return !deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool() &&
                   modeOf(QStringLiteral("B")) == QStringLiteral("off");
        },
        5000);
    if (!leader_gone) {
        std::fprintf(stderr,
                     "dj-ui: with the master put down the follower is A master %d, B mode %s\n",
                     deckOf(QStringLiteral("A")).value(QStringLiteral("master")).toBool(),
                     qPrintable(modeOf(QStringLiteral("B"))));
        return 1;
    }
    const double ratio_after =
        deckOf(QStringLiteral("B")).value(QStringLiteral("tempo_ratio")).toDouble();
    const double head_at_release = frameOf(QStringLiteral("B"));
    QElapsedTimer playing_on;
    playing_on.start();
    while (playing_on.elapsed() < 700) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
        QThread::msleep(10);
    }
    const double head_later = frameOf(QStringLiteral("B"));
    if (std::abs(ratio_after - ratio_before) > 1e-9) {
        std::fprintf(stderr, "dj-ui: the tempo moved when the master went (%.9f -> %.9f)\n",
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
            return !deckOf(QStringLiteral("A")).value(QStringLiteral("playing")).toBool() &&
                   !deckOf(QStringLiteral("B")).value(QStringLiteral("playing")).toBool();
        },
        5000);

    std::fprintf(
        stdout,
        "dj-ui: sync — MASTER on A and SYNC on B put the follower on the beat (phase %.3f "
        "ms), a stopped follower held its head at %.0f frames for 1,2 s while its "
        "distance from the master went %.3f -> %.3f ms and the needle followed it, one beat "
        "along the bar (%.3f beats) moved the needle %.0f px off centre, and putting the "
        "master down cleared the mode while the deck kept its speed %.9f and played %.0f "
        "frames on\n",
        phase_playing, held_at, phase_held, phase_after_stop, bar_along, offCentre, ratio_after,
        head_later - head_at_release);
    return 0;
}

// syncSoakOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.

int runSyncSoak(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
                QCommandLineParser& parser, const QCommandLineOption& connectOption,
                const QCommandLineOption& soakSecondsOption) {
    // S4: the numbers of S1 were arithmetic and the numbers of S2 lasted three steps. This is the
    // same question over ten minutes of playing decks: do two decks stay on one beat, and do they
    // come back after a hand changes something.
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    bridge->connectToEngine(parser.value(connectOption));
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
    const auto deckOf = [&](const QString& id) {
        const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == id) {
                return deck;
            }
        }
        return QVariantMap();
    };
    const auto frameOf = [&](const QString& id) {
        return deckOf(id).value(QStringLiteral("source_frame")).toLongLong();
    };
    const auto phaseOf = [&](const QString& id) {
        const QVariant phase = deckOf(id).value(QStringLiteral("phase_error_ms"));
        return phase.isValid() ? std::abs(phase.toDouble())
                               : std::numeric_limits<double>::infinity();
    };

    // Two click tracks of known tempo. A click is the one track whose grid cannot be argued about:
    // every beat is an impulse, so a deck that drifts shows it as a phase and not as a mood.
    QTemporaryDir soakDir;
    if (!soakDir.isValid()) {
        std::fprintf(stderr, "dj-ui: no temporary directory for the soak\n");
        return 1;
    }
    const int seconds = qMax(10, parser.value(soakSecondsOption).toInt());
    const auto writeClick = [&](const QString& path, double bpm) -> bool {
        const qint64 rate = 48000;
        // Longer than the soak, and not by a few seconds: the faster deck eats its material at
        // 128/120, and the decks start playing while they are still being lined up. A fixture that
        // ends mid-run does not end the run — it turns the last minute into numbers about nothing.
        // Ten minutes at 1.28 is thirteen minutes of material, and the decks play while they are
        // still being lined up: a fixture that ends mid-run turns the last minute into numbers
        // about nothing at all.
        const qint64 frames = static_cast<qint64>(rate) * (static_cast<qint64>(seconds * 1.5) + 60);
        const double beat = 60.0 / bpm;
        QFile file(path);
        if (!file.open(QIODevice::WriteOnly)) {
            return false;
        }
        const auto put32 = [&](quint32 value) {
            const char bytes[4] = {
                static_cast<char>(value & 0xff), static_cast<char>((value >> 8) & 0xff),
                static_cast<char>((value >> 16) & 0xff), static_cast<char>((value >> 24) & 0xff)};
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
            return std::abs(a.value(QStringLiteral("local_bpm")).toDouble() - 128.0) < 0.05 &&
                   std::abs(b.value(QStringLiteral("local_bpm")).toDouble() - 100.0) < 0.05 &&
                   a.value(QStringLiteral("beat")).isValid();
        },
        // Ten minutes of a click track is a hundred megabytes a deck, and a debug build measures
        // its tempo at about a tenth of real time: two decks of ten minutes are two and a half
        // minutes of analysis before the soak can start. The budget is the time the work takes, not
        // a guess that fits an eight-second fixture.
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
        std::fprintf(stderr, "dj-ui: the soak never locked (%.3f ms)\n",
                     phaseOf(QStringLiteral("B")));
        return 1;
    }
    // The first jump puts the deck on the beat; the tempo arrives by trim, and a deck that is still
    // catching up is not a deck that drifts. The soak starts once the follower runs at the master's
    // speed and stands on its beat: otherwise the first seconds measure the start-up, not the lock.
    // The master plays at its own speed and the follower must reach the master's *heard* tempo:
    // 128 BPM at 1.0 against 120 BPM at 1.0667. Comparing the ratios would compare a tempo with a
    // speed, which never matches and says nothing about locking.
    const auto heard = [&](const QString& deck) {
        const QVariantMap state = deckOf(deck);
        return state.value(QStringLiteral("local_bpm")).toDouble() *
               state.value(QStringLiteral("tempo_ratio")).toDouble();
    };
    const auto converged = [&]() {
        return std::abs(heard(QStringLiteral("A")) - heard(QStringLiteral("B"))) < 0.05 &&
               phaseOf(QStringLiteral("B")) < 2.0;
    };
    if (!waitFor(converged, 15000)) {
        std::fprintf(
            stderr, "dj-ui: the soak never settled (heard %.4f BPM against %.4f, phase %.3f ms)\n",
            heard(QStringLiteral("B")), heard(QStringLiteral("A")), phaseOf(QStringLiteral("B")));
        return 1;
    }
    std::fprintf(stderr, "dj-ui: soak — locked and settled, the clock starts here\n");

    struct Stage {
        const char* name;
        double at;
        /// Whether the deck has to be on the master's beat after this event. A pause and a tempo
        /// the fader cannot reach are not failures: the first has no phase to speak of, the second
        /// is a refusal the engine is supposed to make. Counting them would turn a correct
        /// behaviour into a red light, and a check that goes red on purpose teaches nothing.
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
    const auto stage_at = [&](double share) {
        return static_cast<qint64>(seconds * 1000.0 * share);
    };
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
    // A second after the start: the follower is put up, its first jump lands, and the material the
    // seek asked for arrives a few blocks later. A sample taken while that is happening is the deck
    // starting, not the deck drifting — the worst sample of a bad run was at 0,4 s.
    qint64 settled_from = 1000;
    // The audio clock is how this run tells a deck that drifted from a machine that could not
    // render: a block the box never produced moves a play head by a whole block, which is 5,3 ms
    // and looks exactly like a phase error. Counting those samples as drift would make the check a
    // measure of how busy the computer was.
    double clock_before =
        bridge->snapshot().value(QStringLiteral("audio_clock_samples")).toDouble();
    qint64 wall_before = run.elapsed();
    // A deck that asked its source for material and did not get it moves its play head by a whole
    // block when the material finally arrives, which a phase meter reads as a jump of 5,3 ms. The
    // deck counts those blocks itself; this run leaves them out of the phase statistics and says
    // how many there were.
    double ratio_prev = deckOf(QStringLiteral("B")).value(QStringLiteral("tempo_ratio")).toDouble();
    qint64 ratio_moved_at = 0;
    qint64 underruns_before = 0;
    for (const QString& id : {QStringLiteral("A"), QStringLiteral("B")}) {
        underruns_before += deckOf(id).value(QStringLiteral("underruns")).toLongLong();
    }
    // A deck that has not moved is a deck that was not rendered: a block the machine never gave it
    // moves the play head by one block, 5,3 ms at 256 frames, which reads as a phase error and is
    // not one. The audio clock cannot see this — the engine rendered, one deck was starved.
    qint64 master_before = frameOf(QStringLiteral("A"));
    qint64 follower_before = frameOf(QStringLiteral("B"));
    while (run.elapsed() < seconds * 1000) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
        const qint64 now = run.elapsed();
        while (next_stage < stages.size() && now >= stage_at(stages[next_stage].at)) {
            Stage& stage = stages[next_stage];
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
        // deck is catching up the phase is a distance travelled, not a distance kept. The transient
        // is recorded too, but as information — a correction that is heard is not a lock that was
        // lost.
        bool catching_up = false;
        for (const auto& recovery : recoveries) {
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
        const double expected = static_cast<double>(now - wall_before) * 48.0; // 48 frames a ms
        const qint64 master_now = frameOf(QStringLiteral("A"));
        const qint64 follower_now = frameOf(QStringLiteral("B"));
        const double master_moved = static_cast<double>(master_now - master_before);
        const double follower_moved = static_cast<double>(follower_now - follower_before);
        master_before = master_now;
        follower_before = follower_now;
        // Half of what the clock says should have passed is well below any real playback and well
        // above the noise of a block boundary, so a starved deck is caught and a slow one is not.
        qint64 underruns_now = 0;
        for (const QString& id : {QStringLiteral("A"), QStringLiteral("B")}) {
            underruns_now += deckOf(id).value(QStringLiteral("underruns")).toLongLong();
        }
        const bool starved = underruns_now > underruns_before;
        underruns_before = underruns_now;
        const bool stalled = starved || clock_now - clock_before < 0.8 * expected ||
                             master_moved < 0.5 * expected || follower_moved < 0.5 * expected;
        clock_before = clock_now;
        wall_before = now;
        const double phase = phaseOf(QStringLiteral("B"));
        const double ratio_now =
            deckOf(QStringLiteral("B")).value(QStringLiteral("tempo_ratio")).toDouble();
        if (std::abs(ratio_now - ratio_prev) > 1e-9) {
            ratio_prev = ratio_now;
            ratio_moved_at = now;
        }
        // A hand asks for a new tempo; the deck then walks to it at four percent a second, which
        // for a four percent change is a whole second of the follower running at the old speed. The
        // beat is apart for that second by design, so a sample taken inside it says nothing about
        // drift.
        const bool correcting = now - ratio_moved_at < 150;
        if (correcting && std::isfinite(phase)) {
            // The walk to a new tempo is a correction the hand asked for, and the beat is apart
            // while it lasts: that is the price of not jumping the sound.
            worst_transient = std::max(worst_transient, phase);
        }
        if (qEnvironmentVariableIsSet("DJP_SOAK_TRACE") && std::isfinite(phase) &&
            std::abs(phase) > 2.0 && std::abs(phase) < 30.0) {
            const auto a = deckOf(QStringLiteral("A"));
            const auto b = deckOf(QStringLiteral("B"));
            fprintf(
                stderr,
                "trace t=%.1f s phase=%.3f ms | A frame %lld playing %d | B frame %lld playing %d "
                "sync %d ratio %.6f underruns %lld | clock %.1f\n",
                run.elapsed() / 1000.0, phase,
                static_cast<long long>(a.value(QStringLiteral("source_frame")).toDouble()),
                a.value(QStringLiteral("playing")).toBool(),
                static_cast<long long>(b.value(QStringLiteral("source_frame")).toDouble()),
                b.value(QStringLiteral("playing")).toBool(),
                b.value(QStringLiteral("sync")).toBool(),
                b.value(QStringLiteral("tempo_ratio")).toDouble(),
                static_cast<long long>(underruns_before), clock_now);
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
        // A recovery has to hold: one sample inside the tolerance can be luck, a whole second of
        // them is the deck being back on the beat.
        for (auto& recovery : recoveries) {
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
                recovery.seconds =
                    (recovery.hold_from - static_cast<double>(recovery.since)) / 1000.0;
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
    for (const auto& recovery : recoveries) {
        if (!recovery.counted) {
            std::fprintf(stdout, "dj-ui: soak   %s — no phase to hold\n",
                         qPrintable(recovery.name));
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
                 "dj-ui: soak — worst |phase| %.3f ms at %.1f s while settled (%.3f ms during a "
                 "correction), "
                 "%.2f%% of the settled samples inside 2 ms, worst recovery %.1f s after a hand\n",
                 worst_aligned, worst_aligned_at, worst_transient, share_inside, worst_recovery);
    return worst_aligned <= 2.0 && share_inside >= 99.0 && all_recovered && worst_recovery <= 5.0
               ? 0
               : 1;
}

// loopOption: one check of the shell, out of main.
//
// Split out of main: a function of ten thousand lines loses its debug information, and GCC
// says so (`variable tracking size limit exceeded`). The checks are the tests; a named
// function is where a failure is read.

int runLoop(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
            QCommandLineParser& parser, const QCommandLineOption& connectOption,
            const QCommandLineOption& loadOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return map;
            }
        }
        return QVariantMap();
    };
    auto* loopButton = findItem(window->contentItem(), QStringLiteral("loop_A"));
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
    auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
    const auto stripProp = [&](const char* name) {
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
        std::fprintf(stderr,
                     "dj-ui: the first press activated a loop; it must only mark the start\n");
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
    std::fprintf(
        stdout, "dj-ui: the second press closed %.0f…%.0f, active %s, armed %d, playhead at %.0f\n",
        start, end, active ? "yes" : "no", armedCount(), parked);
    if (std::abs(start - 48000.0) > 2000.0) {
        std::fprintf(stderr, "dj-ui: the loop does not start where the first press was\n");
        return 1;
    }
    // A playing deck runs on between the read and the click, so the end is compared loosely: what
    // the check rules out is a loop that ends anywhere but at the press.
    if (std::abs(end - pressed) > 8000.0) {
        std::fprintf(stderr, "dj-ui: the loop does not end where the second press was (%.0f)\n",
                     pressed);
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
    std::fprintf(stdout,
                 "dj-ui: after 12 steps of playing the playhead reached %.0f (loop ends %.0f)\n",
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
    for (const QVariant& entry :
         strip == nullptr ? QVariantList() : strip->property("padLoops").toList()) {
        if (entry.toMap().value(QStringLiteral("active")).toBool()) {
            anyActiveMark = true;
        }
    }
    const bool painted = savedMarks > 0;
    const bool stillActive = anyActiveMark;
    std::fprintf(
        stdout,
        "dj-ui: shift switched the loop %s, edges still painted %s, playhead %.0f -> %.0f\n",
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
    std::fprintf(stdout,
                 "dj-ui: a press outside the loop left a marker at %.4f, playhead %.0f -> %.0f\n",
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
        std::fprintf(stderr,
                     "dj-ui: the old loop is still in force while a new one is being marked\n");
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
    std::fprintf(
        stdout,
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
    std::fprintf(stdout, "dj-ui: saving a playing loop went back by %.0f frames, armed %d -> %d\n",
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
        for (const QVariant& entry : strip->property("padLoops").toList()) {
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
    std::fprintf(
        stdout,
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
    std::fprintf(stdout,
                 "dj-ui: from before the first loop the playhead reached %.0f (ends 40000)\n",
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

int runEqCheck(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
               QCommandLineParser& parser, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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

    QQuickItem* content = window->contentItem();
    const QString decks[2] = {QStringLiteral("A"), QStringLiteral("B")};
    const QString bands[3] = {QStringLiteral("low"), QStringLiteral("mid"), QStringLiteral("high")};
    QQuickItem* knobs[2][3] = {{nullptr, nullptr, nullptr}, {nullptr, nullptr, nullptr}};
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
    auto* meterA = findItem(content, QStringLiteral("meter_A"));
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
    auto* trimA = findItem(content, QStringLiteral("trim_A"));
    auto* trimB = findItem(content, QStringLiteral("trim_B"));
    if (trimA == nullptr || trimB == nullptr) {
        std::fprintf(stderr, "dj-ui: a channel has no gain knob\n");
        return 1;
    }

    // The owner asked for the same cap as the master knob: a band control is the same class of
    // control as the master gain, and a smaller cap reads as a lesser one. Measured, not assumed.
    auto* masterKnob = findItem(content, QStringLiteral("masterKnob"));
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
    for (QQuickItem* trim : {trimA, trimB}) {
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
    // Both knobs, or the angle below measures whatever the previous check left behind: the gate
    // runs these checks one after another on one engine.
    const auto gainOfA = [&]() {
        const QVariantList published = bridge->snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant& entry : published) {
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
    const auto turnTwice = [&](QQuickItem* knob, const char* name, double& up, double& down,
                               double& dbUp, double& dbDown) {
        if (knob == nullptr) {
            return false;
        }
        const QPointF grab = knob->mapToScene(QPointF(knob->width() / 2.0, knob->height() / 2.0));
        const auto send = [&](QEvent::Type type, const QPointF& at) {
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
                     "dj-ui: ten pixels turn %s %+.0f and %+.0f degrees (%+.1f / %+.1f dB)\n", name,
                     up, down, dbUp, dbDown);
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
    QQuickItem* eqLowA = findItem(window->contentItem(), QStringLiteral("eq_low_A"));
    double bandUp = 0.0;
    double bandDown = 0.0;
    double bandDbUp = 0.0;
    double bandDbDown = 0.0;
    if (turnTwice(eqLowA, "the low band", bandUp, bandDown, bandDbUp, bandDbDown) &&
        (std::abs(bandUp - 90.0) > 2.0 || std::abs(bandDown + 90.0) > 2.0)) {
        std::fprintf(stderr, "dj-ui: the band knob turns at different rates up and down\n");
        return 1;
    }
    // The foot of the knob is a detent, and it is not another decibel: turn the hand far past the
    // end of the cut and the band has to arrive at the engine killed, not at the deepest cut. A
    // knob that only reached -26 would leave an isolator that nothing can reach.
    if (eqLowA != nullptr) {
        const auto eqOfA = [&](const QString& band) {
            const QVariantList published =
                bridge->snapshot().value(QStringLiteral("decks")).toList();
            for (const QVariant& entry : published) {
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
            const auto send = [&](QEvent::Type type, const QPointF& at) {
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
            std::fprintf(stderr, "dj-ui: the detent sent %.1f dB, so the isolator is unreachable\n",
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
    // The gesture above left deck A's low band where the hand dropped it; the checks below want a
    // flat channel, so put the knob and the deck back before measuring anything else.
    bridge->setChannelGain(QStringLiteral("A"), 1.0);
    bridge->setChannelEq(QStringLiteral("A"), QStringLiteral("low"), 0.0);
    for (int step = 0; step < 40; ++step) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        QThread::msleep(5);
    }

    const double masterTop = masterKnob->mapToItem(content, QPointF(0, 0)).y();
    for (QQuickItem* trim : {trimA, trimB}) {
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
        std::fprintf(stdout, "dj-ui: the %s knob is labelled %s\n", qPrintable(bands[band]),
                     qPrintable(letter));
    }

    // What the engine says about one band, straight from the published snapshot.
    const auto bandOf = [&](int deck, int band) {
        const QVariantList published = bridge->snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant& entry : published) {
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
            std::fprintf(stdout, "dj-ui: knob %s %s shows %+.1f dB, the engine says %+.1f dB\n",
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
    if (std::abs(bandOf(0, 0)) > 0.01 || std::abs(bandOf(0, 1)) > 0.01 ||
        std::abs(bandOf(0, 2)) > 0.01) {
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

int runGridLines(QQmlApplicationEngine& engine, std::unique_ptr<BridgeClient>& bridge,
                 QCommandLineParser& parser, const QCommandLineOption& connectOption,
                 const QCommandLineOption& loadOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return deck;
            }
        }
        return QVariantMap();
    };
    // The deck may already hold this very file from another client: then "the deck shows the file with a grid"
    // is true before the load is even read, and the check would act on the track the *previous* load put there.
    // Emptying first makes the wait below mean what it says — this load, not whatever was left.
    bridge->eject(QStringLiteral("A"));
    if (!waitFor(
            [&]() {
                const QVariantMap deck = deckA();
                return deck.value(QStringLiteral("path")).toString().isEmpty()
                    && deck.value(QStringLiteral("total_frames")).toDouble() <= 0.0;
            },
            3000)) {
        std::fprintf(stderr, "dj-ui: the deck did not empty before the load\n");
        return 1;
    }
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

    auto* view = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
    auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
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
                return deckA().value(QStringLiteral("source_frame")).toDouble() >=
                       total / 2.0 - 1.0;
            },
            5000)) {
        const QVariantMap state = deckA();
        std::fprintf(stderr,
                     "dj-ui: the deck never moved to the middle of the track (load %s playing %d from %.0f "
                     "remaining %.0f total %.0f tempo %.2f slip %d cached %d)\n",
                     qPrintable(state.value(QStringLiteral("load")).toString()),
                     state.value(QStringLiteral("playing")).toBool() ? 1 : 0,
                     state.value(QStringLiteral("source_frame")).toDouble(),
                     state.value(QStringLiteral("remaining_frames")).toDouble(),
                     state.value(QStringLiteral("total_frames")).toDouble(),
                     state.value(QStringLiteral("tempo_ratio")).toDouble(),
                     state.value(QStringLiteral("slip")).toBool() ? 1 : 0,
                     state.value(QStringLiteral("cached")).toBool() ? 1 : 0);
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
    // ...and centred on the middle of the track, so the whole window holds track and not lead-in.
    // The strip is centred on the play head, so this is also the proof that the seek arrived — and
    // a command that can be dropped while the deck settles is asked again instead of being believed
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
                return lines() >= 4 && spacing() > 0.0 &&
                       lines() * spacing() >= 0.7 * strip->width();
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
                     beats, bars, strip->property("windowStart").toDouble(), span,
                     strip->property("playheadRatio").toDouble(), int(strip->width()));
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
        auto* label = findItem(window->contentItem(), QStringLiteral("bar_A"));
        return label != nullptr ? label->property("text").toString() : QString();
    };
    if (!waitFor([&]() { return squares() >= 1 && firstSquare() >= 0.0; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the strip marked no square of four bars (squares %d)\n",
                     squares());
        return 1;
    }
    const double periodRatio = (rate * 60.0 / bpm) / total;
    // The count is read at the play head, so the check puts the play head where it meant to have
    // it. Two reasons, both measured. A playing deck walks into the next bar while the check waits
    // seconds for a shift. And a deck stopped the way a CDJ stops comes back to its cue point,
    // which may be before the grid altogether — a count read there belongs to a bar that is not in
    // the track. One beat past the middle: the strip centres on the play head, and the middle keeps
    // the window on the whole track, while a beat past it keeps the count off a bar boundary.
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
    auto* barRight = findItem(window->contentItem(), QStringLiteral("gridBeatRight_A"));
    auto* barLeft = findItem(window->contentItem(), QStringLiteral("gridBeatLeft_A"));
    if (barRight == nullptr || barLeft == nullptr) {
        std::fprintf(stderr, "dj-ui: there is no bar button to press\n");
        return 1;
    }
    QMetaObject::invokeMethod(barRight, "clicked");
    if (!waitFor([&]() { return std::abs(firstSquare() - (squareBefore + periodRatio)) < 0.002; },
                 5000)) {
        std::fprintf(stderr, "dj-ui: a beat shift moved the square start to %.4f, not to %.4f\n",
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
                     squareBefore, firstSquare(), qPrintable(labelBefore), qPrintable(barLabel()),
                     deckA().value(QStringLiteral("beat")).toDouble(), frameNow(), drawnBeats(),
                     drawnBars(), strip->property("drawnSquares").toInt(),
                     strip->property("windowStart").toDouble(), strip->property("span").toDouble(),
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
    // One click per block: four clicks inside one block are one shift, not four, and four beats
    // have to be four shifts.
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
    std::fprintf(
        stdout,
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
                return std::abs(deckA().value(QStringLiteral("beat")).toDouble() - beatBefore -
                                6.0) < 0.2;
            },
            5000)) {
        std::fprintf(stderr,
                     "dj-ui: six presses of one beat moved the play head's beat from %.2f to %.2f, "
                     "not by six\n",
                     beatBefore, deckA().value(QStringLiteral("beat")).toDouble());
        return 1;
    }
    const auto firstFrame = [&]() {
        return gridOf().isEmpty()
                   ? -1.0
                   : gridOf().first().toMap().value(QStringLiteral("frame")).toDouble();
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
    std::fprintf(
        stdout,
        "dj-ui: six presses of one beat walked the anchor past the start and kept %d lines\n",
        linesAfter);
    for (int press = 0; press < 6; ++press) {
        QMetaObject::invokeMethod(barRight, "clicked");
        QCoreApplication::processEvents();
        QThread::msleep(150);
    }

    // Half the tempo and the lines move with the grid: this is the proof that they are the grid and
    // not a decoration of the waveform.
    auto* half = findItem(window->contentItem(), QStringLiteral("gridHalf_A"));
    if (half == nullptr) {
        std::fprintf(stderr, "dj-ui: there is no half button to press\n");
        return 1;
    }
    QMetaObject::invokeMethod(half, "clicked");
    if (!waitFor([&]() { return std::abs(spacing() - 2.0 * expected) < 4.0; }, 5000)) {
        std::fprintf(stderr, "dj-ui: after half the beat lines are %.1f px apart, not %.1f\n",
                     spacing(), 2.0 * expected);
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: the strip draws the grid: %d beats, %d bars, %.1f px a beat at %.2f BPM, "
                 "%.1f px after half\n",
                 beats, bars, expected, bpm, spacing());
    return 0;
}

// The rows under the wave, measured by the check that loads a track with a grid. The wave is the
// only child of the deck with fillHeight, so every row takes its height from the wave: folding a
// row has to give that height back, and the two groups of the folding strip have to sit on the
// panel's edges. The window is given a size first: in a headless run it is 0x0, and every
// measurement is then about an arrangement nobody has.

int runLayoutCheck(QQmlApplicationEngine& engine, LayoutSettings& layout, UiSettings& settings) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    QEventLoop settle;
    QTimer::singleShot(600, &settle, &QEventLoop::quit);
    settle.exec();
    auto* content = window->contentItem();
    auto* globalBar = findItem(content, QStringLiteral("globalBar"));
    auto* deckArea = findItem(content, QStringLiteral("deckArea"));
    auto* browser = findItem(content, QStringLiteral("browserPanel"));
    auto* mixer = findItem(content, QStringLiteral("mixer"));
    if (globalBar == nullptr || deckArea == nullptr || browser == nullptr || mixer == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no regions by name\n");
        return 1;
    }
    auto* shell = globalBar->parentItem();
    if (shell == nullptr || shell->height() <= 0.0 || deckArea->width() <= 0.0) {
        std::fprintf(stderr, "dj-ui: the shell has no size yet\n");
        return 1;
    }
    const double height = shell->height();
    const double width = deckArea->width();
    const double globalShare = globalBar->height() / height * 100.0;
    const double deckShare = deckArea->height() / height * 100.0;
    const double browserShare = browser->height() / height * 100.0;
    const bool noMixer = layout.mixerEdge() == QStringLiteral("none");
    if (noMixer && mixer->isVisible()) {
        std::fprintf(stderr, "dj-ui: %s says no mixer, and the mixer is on the screen\n",
                     qPrintable(layout.preset()));
        return 1;
    }
    if (!noMixer && !mixer->isVisible()) {
        std::fprintf(stderr, "dj-ui: %s has no mixer on the screen\n", qPrintable(layout.preset()));
        return 1;
    }
    const double mixerShare = noMixer ? 0.0 : mixer->width() / width * 100.0;

    const auto report = [&](const char* name, double actual, double wanted) {
        std::fprintf(stdout, "dj-ui: layout %-8s %5.1f%% (wanted %4.1f%%)\n", name, actual, wanted);
    };
    report("global", globalShare, layout.globalPercent());
    report("decks", deckShare, layout.deckRowsPercent());
    report("browser", browserShare, layout.browserPercent());
    if (!noMixer) {
        report("mixer", mixerShare, layout.mixerPercent());
    }

    const auto off = [](double actual, double wanted) { return std::abs(actual - wanted) > 2.0; };
    if (off(globalShare, layout.globalPercent())
        || (!noMixer && off(mixerShare, layout.mixerPercent()))) {
        std::fprintf(stderr, "dj-ui: the layout does not match its settings\n");
        return 1;
    }
    if (off(deckShare, layout.deckRowsPercent()) || off(browserShare, layout.browserPercent())) {
        std::fprintf(stderr, "dj-ui: the layout does not match its settings\n");
        return 1;
    }
    // What the arrangement promises: which decks stand on the screen, and which must not.
    const QString arrangement = layout.arrangement();
    QStringList expected;
    if (arrangement == QStringLiteral("duo") || arrangement == QStringLiteral("stack-duo")) {
        expected = {QStringLiteral("A"), QStringLiteral("B")};
    } else if (arrangement == QStringLiteral("studio")) {
        expected = {QStringLiteral("A")};
    } else {
        expected = {QStringLiteral("A"), QStringLiteral("B"), QStringLiteral("C"), QStringLiteral("D")};
    }
    // The mixer's place: the edge it was asked for, or between the columns when it is in the middle.
    const QRectF mixerRect = mixer->mapRectToScene(QRectF(0, 0, mixer->width(), mixer->height()));
    const QRectF areaRect = deckArea->mapRectToScene(QRectF(0, 0, deckArea->width(), deckArea->height()));
    const QString edge = layout.mixerEdge();
    if (edge == QStringLiteral("left") && std::abs(mixerRect.left() - areaRect.left()) > 2.0) {
        std::fprintf(stderr, "dj-ui: the mixer stands %.0f px from the left edge, not on it\n",
                     mixerRect.left() - areaRect.left());
        return 1;
    }
    if (edge == QStringLiteral("right") && std::abs(mixerRect.right() - areaRect.right()) > 2.0) {
        std::fprintf(stderr, "dj-ui: the mixer stands %.0f px from the right edge, not on it\n",
                     areaRect.right() - mixerRect.right());
        return 1;
    }

    // Each deck, measured: a deck that grows tall enough to swallow the window is what "too
    // puffy" means, so the numbers are printed next to the layout's.
    QStringList onScreen;
    for (const char* id : {"A", "B", "C", "D"}) {
        auto* deck = findItem(content, QStringLiteral("deckPanel_%1").arg(QString::fromLatin1(id)));
        auto* detail =
            findItem(content, QStringLiteral("deckDetail_%1").arg(QString::fromLatin1(id)));
        auto* overview =
            findItem(content, QStringLiteral("deckOverview_%1").arg(QString::fromLatin1(id)));
        const QString wanted = QString::fromLatin1(id);
        if (!expected.contains(wanted)) {
            // A layout without that deck must not have it: a panel that exists but is hidden still
            // answers to its name, and the checks would find the wrong one.
            if (deck != nullptr) {
                std::fprintf(stderr, "dj-ui: deck %s is in a layout that has no room for it\n", id);
                return 1;
            }
            continue;
        }
        if (deck == nullptr || detail == nullptr || overview == nullptr || !deck->isVisible()) {
            std::fprintf(stderr, "dj-ui: deck %s has no strips (panel %s, strip %s, overview %s)\n",
                         id, deck != nullptr ? "found" : "missing",
                         detail != nullptr ? "found" : "missing",
                         overview != nullptr ? "found" : "missing");
            return 1;
        }
        onScreen.append(wanted);
        // The rows above and below the wave, measured: the four decks in a column have to know what
        // the panel costs before anybody argues about a threshold.
        const auto rowHeight = [&](const QString& name) {
            QQuickItem* item = findItem(content, name);
            if (item == nullptr) {
                return QStringLiteral("absent");
            }
            if (!item->isVisible()) {
                return QStringLiteral("hidden");
            }
            return QStringLiteral("%1").arg(item->height(), 0, 'f', 0);
        };
        const QString deckId = QString::fromLatin1(id);
        std::fprintf(stdout,
                     "dj-ui: deck %s %.0f px tall, strip %.0f px, overview %.0f px; rows: header %s, "
                     "wave %s, transport %s, loop %s, grid %s\n",
                     id, deck->height(), detail->height(), overview->height(),
                     qPrintable(rowHeight(QStringLiteral("header_") + deckId)),
                     qPrintable(rowHeight(QStringLiteral("wave_") + deckId)),
                     qPrintable(rowHeight(QStringLiteral("transport_") + deckId)),
                     qPrintable(rowHeight(QStringLiteral("loopsize_") + deckId)),
                     qPrintable(rowHeight(QStringLiteral("gridedit_") + deckId)));
    }
    if (onScreen != expected) {
        std::fprintf(stderr, "dj-ui: %s shows %s, not %s\n", qPrintable(layout.preset()),
                     qPrintable(onScreen.join(QStringLiteral(", "))),
                     qPrintable(expected.join(QStringLiteral(", "))));
        return 1;
    }
    // In the middle the mixer stands between the columns: something on its left, something on its
    // right. An edge mixer has already been checked against its edge.
    if (edge == QStringLiteral("centre")) {
        const auto rectOf = [&](const QString& name) {
            QQuickItem* item = findItem(content, name);
            return item == nullptr ? QRectF()
                                   : item->mapRectToScene(QRectF(0, 0, item->width(), item->height()));
        };
        const QRectF leftColumn = rectOf(QStringLiteral("leftDecks"));
        const QRectF rightColumn = rectOf(QStringLiteral("rightDecks"));
        if (leftColumn.right() > mixerRect.left() + 2.0 || rightColumn.left() < mixerRect.right() - 2.0) {
            std::fprintf(stderr, "dj-ui: the mixer is not between the columns (left ends %.0f, mixer %.0f..%.0f, right starts %.0f)\n",
                         leftColumn.right(), mixerRect.left(), mixerRect.right(), rightColumn.left());
            return 1;
        }
    }
    // A column of decks: same left edge, same width, each one below the previous. That is what "deck
    // under deck" means, and it is what Stack and Stack Duo promise.
    if (arrangement == QStringLiteral("stack") || arrangement == QStringLiteral("stack-duo")) {
        QRectF previous;
        for (const QString& deckId : onScreen) {
            auto* deck = findItem(content, QStringLiteral("deckPanel_") + deckId);
            const QRectF rect = deck->mapRectToScene(QRectF(0, 0, deck->width(), deck->height()));
            if (!previous.isNull()) {
                if (std::abs(rect.left() - previous.left()) > 2.0
                    || std::abs(rect.width() - previous.width()) > 2.0) {
                    std::fprintf(stderr,
                                 "dj-ui: deck %s stands at %.0f..%.0f, the one above at %.0f..%.0f\n",
                                 qPrintable(deckId), rect.left(), rect.right(), previous.left(),
                                 previous.right());
                    return 1;
                }
                if (rect.top() <= previous.top()) {
                    std::fprintf(stderr, "dj-ui: deck %s is not under the deck above it (%.0f vs %.0f)\n",
                                 qPrintable(deckId), rect.top(), previous.top());
                    return 1;
                }
            }
            previous = rect;
            auto* wave = findItem(content, QStringLiteral("deckDetail_") + deckId);
            if (wave == nullptr || wave->height() < 40.0) {
                std::fprintf(stderr, "dj-ui: deck %s has a wave of %.0f px, under its floor\n",
                             qPrintable(deckId), wave == nullptr ? 0.0 : wave->height());
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: %s stands in one column of %d decks, each wave at least 40 px\n",
                     qPrintable(layout.preset()), static_cast<int>(onScreen.size()));
        // The fold is the promise for a smaller window: shrink it and the rows a hand can live without
        // must go before the wave does.
        window->resize(1500, 700);
        QElapsedTimer shrinking;
        shrinking.start();
        while (shrinking.elapsed() < 600) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            QThread::msleep(10);
        }
        for (const QString& deckId : onScreen) {
            auto* wave = findItem(content, QStringLiteral("deckDetail_") + deckId);
            auto* gridRow = findItem(content, QStringLiteral("gridedit_") + deckId);
            if (wave == nullptr || wave->height() < 39.0) {
                std::fprintf(stderr,
                             "dj-ui: at 1500x700 deck %s leaves the wave %.0f px, under its floor\n",
                             qPrintable(deckId), wave == nullptr ? 0.0 : wave->height());
                return 1;
            }
            if (gridRow != nullptr && gridRow->isVisible()) {
                std::fprintf(stderr, "dj-ui: at 1500x700 deck %s keeps a row it cannot afford\n",
                             qPrintable(deckId));
                return 1;
            }
        }
        std::fprintf(stdout, "dj-ui: at 1500x700 every deck folded its rows and kept its wave\n");
    }
    // Preparation: one deck, and it owns the width. A "wide" deck that takes half the room would leave
    // the DJ preparing in a panel the size of a mixing deck.
    if (arrangement == QStringLiteral("studio")) {
        auto* deck = findItem(content, QStringLiteral("deckPanel_A"));
        const QRectF area = deckArea->mapRectToScene(QRectF(0, 0, deckArea->width(), deckArea->height()));
        const double share = deck->width() / area.width() * 100.0;
        if (share < 60.0) {
            std::fprintf(stderr, "dj-ui: the single deck takes %.0f%% of the deck area, not the room\n",
                         share);
            return 1;
        }
        std::fprintf(stdout, "dj-ui: %s gives deck A %.0f%% of the deck area\n",
                     qPrintable(layout.preset()), share);
    }
    std::fprintf(stdout, "dj-ui: the layout in force is %s (%s, mixer %s): decks %s\n",
                 qPrintable(layout.preset()), qPrintable(arrangement), qPrintable(edge),
                 qPrintable(onScreen.join(QStringLiteral(", "))));
    // At rest nothing plays, so every deck button in the browser must look the same: a button
    // that wears a selection colour just because it is the first in the row says nothing true.
    {
        QColor first;
        for (const char* id : {"A", "B", "C", "D"}) {
            // The browser's strip offers the decks the layout shows; a button for a deck that is not on
            // the screen would be a target the DJ cannot see the result of.
            if (!expected.contains(QString::fromLatin1(id))) {
                auto* hidden =
                    findItem(content, QStringLiteral("deckButton_%1").arg(QString::fromLatin1(id)));
                if (hidden != nullptr && hidden->isVisible()) {
                    std::fprintf(stderr, "dj-ui: deck %s has a button in a layout without that deck\n", id);
                    return 1;
                }
                continue;
            }
            auto* button =
                findItem(content, QStringLiteral("deckButton_%1").arg(QString::fromLatin1(id)));
            if (button == nullptr) {
                std::fprintf(stderr, "dj-ui: the browser has no button for deck %s\n", id);
                return 1;
            }
            const QColor face = button->property("faceColor").value<QColor>();
            if (first.isValid() && face != first) {
                std::fprintf(stderr, "dj-ui: deck %s looks different at rest (%s against %s)\n", id,
                             qPrintable(face.name()), qPrintable(first.name()));
                return 1;
            }
            first = face;
        }
        std::fprintf(stdout, "dj-ui: deck buttons at rest are all %s\n", qPrintable(first.name()));
        if (first != QColor(settings.themeRaised())) {
            std::fprintf(stderr, "dj-ui: deck buttons do not wear the resting colour\n");
            return 1;
        }
    }

    // The window a deck opens at, measured from the item itself: a number of beats, not a fraction of the
    // track — a fraction is a scale nobody can compare between two tracks (owner's decision, 2026-11-20).
    if (auto* stripView = findItem(content, QStringLiteral("deckDetail_A")); stripView != nullptr) {
        const double beats = stripView->property("beatsOnScreen").toDouble();
        std::fprintf(stdout, "dj-ui: deck A opens at %.0f beats on screen (the setting says %.0f)\n", beats,
                     layout.waveformBeats());
        if (std::abs(beats - layout.waveformBeats()) > 0.01) {
            std::fprintf(stderr, "dj-ui: the deck does not open at the beats the setting names\n");
            return 1;
        }
    }

    // A layout without a mixer stops here: the strips, the laws and the crossfader are not on the screen,
    // and a check that looks for them would report the arrangement as broken.
    if (noMixer) {
        std::fprintf(stdout, "dj-ui: %s folds the mixer away; the decks own the row\n",
                     qPrintable(layout.preset()));
        return 0;
    }

    // The mixer, measured: the strips in the order the setting names, and the crossfader handle
    // resting in the middle at 0.5 rather than against the left stop.
    QStringList placed;
    for (const QChar letter : layout.channelOrder()) {
        auto* strip = findItem(content, QStringLiteral("strip_%1").arg(letter));
        if (strip == nullptr) {
            std::fprintf(stderr, "dj-ui: the mixer has no strip %s\n", qPrintable(letter));
            return 1;
        }
        placed.append(QStringLiteral("%1@%2").arg(letter).arg(int(strip->x())));
    }
    std::fprintf(stdout, "dj-ui: mixer order %s (wanted %s)\n",
                 qPrintable(placed.join(QStringLiteral(" "))), qPrintable(layout.channelOrder()));

    auto* handle = findItem(content, QStringLiteral("crossfaderHandle"));
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
    if (auto* stripA = findItem(content, QStringLiteral("strip_A")); stripA != nullptr) {
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
    auto* knob = findItem(content, QStringLiteral("fader_A"));
    auto* meter = findItem(content, QStringLiteral("meter_A"));
    if (knob != nullptr && meter != nullptr) {
        std::fprintf(stdout, "dj-ui: knob %.0f px, meter %.0f px, difference %.0f px\n",
                     knob->height(), meter->height(), std::abs(knob->height() - meter->height()));
        if (std::abs(knob->height() - meter->height()) > 2.0) {
            std::fprintf(stderr, "dj-ui: the knob is not the height of its meter\n");
            return 1;
        }
    }

    // The two global modes stand in the master column, centred under its knob: they belong to the
    // whole mix, and the master is where the whole mix is decided (owner's decision, 14.11).
    {
        auto* masterColumn = findItem(content, QStringLiteral("masterColumn"));
        auto* masterKnobItem = findItem(content, QStringLiteral("masterKnob"));
        auto* snapGlobal = findItem(content, QStringLiteral("snapGlobal"));
        auto* quantizeGlobal = findItem(content, QStringLiteral("quantizeGlobal"));
        if (masterColumn == nullptr || masterKnobItem == nullptr || snapGlobal == nullptr ||
            quantizeGlobal == nullptr) {
            std::fprintf(stderr, "dj-ui: the mixer has no master column or no global switches\n");
            return 1;
        }
        const double knobAxis =
            masterKnobItem->mapToItem(content, QPointF(masterKnobItem->width() / 2.0, 0.0)).x();
        const double switchAxis =
            snapGlobal->mapToItem(content, QPointF(snapGlobal->width() / 2.0, 0.0)).x();
        const double otherAxis =
            quantizeGlobal->mapToItem(content, QPointF(quantizeGlobal->width() / 2.0, 0.0)).x();
        const double knobBottom =
            masterKnobItem->mapToItem(content, QPointF(0.0, masterKnobItem->height())).y();
        const double switchTop = snapGlobal->mapToItem(content, QPointF(0.0, 0.0)).y();
        const double switchBottom =
            quantizeGlobal->mapToItem(content, QPointF(0.0, quantizeGlobal->height())).y();
        std::fprintf(
            stdout,
            "dj-ui: the global switches stand %.1f px off the master's axis, %.1f px under its "
            "knob, %.1f px tall together, and end %.1f px above the column's bottom\n",
            std::abs(switchAxis - knobAxis), switchTop - knobBottom, switchBottom - switchTop,
            masterColumn->mapToItem(content, QPointF(0.0, masterColumn->height())).y() -
                switchBottom);
        if (std::abs(switchAxis - knobAxis) > 2.0 || std::abs(otherAxis - knobAxis) > 2.0) {
            std::fprintf(stderr,
                         "dj-ui: the global switches are not centred under the master's knob\n");
            return 1;
        }
        if (switchTop < knobBottom) {
            std::fprintf(stderr, "dj-ui: the global switches are not below the master's knob\n");
            return 1;
        }
        if (switchBottom >
            masterColumn->mapToItem(content, QPointF(0.0, masterColumn->height())).y()) {
            std::fprintf(stderr, "dj-ui: the global switches hang past the master column\n");
            return 1;
        }
    }
    // At rest nothing is playing, so no deck frame may wear the accent: a highlight that sits on
    // one deck for ever is the bug the owner saw.
    const QColor accent = QColor(settings.themeAccent());
    QStringList framed;
    for (const char* id : {"A", "B", "C", "D"}) {
        // A deck the arrangement does not show has no frame to check — and must not have one.
        if (!expected.contains(QString::fromLatin1(id))) {
            continue;
        }
        auto* frame =
            findItem(content, QStringLiteral("deckFrame_%1").arg(QString::fromLatin1(id)));
        if (frame == nullptr) {
            std::fprintf(stderr, "dj-ui: deck %s has no frame\n", id);
            return 1;
        }
        auto* pen = frame->property("border").value<QObject*>();
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
    for (const char* id : {"A", "B", "C", "D"}) {
        auto* stripKnob =
            findItem(content, QStringLiteral("fader_%1").arg(QString::fromLatin1(id)));
        auto* stripMeter =
            findItem(content, QStringLiteral("meter_%1").arg(QString::fromLatin1(id)));
        if (stripKnob == nullptr || stripMeter == nullptr) {
            std::fprintf(stderr, "dj-ui: deck %s has no knob or meter\n", id);
            return 1;
        }
        const QPointF knobAt = stripKnob->mapToScene(QPointF(0, 0));
        const QPointF meterAt = stripMeter->mapToScene(QPointF(0, 0));
        const double shift = std::abs(knobAt.x() - meterAt.x()) +
                             std::abs(knobAt.y() - meterAt.y()) +
                             std::abs(stripKnob->width() - stripMeter->width()) +
                             std::abs(stripKnob->height() - stripMeter->height());
        std::fprintf(stdout, "dj-ui: deck %s pointer on the meter, %.0f px apart\n", id, shift);
        if (shift > 2.0) {
            std::fprintf(stderr, "dj-ui: deck %s has its pointer beside the meter, not on it\n",
                         id);
            return 1;
        }
    }

    // And the fader has to be painted, not merely placed: at rest it stands at unity, which for a
    // channel fader is the top of the scale, so a bright line must cross the meter there over a
    // dark background.
    if (auto* pointer = findItem(content, QStringLiteral("fader_A")); pointer != nullptr) {
        const QImage frame = window->grabWindow();
        if (!frame.isNull()) {
            const QPointF origin = pointer->mapToScene(QPointF(0, 0));
            const double ratio =
                content->width() > 0 ? static_cast<double>(frame.width()) / content->width() : 1.0;
            // The fader at unity sits on the first line of the scale, three pixels in from the top
            // edge — the same inset the knob draws its travel with.
            const double unityY = origin.y() + 3.0;
            // Away from the middle: the grip sits there, and this is a check about the line.
            const double sampleX = origin.x() + 4.0;
            const QColor onLine = frame.pixelColor(static_cast<int>(sampleX * ratio),
                                                   static_cast<int>(unityY * ratio));
            const QColor below =
                frame.pixelColor(static_cast<int>(sampleX * ratio),
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
    for (const QString& law : laws) {
        auto* button = findItem(content, QStringLiteral("curveButton_") + law);
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
    std::fprintf(stdout,
                 "dj-ui: the four laws sit in one row %.0f px wide, %.0f px tall, "
                 "narrowest %s at %.0f px\n",
                 row.width(), rowBottom - rowTop, qPrintable(narrowestLaw), narrowest);
    if (auto* last = findItem(content, QStringLiteral("strip_D")); last != nullptr) {
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
    for (const QString& law : laws) {
        auto* button = findItem(content, QStringLiteral("curveButton_") + law);
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

    // The wheel has to reach one beat across the strip, not two: the deepest step is the one a person tries
    // when they want a single beat under the playhead.
    if (auto* viewA = findItem(content, QStringLiteral("deckDetail_A")); viewA != nullptr) {
        for (int step = 0; step < 16; ++step) {
            QMetaObject::invokeMethod(viewA, "zoomBy", Q_ARG(QVariant, QVariant(2.0)));
        }
        const double deepest = viewA->property("beatsOnScreen").toDouble();
        std::fprintf(stdout, "dj-ui: the wheel reaches %.1f beats on screen\n", deepest);
        if (std::abs(deepest - 1.0) > 1e-6) {
            std::fprintf(stderr, "dj-ui: the deepest zoom shows %.2f beats, wanted one\n", deepest);
            return 1;
        }
        QMetaObject::invokeMethod(viewA, "zoomBy", Q_ARG(QVariant, QVariant(0.000001)));
    }

    // The wheel trims a channel — over the knob and over the meter, because that is where the
    // pointer actually is when a person scrolls. A handler on the strip missed both.
    if (auto* stripA = findItem(content, QStringLiteral("strip_A")); stripA != nullptr) {
        const auto wheelAt = [&](QQuickItem* target, int notches) {
            const QPointF centre =
                target->mapToScene(QPointF(target->width() / 2.0, target->height() / 2.0));
            QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(),
                              QPoint(0, 120 * notches), Qt::NoButton, Qt::NoModifier,
                              Qt::NoScrollPhase, false);
            QCoreApplication::sendEvent(window, &event);
            QCoreApplication::processEvents();
            return stripA->property("lastWheelFader").toDouble();
        };
        auto* knobA = findItem(content, QStringLiteral("fader_A"));
        auto* meterA = findItem(content, QStringLiteral("meter_A"));
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
// The collection's sort: a click on a column name sorts by it, a second click turns it around, and
// a Shift click adds the next key. And the rail: the collection at its top, the files at its
// bottom.

int runTransportCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, UiSettings& settings,
                      QCommandLineParser& parser, const QCommandLineOption& loadOption,
                      const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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

    const auto fieldOf = [&](const char* field) {
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                continue;
            }
            return map.value(QString::fromLatin1(field)).toDouble();
        }
        return -1.0;
    };
    const auto playing = [&]() {
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
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
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
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
    const auto press = [&](QQuickItem* item) {
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
    const auto release = [&](QQuickItem* item) {
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
    const double totalFrames = fieldOf("source_frame") + fieldOf("remaining_frames");
    auto* playButton = findItem(window->contentItem(), QStringLiteral("play_A"));
    auto* cueButton = findItem(window->contentItem(), QStringLiteral("cue_A"));
    auto* overview = findItem(window->contentItem(), QStringLiteral("deckOverview_A"));
    auto* clock = findItem(window->contentItem(), QStringLiteral("clock_A"));
    if (playButton == nullptr || cueButton == nullptr || overview == nullptr || clock == nullptr) {
        std::fprintf(stderr,
                     "dj-ui: the deck is missing its transport, its clock or its overview\n");
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
    const QPointF from =
        overview->mapToScene(QPointF(overview->width() * 0.2, overview->height() / 2));
    const QPointF to =
        overview->mapToScene(QPointF(overview->width() * 0.6, overview->height() / 2));
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
    std::fprintf(stdout,
                 "dj-ui: scrubbing to sixty per cent of the overview put the playhead at "
                 "%.0f of %.0f frames\n",
                 scrubbed, total);
    if (total <= 0.0 || scrubbed < total * 0.45 || scrubbed > total * 0.75) {
        std::fprintf(stderr, "dj-ui: the scrub did not move the playhead where the hand went\n");
        return 1;
    }

    // Scrubbing by hand on the zoomed strip: dragging to the right takes the material back, and in
    // the default silent mode the transport is left alone.
    {
        auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        if (strip == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no waveform strip\n");
            return 1;
        }
        const bool wasPlaying = playing();
        const double before = fieldOf("source_frame");
        const QPointF grab =
            strip->mapToScene(QPointF(strip->width() * 0.5, strip->height() * 0.5));
        const QPointF moved = grab + QPointF(60, 0);
        for (const auto& [type, point] :
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
        auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        if (strip == nullptr) {
            std::fprintf(stderr, "dj-ui: deck A has no strip\n");
            return 1;
        }
        // The mode is a setting now, and the check asks the settings for it — which is the way the
        // DJ gets it too (owner's decision, 14.11: no button on the deck).
        settings.setScrubAudible(true);
        if (!settings.scrubAudible()) {
            std::fprintf(stderr, "dj-ui: the scrub mode will not switch to audible\n");
            return 1;
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
            std::fprintf(stderr,
                         "dj-ui: releasing the hand did not leave the deck where it stood\n");
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

int runPitchCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto tempoOf = [&](const char* deck) {
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() == QString::fromLatin1(deck)) {
                return map.value(QStringLiteral("tempo_ratio")).toDouble();
            }
        }
        return -1.0;
    };
    auto* pitch = findItem(window->contentItem(), QStringLiteral("pitch_A"));
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
                for (const QVariant& entry :
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
        return pitch->mapToScene(QPointF(pitch->width() / 2.0, pitch->height() / 2.0));
    };
    const QPointF grab = grabPoint();
    const auto send = [&](QEvent::Type type, const QPointF& at) {
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
        std::fprintf(stdout, "dj-ui: the pitch throw is ±%.1f %% (%.3f … %.3f)\n", rangePercent,
                     minimum, maximum);
        if (std::abs(rangePercent - 16.0) > 1e-6 || std::abs(minimum - 0.84) > 1e-6 ||
            std::abs(maximum - 1.16) > 1e-6) {
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
                 "%.4f (deck B at %.3f)\n",
                 asked, playing, other);
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
            for (const QVariant& entry :
                 bridge->snapshot().value(QStringLiteral("decks")).toList()) {
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
        // From a known place, far enough from the end: a deck that runs out of clip half way
        // through the window stops dead, and the average then reads exactly like a deck running
        // slow. Left over from an earlier check, a short clip can be a second from its end.
        double remaining = 0.0;
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                remaining = map.value(QStringLiteral("remaining_frames")).toDouble();
            }
        }
        if (remaining < 48000.0 * 4) {
            bridge->seek(QStringLiteral("A"), 0);
            pump(40);
        }
        const double before = frameOf();
        QElapsedTimer start;
        start.start();
        while (start.elapsed() < 4000 && !(frameOf() > before + 100.0)) {
            pump(2);
        }
        if (!(frameOf() > before + 100.0)) {
            QString state;
            for (const QVariant& entry :
                 bridge->snapshot().value(QStringLiteral("decks")).toList()) {
                const QVariantMap map = entry.toMap();
                if (map.value(QStringLiteral("deck")).toString() != QStringLiteral("A")) {
                    continue;
                }
                state =
                    QStringLiteral("load %1, playing %2, frame %3, remaining %4")
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
        // Long enough that the snapshot's 20 Hz publishing is a small part of the window. Sampled
        // in slices: a deck that stalls for part of the window and a deck that runs slow show the
        // same average, and only the series tells them apart.
        QVector<double> series;
        for (int slice = 0; slice < 12; ++slice) {
            pump(20);
            series.append(frameOf());
        }
        const double elapsed = window.elapsed() / 1000.0;
        const double to = frameOf();
        bridge->play(QStringLiteral("A"), false);
        // The clip that is on the deck, not the one this check assumed: a file at another rate
        // moves a different number of source frames for the same second of sound.
        qint64 meterRate = 0;
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                meterRate = map.value(QStringLiteral("source_rate")).toLongLong();
            }
        }
        // Against the rate the deck actually holds: the window is wall clock, the material is
        // frames of that file, and a 44,1 kHz clip would read a tenth fast at a hard-coded 48 000.
        const double meter = meterRate > 0 ? static_cast<double>(meterRate) : 48000.0;
        const double heard = (to - from) / (elapsed * meter);
        std::fprintf(stdout,
                     "dj-ui: the deck consumed %.3f seconds of the file per second at %.4f "
                     "(elapsed %.3f s, source rate %lld)\n",
                     heard, playing, elapsed, static_cast<long long>(meterRate));
        if (elapsed < 0.5 || std::abs(heard - playing) > 0.06) {
            QString walked;
            for (const double sample : series) {
                walked += QStringLiteral("%1 ").arg(sample - from, 0, 'f', 0);
            }
            std::fprintf(stderr, "dj-ui: the window walked %s\n", qPrintable(walked));
            std::fprintf(stderr,
                         "dj-ui: the fader says %.3f but the material moved at %.3f "
                         "(elapsed %.3f s, source rate %lld, %lld frames)\n",
                         playing, heard, elapsed, static_cast<long long>(meterRate),
                         static_cast<long long>(to - from));
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
        QQuickItem* handle = findItem(window->contentItem(), QStringLiteral("pitchHandle_A"));
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

int runGridCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                 const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
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
    const auto tempoNow = [&]() { return deckA().value(QStringLiteral("local_bpm")).toDouble(); };
    const auto click = [&](const QString& name) {
        auto* item = findItem(window->contentItem(), name);
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
        std::fprintf(stderr,
                     "dj-ui: the shift did not move the grid by ten milliseconds (%lld -> %lld)\n",
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
    // A beat a press: one period of the tempo under the play head, and back again. A grid that
    // starts on the wrong beat needs a beat, not the ten milliseconds of the arrows.
    const qint64 beat = std::llround(double(rate) * 60.0 / tempo);
    if (!click(QStringLiteral("gridBeatRight_A"))) {
        return 1;
    }
    if (!waitFor([&]() { return firstBeat() == start + beat; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the beat shift moved the grid to %lld instead of %lld\n",
                     static_cast<long long>(firstBeat()), static_cast<long long>(start + beat));
        return 1;
    }
    // Размер такта: замер отдаёт четыре, DJ ставит три и возвращает обратно. Счёт такта на деке
    // читает то же поле снапшота, так что проверка смотрит ровно то, что видит глаз.
    const auto metreNow = [&]() {
        return deckA()
            .value(QStringLiteral("track"))
            .toMap()
            .value(QStringLiteral("grid"))
            .toMap()
            .value(QStringLiteral("beats_per_bar"))
            .toInt();
    };
    const int metreBefore = metreNow();
    // Меню размера встаёт у кнопки: владелец находил его в левом верхнем углу окна, потому что
    // всплывающее окно без места открывается в начало координат. Само окно стенду недоступно, но
    // панель отдаёт его координаты — они и сравниваются с положением кнопки.
    if (QQuickItem* metreButton = findItem(window->contentItem(), QStringLiteral("gridMetre_A"))) {
        // A button the DJ cannot see cannot be pressed: the menu is measured against it, so its own
        // visibility is part of the premise.
        if (!metreButton->isVisible()) {
            std::fprintf(stderr, "dj-ui: the metre button is in the scene but not visible\n");
            return 1;
        }
        if (!click(QStringLiteral("gridMetre_A"))) {
            return 1;
        }
        // Дать меню раскрыться: waitFor ждёт всё окно, когда условие не выполняется.
        (void)waitFor([&]() { return false; }, 120);
        QQuickItem* deckPanel = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
        if (deckPanel == nullptr) {
            std::fprintf(stderr, "dj-ui: the deck panel is not there to ask about the menu\n");
            return 1;
        }
        QVariant at;
        if (!QMetaObject::invokeMethod(deckPanel, "metreMenuAt", Q_RETURN_ARG(QVariant, at))) {
            std::fprintf(stderr, "dj-ui: the panel does not say where the metre menu is\n");
            return 1;
        }
        const QVariantMap atMap = at.toMap();
        const double menuX = atMap.value(QStringLiteral("x")).toDouble();
        const double menuY = atMap.value(QStringLiteral("y")).toDouble();
        // Место считается от самой кнопки: левый верх меню — на её левом краю, верх — на её низу.
        // Высоту кнопки отдаёт QML: там же, где живёт само меню, — иначе сравниваются два разных
        // представления об одном предмете.
        const double wantX = 0.0;
        const double wantY = atMap.value(QStringLiteral("buttonHeight")).toDouble();
        // Вплотную: меню — сосед кнопки, а не что-то «примерно там же». Два пикселя на рамку
        // всплывающего окна, не больше.
        if (std::abs(menuX - wantX) > 2.0 || std::abs(menuY - wantY) > 2.0) {
            std::fprintf(
                stderr,
                "dj-ui: the metre menu opened at %.0f,%.0f instead of the button at %.0f,%.0f\n",
                menuX, menuY, wantX, wantY);
            return 1;
        }
        std::fprintf(stderr,
                     "dj-ui: the metre menu sits at %.0f,%.0f under the button at %.0f,%.0f\n",
                     menuX, menuY, wantX, wantY);
    }

    if (bridge->setGridMetre(QStringLiteral("A"), 3) < 0) {
        std::fprintf(stderr, "dj-ui: the metre change was refused\n");
        return 1;
    }
    if (!waitFor([&]() { return metreNow() == 3; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the bar reads %d instead of 3\n", metreNow());
        return 1;
    }
    if (bridge->setGridMetre(QStringLiteral("A"), metreBefore) < 0 ||
        !waitFor([&]() { return metreNow() == metreBefore; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the bar did not come back to %d\n", metreBefore);
        return 1;
    }

    // Инструменты переменной сетки: разбить участок, задать ему темп, убрать его. Команды те же,
    // что шлют кнопки ряда и диалог BPM; кадр берётся из сетки, а не из головы, — этой проверке
    // нужен участок, а не позиция транспорта.
    const int segmentsBefore = int(gridOf().size());
    const qint64 deeper = start + beat * 64;
    if (bridge->splitGrid(QStringLiteral("A"), deeper) < 0) {
        std::fprintf(stderr, "dj-ui: the split was refused\n");
        return 1;
    }
    if (!waitFor([&]() { return int(gridOf().size()) == segmentsBefore + 1; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the split left %d stretches instead of %d\n",
                     int(gridOf().size()), segmentsBefore + 1);
        return 1;
    }
    // Темп участка: та же функция, которую зовёт диалог BPM и стрелки по 0,1.
    if (bridge->setGridStretchTempo(QStringLiteral("A"), deeper, 101.0) < 0) {
        std::fprintf(stderr, "dj-ui: the stretch tempo was refused\n");
        return 1;
    }
    const auto stretchTempo = [&](int index) {
        const QVariantList segments = gridOf();
        if (index >= segments.size()) {
            return 0.0;
        }
        return segments.at(index).toMap().value(QStringLiteral("bpm")).toDouble();
    };
    if (!waitFor([&]() { return std::fabs(stretchTempo(1) - 101.0) < 0.6; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the second stretch reads %.2f BPM instead of 101\n",
                     stretchTempo(1));
        return 1;
    }
    if (!waitFor([&]() { return std::fabs(stretchTempo(0) - tempo) < 0.6; }, 5000)) {
        std::fprintf(stderr, "dj-ui: the first stretch moved to %.2f BPM\n", stretchTempo(0));
        return 1;
    }
    if (bridge->dropGridStretch(QStringLiteral("A"), deeper) < 0) {
        std::fprintf(stderr, "dj-ui: the drop was refused\n");
        return 1;
    }
    if (!waitFor([&]() { return int(gridOf().size()) == segmentsBefore; }, 5000)) {
        std::fprintf(stderr, "dj-ui: dropping the stretch left %d stretches instead of %d\n",
                     int(gridOf().size()), segmentsBefore);
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
    auto* field = findItem(window->contentItem(), QStringLiteral("tempoField_A"));
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
    const QString sidecar = QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
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
                 tempo,
                 static_cast<long long>(
                     grid.value(QStringLiteral("first_beat_frame")).toVariant().toLongLong()),
                 stored().value(QStringLiteral("revision")).toInt());
    // The rows under the wave are part of what this check promises: the strip folds, and folding it
    // gives the wave its height back.
    if (const int rows = measureDeckRows(window, QStringLiteral("A")); rows != 0) {
        return rows;
    }
    return 0;
}

int runLoopBeatsCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                      QCommandLineParser& parser, const QCommandLineOption& loadOption,
                      const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
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
    const auto loopRegion = [&]() { return deckA().value(QStringLiteral("loop_region")).toMap(); };
    // A Rectangle with a MouseArea has no `clicked` signal: the press is a mouse event at its
    // centre.
    const auto clickAt = [](QQuickItem* item) {
        const QPointF centre = item->mapToScene(QPointF(item->width() / 2.0, item->height() / 2.0));
        QMouseEvent press(QEvent::MouseButtonPress, centre, centre, Qt::LeftButton, Qt::LeftButton,
                          Qt::NoModifier);
        QMouseEvent release(QEvent::MouseButtonRelease, centre, centre, Qt::LeftButton,
                            Qt::NoButton, Qt::NoModifier);
        QCoreApplication::sendEvent(item->window(), &press);
        QCoreApplication::sendEvent(item->window(), &release);
        QCoreApplication::processEvents();
    };
    const auto click = [&](const QString& name) {
        auto* item = findItem(window->contentItem(), name);
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
        return waitFor(
            [&]() {
                return std::llabs(deckA().value(QStringLiteral("source_frame")).toLongLong() -
                                  frame) <= 2;
            },
            5000);
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
    auto* panelItem = findItem(window->contentItem(), QStringLiteral("deckPanel_A"));
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
        std::fprintf(stderr,
                     "dj-ui: the loop edges are off the grid: %lld..%lld (anchor %lld, "
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
    // The sizes and the switches live in the strip that folds, so the DJ opens it first: the check
    // does the same thing (owner's decision, 14.11).
    if (!click(QStringLiteral("loopJumpToggle_A"))) {
        return 1;
    }
    // The layout needs a frame before the strip's own buttons can be hit.
    for (int tick = 0; tick < 40; ++tick) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
        QThread::msleep(10);
    }
    // The switches are the DJ's: turned off, the very same pair must land where the hand was and
    // not on the grid. A switch that only looks lit is worse than none. The switches are global, in
    // the mixer: the modes are one decision for the set, and the decks read them (owner's
    // decision, 14.11).
    auto* snapSwitch = findItem(window->contentItem(), QStringLiteral("snapGlobal"));
    auto* quantSwitch = findItem(window->contentItem(), QStringLiteral("quantizeGlobal"));
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
        return region.value(QStringLiteral("end")).toLongLong() -
               region.value(QStringLiteral("start")).toLongLong();
    };
    if (!waitFor([&]() { return std::llabs(loopLength() - four) <= 2; }, 5000)) {
        std::fprintf(
            stderr, "dj-ui: a four beat loop came back %lld frames long, wanted %lld (start %s)\n",
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
                const qint64 moved =
                    loopRegion().value(QStringLiteral("start")).toLongLong() - start;
                return std::llabs(moved - beatStep) <= 2 && std::llabs(loopLength() - four) <= 2;
            },
            5000)) {
        std::fprintf(stderr, "dj-ui: the arrow moved the loop to frame %s, and it is %lld long\n",
                     qPrintable(loopRegion().value(QStringLiteral("start")).toString()),
                     loopLength());
        return 1;
    }
    // Every loop is the same loop: the one the size button made has to light the Loop button, and a
    // press inside it has to put the mode out — that is how a DJ leaves a loop.
    auto* loopButton = findItem(window->contentItem(), QStringLiteral("loop_A"));
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
                return !deckA().value(QStringLiteral("loop_mode")).toBool() &&
                       !loopButton->property("active").toBool();
            },
            5000)) {
        std::fprintf(stderr, "dj-ui: a press on the lit Loop button left the mode on\n");
        return 1;
    }
    std::fprintf(
        stdout,
        "dj-ui: a four beat loop is %lld frames of the grid (a beat is %.1f), it starts on a "
        "beat at frame %lld, and one press of the arrow moved it %.1f frames without "
        "changing its length; two presses off the grid snapped to %lld..%lld (%lld whole "
        "beats), with the switches off the same pair landed on %lld (%s the grid), the size "
        "loop lit the Loop button, and a press inside it put the mode out\n",
        four, period, start, static_cast<double>(beatStep), snappedStart, snappedEnd, snappedBeats,
        rawStart, onGrid(rawStart) ? "on" : "off");
    return 0;
}

int runGridCreateCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                       QCommandLineParser& parser, const QCommandLineOption& loadOption,
                       const QCommandLineOption& loadSecondOption,
                       const QCommandLineOption& connectOption) {
    // A track whose tempo could not be measured has no grid — and that is exactly the track whose
    // grid a person has to place. The tools have to be there, the ones that read a grid have to be
    // grey, and the two that make one have to work: a beat placed at the play head starts a grid
    // there, a typed tempo starts one at the first frame (DJP-MEDIA-006).
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
    const auto deckA = [&]() { return deckById(bridge->snapshot(), QStringLiteral("A")); };
    const auto gridOf = [&]() {
        return deckA().value(QStringLiteral("track")).toMap().value(QStringLiteral("grid")).toMap();
    };
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
    const auto itemByName = [&](const QString& name) {
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
        std::fprintf(stderr,
                     "dj-ui: %s measured a grid of its own; this check needs a track "
                     "with none\n",
                     qPrintable(wantName));
        return 1;
    }

    // The row is there, the readers are grey, the makers are live.
    auto* row = itemByName(QStringLiteral("gridedit_A"));
    if (row == nullptr || !row->isVisible()) {
        std::fprintf(stderr, "dj-ui: the grid tools are hidden on a track with no grid\n");
        return 1;
    }
    struct Expect {
        const char* name;
        bool enabled;
    };
    for (const Expect& expect :
         {Expect{"gridHere_A", true}, Expect{"gridTempo_A", true}, Expect{"gridTap_A", true},
          Expect{"gridRight_A", false}, Expect{"gridHalf_A", false}, Expect{"gridDouble_A", false},
          Expect{"metronome_A", false}}) {
        auto* item = itemByName(QString::fromLatin1(expect.name));
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
                         expect.name, enabled ? "live" : "grey", expect.enabled ? "live" : "grey");
            return 1;
        }
    }
    std::fprintf(stdout, "dj-ui: on a track with no grid the tools are there, the readers grey\n");

    // A beat placed at the play head: the grid starts there, at the tempo a deck shows when nothing
    // has been measured.
    constexpr qint64 target = 48'000;
    bridge->seek(QStringLiteral("A"), static_cast<qulonglong>(target));
    waitFor([&]() { return deckA().value(QStringLiteral("source_frame")).toLongLong() == target; },
            5000);
    QMetaObject::invokeMethod(itemByName(QStringLiteral("gridHere_A")), "clicked");
    const auto placed = [&]() { return !gridOf().isEmpty() && firstBeat() == target; };
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
    // to, and the reading a DJ then shifts onto the first kick is the one that starts at zero. It
    // is measured on deck B, whose file has no grid: deck A has one now, and retiming an existing
    // grid keeps its first beat — a different rule, and the one that would hide this one.
    const QString second = parser.value(loadSecondOption);
    if (second.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --grid-create-check needs --load-b, a second track with no "
                             "grid\n");
        return 1;
    }
    const auto deckB = [&]() { return deckById(bridge->snapshot(), QStringLiteral("B")); };
    const auto gridOfB = [&]() {
        return deckB().value(QStringLiteral("track")).toMap().value(QStringLiteral("grid")).toMap();
    };
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
        std::fprintf(stderr,
                     "dj-ui: %s measured a grid of its own; this check needs a track "
                     "with none\n",
                     qPrintable(secondName));
        return 1;
    }
    bridge->setGridTempo(QStringLiteral("B"), 174.0);
    const auto retimed = [&]() {
        return !gridOfB().isEmpty() && std::abs(tempoOfB() - 174.0) < 0.01 && firstBeatB() == 0;
    };
    if (!waitFor(retimed, 5000)) {
        std::fprintf(stderr, "dj-ui: a typed tempo left the grid at %.2f BPM from frame %lld\n",
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
        std::fprintf(stderr, "dj-ui: a press down left the tempo at %.3f, not 174.1\n", tempoOfB());
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
    auto* editButton = itemByName(QStringLiteral("gridTempo_A"));
    if (editButton == nullptr) {
        std::fprintf(stderr, "dj-ui: no tempo field button on deck A\n");
        return 1;
    }
    QMetaObject::invokeMethod(editButton, "clicked");
    QCoreApplication::processEvents();
    auto* field = itemByName(QStringLiteral("tempoField_A"));
    if (field == nullptr) {
        std::fprintf(stderr, "dj-ui: the tempo field never appeared\n");
        return 1;
    }
    // Typed, not assigned: a comma that survives `setProperty` may still never arrive from a
    // keyboard, and the keyboard is what the DJ uses. The key events go to the focused item through
    // the window, exactly as a real press does.
    const auto typeInto = [&](const QString& text) {
        field->forceActiveFocus();
        field->setProperty("text", QString());
        QCoreApplication::processEvents();
        for (const QChar& character : text) {
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
    for (const QString& typed : {QStringLiteral("174.5"), QStringLiteral("174,5")}) {
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
    for (const QString& typed : {QStringLiteral("174.5.5"), QStringLiteral("174,5,5")}) {
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
    auto* label = itemByName(QStringLiteral("tempoEditLabel_A"));
    if (label == nullptr) {
        std::fprintf(stderr, "dj-ui: the tempo dialog has no label\n");
        return 1;
    }
    const auto rightEdge = [](QQuickItem* item) {
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
        std::fprintf(stderr,
                     "dj-ui: the tempo dialog label is %.0f px wide, wider than the "
                     "popup\n",
                     label->width());
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the tempo dialog keeps its content inside itself\n");
    std::fprintf(stdout, "dj-ui: a track with no grid can be given one by hand\n");
    return 0;
}

int runFourDeckCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                     QCommandLineParser& parser, const QCommandLineOption& loadOption,
                     const QCommandLineOption& connectOption) {
    // Four decks are a claim about the whole chain: four deck objects, four mixer channels, four
    // strips on screen, and a crossfader that knows which side each one is on. This check walks the
    // chain once, from the file to the master.
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    bridge->connectToEngine(parser.value(connectOption));
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
    const auto deckOf = [&](const QString& id) {
        const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == id) {
                return deck;
            }
        }
        return QVariantMap();
    };
    const auto frameOf = [&](const QString& id) {
        return deckOf(id).value(QStringLiteral("source_frame")).toLongLong();
    };
    const auto peakOf = [&](const QString& id) {
        // A deck's meter is a peak and a hold. The peak lasts one block and a sample of it can land
        // in the gap between two beats; the hold is what the strip says a moment later, and it is
        // the honest answer to "did this deck make a sound in this window".
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
    const auto loudestOver = [&](const std::function<double()>& read, int budget_ms) {
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
    for (const QString& id : ids) {
        bridge->load(id, path);
    }
    const bool loaded = waitFor(
        [&]() {
            for (const QString& id : ids) {
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
    // Four decks reading the same bytes from the same place would prove nothing about four decks,
    // and an eight-second fixture runs out from under four windows of measurement: each deck gets a
    // loop of its own half of the track, a quarter of it further in than the one before. The
    // material never ends, and no two decks are reading the same place.
    const qint64 length = deckOf(QStringLiteral("A"))
                              .value(QStringLiteral("track"))
                              .toMap()
                              .value(QStringLiteral("duration_frames"))
                              .toLongLong();
    if (length <= 0) {
        std::fprintf(stderr, "dj-ui: the four-deck check has no length to loop over\n");
        return 1;
    }
    for (int index = 0; index < ids.size(); ++index) {
        // Every window ends inside the file, never on its last frame: a loop that ends exactly on
        // the end of its file goes silent on the wrap (evidence in
        // docs/qa/evidence/A07/four-decks.md), and this check is about the mixer carrying four
        // decks, not about that defect. It is recorded there and waits for its own test.
        const qint64 start = length / 8 + length / 8 * index;
        const qint64 end = std::min(start + length / 5, length - length / 8);
        bridge->setLoop(ids[index], true, static_cast<qulonglong>(start),
                        static_cast<qulonglong>(end));
    }
    for (const QString& id : ids) {
        bridge->play(id, true);
    }
    if (!waitFor(
            [&]() {
                for (const QString& id : ids) {
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
    for (const QString& id : ids) {
        before.append(frameOf(id));
    }
    QThread::msleep(1500);
    QCoreApplication::processEvents(QEventLoop::AllEvents, 100);
    bool all_moved = true;
    for (int index = 0; index < ids.size(); ++index) {
        const qint64 moved = frameOf(ids[index]) - before[index];
        std::fprintf(stdout, "dj-ui: deck %s moved %lld frames in 1,5 s\n", qPrintable(ids[index]),
                     static_cast<long long>(moved));
        if (moved < 24000) { // half a second of material: a deck that plays is not one that crawls
            all_moved = false;
        }
    }
    if (!all_moved) {
        std::fprintf(stderr, "dj-ui: a deck of four is not moving\n");
        return 1;
    }

    // 2. Every deck makes its own sound, and all of it reaches the master.
    QVector<double> peaks;
    for (const QString& id : ids) {
        peaks.append(loudestOver([&]() { return peakOf(id); }, 2000));
        std::fprintf(stdout, "dj-ui: deck %s peaks at %.4f\n", qPrintable(id), peaks.last());
    }
    for (int index = 0; index < ids.size(); ++index) {
        if (peaks[index] <= 0.01) {
            std::fprintf(stderr, "dj-ui: deck %s says nothing (peak %.4f)\n",
                         qPrintable(ids[index]), peaks[index]);
            return 1;
        }
    }

    // 3. The decks are four voices of one mix, and the crossfader keeps each pair on its own side.
    // Measuring one deck alone turned out to be a measure of the instrument rather than of the mix
    // — a deck soloed through three muted channels read zero while the same two decks together read
    // half scale — so the check asks its question in pairs: shut two, and the other two must still
    // reach the master. Which side each deck is on is a unit test of its own
    // (`the_crossfader_holds_two_sides_of_four_decks`).
    // A command is a message, not a fact: the mix takes the fader on its own thread, and a
    // measurement started one block too early measures the setting that was there before it. Under
    // a loaded machine that turned the first pair of the run into a zero — the crossfader had not
    // arrived yet.
    const auto settled = [&](const std::function<bool()>& reached) {
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
    // A peak lives one block: a window that saw ten snapshots can miss it, and a window that saw
    // none has measured nothing at all. The count is what tells the two apart.
    const auto loudestWithCount = [&](const std::function<double()>& read, int budget_ms,
                                      int* seen) {
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
    const auto masterWithOnly = [&](const QString& first, const QString& second) {
        for (const QString& id : ids) {
            bridge->setChannelFader(id, id == first || id == second ? 1.0 : 0.0);
        }
        const auto open = [&]() {
            for (const QString& id : ids) {
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
                std::fprintf(
                    stderr,
                    "dj-ui: the machine gave %d snapshots in 1,2 s: the pair %s and %s was "
                    "not measured\n",
                    seen, qPrintable(first), qPrintable(second));
                return -1.0;
            }
        }
        for (const QString& id : ids) {
            bridge->setChannelFader(id, 1.0);
        }
        return heard;
    };
    bridge->setCrossfader(0.5);
    if (!settled([&]() {
            return std::abs(bridge->snapshot().value(QStringLiteral("crossfader")).toDouble() -
                            0.5) < 0.01;
        })) {
        std::fprintf(stderr, "dj-ui: the mix never took the crossfader to the centre\n");
        return 1;
    }
    const double only_cd = masterWithOnly(QStringLiteral("C"), QStringLiteral("D"));
    const double only_ab = masterWithOnly(QStringLiteral("A"), QStringLiteral("B"));
    const double only_ac = masterWithOnly(QStringLiteral("A"), QStringLiteral("C"));
    std::fprintf(
        stdout,
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
    for (const QString& id : ids) {
        if (findItem(window->contentItem(), QStringLiteral("deckPanel_") + id) == nullptr) {
            missing.append(id);
        }
    }
    if (findItem(window->contentItem(), QStringLiteral("mixer")) == nullptr) {
        missing.append(QStringLiteral("mixer"));
    }
    if (!missing.isEmpty()) {
        std::fprintf(stderr, "dj-ui: the scene has no panel for %s\n",
                     qPrintable(missing.join(',')));
        return 1;
    }

    for (const QString& id : ids) {
        bridge->play(id, false);
    }
    std::fprintf(
        stdout,
        "dj-ui: four decks play and mix — each one moves, each one is heard on its own, every "
        "pair of them reaches the master, and the scene has a panel for every one of them\n");
    return 0;
}

int runMasterCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, UiSettings& settings,
                   QCommandLineParser& parser, const QCommandLineOption& loadOption,
                   const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    QQuickItem* content = window->contentItem();

    // The window is sized by the session and is not shown in a headless run: give it the real
    // size, or every measurement below is taken against a 0x0 window and lies small.
    window->resize(1366, 768);
    window->show();
    QCoreApplication::processEvents(QEventLoop::AllEvents, 100);

    const auto item = [&](const QString& name) { return findItem(content, name); };
    auto* knob = item(QStringLiteral("masterKnob"));
    auto* meter = item(QStringLiteral("masterMeter"));
    auto* column = item(QStringLiteral("masterColumn"));
    auto* stripA = item(QStringLiteral("strip_A"));
    auto* stripB = item(QStringLiteral("strip_B"));
    auto* meterA = item(QStringLiteral("meter_A"));
    if (knob == nullptr || meter == nullptr || column == nullptr || stripA == nullptr ||
        stripB == nullptr || meterA == nullptr) {
        std::fprintf(stderr, "dj-ui: the master column is not in the mixer\n");
        return 1;
    }
    // Between A and B, not at an end.
    const double columnMid = column->mapToItem(content, QPointF(column->width() / 2.0, 0)).x();
    const double aMid = stripA->mapToItem(content, QPointF(stripA->width() / 2.0, 0)).x();
    const double bMid = stripB->mapToItem(content, QPointF(stripB->width() / 2.0, 0)).x();
    std::fprintf(stdout,
                 "dj-ui: the master column sits at %.0f px, between A at %.0f and B at %.0f\n",
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
    std::fprintf(stdout,
                 "dj-ui: the master meter starts %.0f px below the knob and ends %.0f px "
                 "from the deck meters; heights %.0f and %.0f px\n",
                 meterTop - knobBottom, meterBottom - deckMeterBottom, meter->height(),
                 meterA->height());
    auto* row = item(QStringLiteral("stripRow"));
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
            for (const QVariant& entry : decksNow) {
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
            for (const QVariant& entry : decks) {
                const QVariantMap deck = entry.toMap();
                if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                    const QVariant load = deck.value(QStringLiteral("load"));
                    // serde writes a variant with data as an object with one key, and one without
                    // as a plain string.
                    return load.typeId() == QMetaType::QString ? load.toString()
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
            peak = bridge->snapshot()
                       .value(QStringLiteral("master"))
                       .toMap()
                       .value(QStringLiteral("peak_left"))
                       .toDouble();
            QThread::msleep(20);
        }
        // Where the signal stops, if it stops: the deck's own meter and the transport state say
        // whether the master is silent because nothing plays or because nothing is routed.
        bool playingA = false;
        double deckPeak = 0.0;
        const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                playingA = deck.value(QStringLiteral("playing")).toBool();
                deckPeak = deck.value(QStringLiteral("levels"))
                               .toMap()
                               .value(QStringLiteral("peak_left"))
                               .toDouble();
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
    const QString zoneLow = zoneAt(0.1);  // -20 dB
    const QString zoneAt6 = zoneAt(0.45); // -6.9 dB
    const QString zoneNear = zoneAt(0.6); // -4.4 dB
    const QString zoneFull = zoneAt(1.0); // 0 dB, the top edge
    std::fprintf(stdout, "dj-ui: master meter zones: -20 dB %s, -6.9 dB %s, -4.4 dB %s, 0 dB %s\n",
                 qPrintable(zoneLow), qPrintable(zoneAt6), qPrintable(zoneNear),
                 qPrintable(zoneFull));
    if (zoneLow != QLatin1String("green") || zoneAt6 != QLatin1String("green") ||
        zoneNear != QLatin1String("yellow") || zoneFull != QLatin1String("red")) {
        std::fprintf(stderr, "dj-ui: the master meter colours do not follow the level\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: master meter yellow from %.0f dB, red from %.0f dB, top %.0f dB\n",
                 meter->property("yellowFromDb").toDouble(),
                 meter->property("redFromDb").toDouble(), meter->property("ceilingDb").toDouble());

    // And the picture itself: a gradient runs top-down while the thresholds are measured from the
    // bottom, so the colours can be logical in the properties and upside down on screen. Drive the
    // meter past 0 dB — that is the only place red exists — and read the pixels: red at the top,
    // yellow under it, green at the foot.
    meter->setProperty("peak", QVariant(1.0));
    QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
    const QImage frame = window->grabWindow();
    if (frame.isNull()) {
        std::fprintf(stdout,
                     "dj-ui: the window would not render, so the meter colours are unread\n");
    } else {
        const QPointF origin = meter->mapToItem(content, QPointF(0, 0));
        const double ratio =
            content->width() > 0 ? static_cast<double>(frame.width()) / content->width() : 1.0;
        const auto sample = [&](double downFromTop) {
            const int x = static_cast<int>((origin.x() + meter->width() / 2.0) * ratio);
            const int y = static_cast<int>((origin.y() + downFromTop) * ratio);
            return frame.pixelColor(x, y);
        };
        const double height = meter->height();
        const QColor head = sample(height * 0.03);
        const QColor middle = sample(height * 0.12);
        const QColor foot = sample(height * 0.90);
        std::fprintf(stdout,
                     "dj-ui: at full scale the master meter reads %s at the top, %s under "
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

int runTransportExtrasCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                            QCommandLineParser& parser, const QCommandLineOption& loadOption,
                            const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
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
    const auto press = [&](const QString& name) {
        auto* item = findItem(window->contentItem(), name);
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
    for (const QString& name :
         {QStringLiteral("m64"), QStringLiteral("m32"), QStringLiteral("m16"), QStringLiteral("m8"),
          QStringLiteral("m4"), QStringLiteral("m1"), QStringLiteral("p1"), QStringLiteral("p4"),
          QStringLiteral("p8"), QStringLiteral("p16"), QStringLiteral("p32"),
          QStringLiteral("p64")}) {
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
                return deck.value(QStringLiteral("slip")).toBool() &&
                       deck.value(QStringLiteral("slip_frame")).isValid();
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
    auto* overview = findItem(window->contentItem(), QStringLiteral("overviewStrip_A"));
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
    auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
    if (strip == nullptr || strip->property("slipRatio").toDouble() < 0.0) {
        std::fprintf(stderr, "dj-ui: the main strip was never told where the virtual head is\n");
        return 1;
    }
    if (!press(QStringLiteral("slip_A"))) {
        return 1;
    }
    if (!waitFor([&]() { return !deckA().value(QStringLiteral("slip")).toBool(); }, 3000)) {
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
                return deckA().value(QStringLiteral("cached")).toBool() &&
                       deckA().value(QStringLiteral("cached_bytes")).toDouble() > 0.0;
            },
            20000)) {
        std::fprintf(stderr,
                     "dj-ui: the deck never reported its material in memory (cache off?)\n");
        return 1;
    }
    const double cachedMb =
        deckA().value(QStringLiteral("cached_bytes")).toDouble() / (1024.0 * 1024.0);
    const double budgetMb =
        bridge->snapshot().value(QStringLiteral("cache_budget_bytes")).toDouble() /
        (1024.0 * 1024.0);
    std::fprintf(stdout, "dj-ui: the track is in memory: %.1f MB of the %.0f MB budget (%.1f%%)\n",
                 cachedMb, budgetMb, budgetMb > 0.0 ? 100.0 * cachedMb / budgetMb : 0.0);
    const qint64 beforeReverse = playHead();
    bool accepted = false;
    QElapsedTimer reverseWait;
    reverseWait.start();
    while (reverseWait.elapsed() < 12'000) {
        if (!press(QStringLiteral("reverse_A"))) {
            return 1;
        }
        if (waitFor([&]() { return deckA().value(QStringLiteral("reverse")).toBool(); }, 800)) {
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
    if (!waitFor([&]() { return !deckA().value(QStringLiteral("reverse")).toBool(); }, 3000)) {
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
    if (deckA().value(QStringLiteral("playing")).toBool() && !press(QStringLiteral("play_A"))) {
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

int runPadsCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                 const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
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
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return map;
            }
        }
        return QVariantMap();
    };
    const auto click = [&](QQuickItem* item, Qt::KeyboardModifiers modifiers) {
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
    auto* pad = findItem(window->contentItem(), QStringLiteral("cuepad_A_3"));
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
        for (const QVariant& cue : cuesNow) {
            parts << (cue.isNull() ? QStringLiteral("—") : QString::number(cue.toDouble()));
        }
        std::fprintf(stdout, "dj-ui: after the click the deck's load state is %s\n",
                     qPrintable(QJsonDocument::fromVariant(after.value(QStringLiteral("load")))
                                    .toJson(QJsonDocument::Compact)));
        std::fprintf(stdout, "dj-ui: after the click the pads read [%s] at playhead %.0f\n",
                     qPrintable(parts.join(QStringLiteral(", "))),
                     after.value(QStringLiteral("source_frame")).toDouble());
    }
    const QVariantMap set = deckOf();
    const QVariantList cues = set.value(QStringLiteral("hotcues")).toList();
    const double stored =
        cues.size() > 3 && cues[3].isValid() && !cues[3].isNull() ? cues[3].toDouble() : -1.0;
    std::fprintf(stdout, "dj-ui: clicking an empty pad 3 stored a cue at %.0f\n", stored);
    if (std::abs(stored - 96000.0) > 2000.0) {
        std::fprintf(stderr, "dj-ui: the pad did not store the playhead\n");
        return 1;
    }

    // A name and a colour travel through the engine and come back in the snapshot.
    bridge->setCueMeta(QStringLiteral("A"), 3, QStringLiteral("Drop"), QStringLiteral("#5fb85f"));
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
        auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
        const QVariantList marks =
            strip != nullptr ? strip->property("cues").toList() : QVariantList();
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
    auto* padFour = findItem(window->contentItem(), QStringLiteral("cuepad_A_4"));
    if (padFour == nullptr) {
        std::fprintf(stderr, "dj-ui: deck A has no pad 4\n");
        return 1;
    }
    click(padFour, Qt::ShiftModifier);
    pump(30);
    const QVariantList loopsAfter = deckOf().value(QStringLiteral("pad_loops")).toList();
    const bool loopLeft =
        loopsAfter.size() > 4 && loopsAfter[4].isValid() && !loopsAfter[4].isNull();
    const int armedLeft = int(deckOf().value(QStringLiteral("active_loops")).toList().size());
    std::fprintf(stdout, "dj-ui: shift-click left pad 4 %s loop, %d loop(s) armed\n",
                 loopLeft ? "holding a" : "without a", armedLeft);
    if (loopLeft) {
        std::fprintf(stderr, "dj-ui: shift-click did not clear the loop on the pad\n");
        return 1;
    }
    return 0;
}

int runMetronomeCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                      QCommandLineParser& parser, const QCommandLineOption& loadOption,
                      const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    const QString fixture = parser.value(loadOption);
    if (fixture.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --metronome-check needs --load\n");
        return 1;
    }
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return deck;
            }
        }
        return QVariantMap();
    };
    const auto flag = [&]() { return deckA().value(QStringLiteral("metronome")).toBool(); };
    const auto clicks = [&]() { return deckA().value(QStringLiteral("clicks")).toULongLong(); };
    const auto lit = [&]() {
        auto* button = findItem(window->contentItem(), QStringLiteral("metronome_A"));
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
                return !held.isEmpty() && held.endsWith(wantName) &&
                       !deckA()
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
    auto* button = findItem(window->contentItem(), QStringLiteral("metronome_A"));
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
    // A flag is not a click: the deck has to play, and the engine has to count the clicks it played
    // on the beats it crossed. The count is the one number a check can measure instead of hearing.
    // The engine attaches the file at a block boundary, and the measurement that follows
    // re-attaches it: a deck told to play before that lands is put back at the start and stopped,
    // which is exactly what a metronome that never clicked looked like. So the load has to settle
    // first, and the play has to be seen holding before the clicks are counted.
    const auto settled = [&]() {
        const QVariantMap deck = deckA();
        const QString load = deck.value(QStringLiteral("load")).toString();
        return load.isEmpty() || load == QStringLiteral("Ready");
    };
    if (!waitFor(settled, 20000)) {
        std::fprintf(stderr, "dj-ui: the deck never finished loading for the metronome\n");
        return 1;
    }
    waitFor([&]() { return false; }, 300);
    // The engine attaches a file at a block boundary and a measurement that finishes re-attaches
    // it, which puts the deck back at the start, stopped. A hand would press play again; so does
    // this.
    const auto playing = [&]() { return deckA().value(QStringLiteral("playing")).toBool(); };
    bridge->play(QStringLiteral("A"), true);
    if (!waitFor(playing, 5000)) {
        std::fprintf(stderr, "dj-ui: the deck did not start playing for the metronome\n");
        return 1;
    }
    const auto clicksArrive = [&]() {
        for (int tick = 0; tick < 80; ++tick) {
            if (clicks() >= 2) {
                return true;
            }
            if (!playing()) {
                // A reload landed: start again from where the engine put the deck.
                bridge->play(QStringLiteral("A"), true);
            }
            waitFor([&]() { return false; }, 100);
        }
        return clicks() >= 2;
    };
    if (!clicksArrive()) {
        // Everything a silent metronome could be: the deck's own state, the grid it clicks against,
        // and whether the head is moving at all. A count of zero on its own says nothing.
        const QVariantMap deck = deckA();
        const QVariantMap grid =
            deck.value(QStringLiteral("track")).toMap().value(QStringLiteral("grid")).toMap();
        const QVariantList segments = grid.value(QStringLiteral("segments")).toList();
        const auto segment = segments.isEmpty() ? QVariantMap() : segments.first().toMap();
        std::fprintf(
            stderr,
            "dj-ui: the deck played but the metronome clicked %llu times "
            "(frame %lld playing %d metronome %d bpm %.3f first %lld metre %d segments %lld "
            "segment0 frame %lld beat %lld bpm %.3f)\n",
            static_cast<unsigned long long>(clicks()),
            static_cast<long long>(deck.value(QStringLiteral("source_frame")).toLongLong()),
            int(deck.value(QStringLiteral("playing")).toBool()), int(flag()),
            grid.value(QStringLiteral("bpm")).toDouble(),
            static_cast<long long>(grid.value(QStringLiteral("first_beat_frame")).toLongLong()),
            grid.value(QStringLiteral("beats_per_bar")).toInt(),
            static_cast<long long>(segments.size()),
            static_cast<long long>(segment.value(QStringLiteral("frame")).toLongLong()),
            static_cast<long long>(segment.value(QStringLiteral("beat")).toLongLong()),
            segment.value(QStringLiteral("bpm")).toDouble());
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
    std::fprintf(
        stdout,
        "dj-ui: the metronome clicks the grid: the button lit, the deck played %llu clicks "
        "on its beats, and switching it off stopped them\n",
        played);
    return 0;
}

int runScratchCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                    const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    const QString fixture = parser.value(loadOption);
    if (fixture.isEmpty()) {
        std::fprintf(stderr, "dj-ui: --scratch-check needs --load\n");
        return 1;
    }
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return deck;
            }
        }
        return QVariantMap();
    };
    const auto frame = [&]() { return deckA().value(QStringLiteral("source_frame")).toLongLong(); };
    const auto playing = [&]() { return deckA().value(QStringLiteral("playing")).toBool(); };

    bridge->load(QStringLiteral("A"), fixture);
    if (!waitFor(
            [&]() {
                const QVariantMap deck = deckA();
                return deck.value(QStringLiteral("load")).toString() == QStringLiteral("Ready") &&
                       !deck.value(QStringLiteral("track"))
                            .toMap()
                            .value(QStringLiteral("grid"))
                            .toMap()
                            .value(QStringLiteral("segments"))
                            .toList()
                            .isEmpty();
            },
            20000)) {
        std::fprintf(stderr, "dj-ui: the deck never finished loading for the scratch\n");
        return 1;
    }
    // The deck is shared with the checks that ran before this one, and a track played to its end leaves the play
    // head where nothing follows: a hand that turns forward from there moves no material, and the motor has nothing
    // to carry when the hand lets go. The check puts the head a second into the file first and says where it stands.
    bridge->seek(QStringLiteral("A"), 48'000);
    if (!waitFor([&]() { return std::llabs(frame() - 48'000) < 4'800; }, 3000)) {
        std::fprintf(stderr, "dj-ui: the deck would not stand a second into the file for the scratch (%lld)\n",
                     static_cast<long long>(frame()));
        return 1;
    }

    bridge->play(QStringLiteral("A"), true);
    if (!waitFor(playing, 5000)) {
        std::fprintf(stderr, "dj-ui: the deck did not start playing for the scratch\n");
        return 1;
    }
    (void)waitFor([&]() { return frame() > 0; }, 3000);

    auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
    if (strip == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck has no waveform strip to take hold of\n");
        return 1;
    }
    const auto send = [&](QEvent::Type type, const QPointF& at) {
        // A move carries the button that is *held*, not the one that was pressed: a move with no
        // button held is not a drag, and the hand lets go of the record in the middle of it. A
        // press and a release both name the left button; only a move carries none. `buttons` is
        // what is held *after* the event, so a release has none.
        QMouseEvent event(type, at, window->mapToGlobal(at.toPoint()),
                          type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton,
                          type == QEvent::MouseButtonRelease ? Qt::NoButton : Qt::LeftButton,
                          Qt::NoModifier);
        QCoreApplication::sendEvent(window, &event);
        QCoreApplication::processEvents();
    };
    const QPointF at = strip->mapToScene(QPointF(strip->width() * 0.5, strip->height() * 0.5));

    // A hand on the record stops the motor: the record's speed is the hand's from here on. The hand
    // is taken by the first move, not by the press — a click on the wave is only a click.
    send(QEvent::MouseButtonPress, at);
    send(QEvent::MouseMove, at - QPointF(2.0, 0.0));
    if (!waitFor([&]() { return !playing(); }, 3000)) {
        std::fprintf(stderr, "dj-ui: taking the record in hand did not stop the transport\n");
        return 1;
    }
    const qint64 grabbed = frame();

    // Twelve moves to the left: the material goes forward, as the picture says it does.
    for (int step = 1; step <= 12; ++step) {
        send(QEvent::MouseMove, at - QPointF(10.0 * step, 0.0));
        (void)waitFor([&]() { return false; }, 15);
    }
    // The position read here is the engine's, published some twenty times a second: a drag of two hundred
    // milliseconds can finish between two snapshots, and reading the older one says the hand moved nothing
    // (measured, 22.11 — the check accused the hand of a fault that was a stale picture). Ask until a snapshot
    // carries the drag, then measure it.
    if (!waitFor([&]() { return frame() != grabbed; }, 1000)) {
        std::fprintf(stderr, "dj-ui: the hand moved left but the material went nowhere (%lld)\n",
                     static_cast<long long>(grabbed));
        return 1;
    }
    const qint64 moved = frame();
    if (moved <= grabbed) {
        std::fprintf(stderr,
                     "dj-ui: the hand moved left but the material went backwards (%lld -> %lld)\n",
                     static_cast<long long>(grabbed), static_cast<long long>(moved));
        return 1;
    }
    // A hand, not a jump: 120 px of a zoomed strip is a fraction of a second of material, and the
    // whole file is eight.
    const qint64 travelled = moved - grabbed;
    if (travelled > 48'000 * 2) {
        std::fprintf(stderr, "dj-ui: the drag jumped %lld frames instead of moving the material\n",
                     static_cast<long long>(travelled));
        return 1;
    }

    // A hand that stops moving comes to rest: the record has the speed it was last given until the hand's silence
    // reaches the deck, and then it stands still. The shell's idle timer is twenty milliseconds and the deck's own
    // silence is a quarter of a second, so waiting a fixed two hundred and hoping was a coin toss on a busy machine
    // (three different messages out of one cause, 22.11). The check waits for the rest to arrive — at most a second,
    // which is four times the contract — and then requires it to last.
    qint64 rested = frame();
    bool cameToRest = false;
    QElapsedTimer silence;
    silence.start();
    while (silence.elapsed() < 1000) {
        (void)waitFor([&]() { return false; }, 100);
        const qint64 now = frame();
        if (now == rested) {
            cameToRest = true;
            break;
        }
        rested = now;
    }
    if (!cameToRest) {
        std::fprintf(stderr, "dj-ui: a still hand kept moving the record (%lld after a second of silence)\n",
                     static_cast<long long>(rested));
        return 1;
    }
    (void)waitFor([&]() { return false; }, 300);
    if (frame() != rested) {
        std::fprintf(stderr, "dj-ui: a still hand kept moving the record (%lld -> %lld)\n",
                     static_cast<long long>(rested), static_cast<long long>(frame()));
        return 1;
    }

    // Letting go gives the record back to the motor, from where the hand left it.
    send(QEvent::MouseButtonRelease, at - QPointF(120.0, 0.0));
    if (!waitFor(playing, 3000)) {
        std::fprintf(stderr,
                     "dj-ui: the motor did not take the record back after the hand let go\n");
        return 1;
    }
    if (frame() < moved) {
        std::fprintf(stderr, "dj-ui: the motor carried the record backwards (%lld -> %lld)\n",
                     static_cast<long long>(moved), static_cast<long long>(frame()));
        return 1;
    }
    // The motor, not the hand: after the hand lets go the deck has to carry the material at the
    // file's own speed, which is the one thing that says the hand is really off the record.
    (void)waitFor([&]() { return false; }, 100);
    const qint64 carriedAt = frame();
    const double rate = deckA().value(QStringLiteral("source_rate")).toDouble();
    // The release fade is the deck's own business and takes a few blocks: the measurement starts once the record is
    // actually moving again, so a slow start is not read as a slow motor.
    if (!waitFor([&]() { return frame() > carriedAt + static_cast<qint64>(rate * 0.02); }, 1000)) {
        std::fprintf(stderr, "dj-ui: the record did not start moving again after the hand let go\n");
        return 1;
    }
    (void)waitFor([&]() { return false; }, 400);
    const qint64 carriedTo = frame();
    const double expected = rate * 0.4;
    const double actual = static_cast<double>(carriedTo - carriedAt);
    if (std::abs(actual - expected) > expected * 0.35) {
        std::fprintf(
            stderr, "dj-ui: after the hand let go the material moved at %.2f of the file's speed\n",
            actual / expected);
        return 1;
    }
    std::fprintf(
        stdout,
        "dj-ui: the waveform is the record: the hand stopped the motor, moved the material "
        "%lld frames to the left, a still hand moved nothing, and letting go handed it back "
        "to the motor\n",
        static_cast<long long>(travelled));
    return 0;
}

int runTempoRecheckCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                         QCommandLineParser& parser, const QCommandLineOption& loadOption,
                         const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
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
                const QVariant tempo =
                    deckA().value(QStringLiteral("track")).toMap().value(QStringLiteral("bpm"));
                return tempo.isValid() && !tempo.isNull();
            },
            20000)) {
        std::fprintf(stderr, "dj-ui: the engine measured no tempo to re-measure\n");
        return 1;
    }
    const QString sidecar = QFileInfo(fixture).absolutePath() + QStringLiteral("/.djmeta/") +
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
    auto* button = findItem(window->contentItem(), QStringLiteral("tempo_A"));
    if (button == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck has no re-measure button\n");
        return 1;
    }
    auto* confirm = findItem(window->contentItem(), QStringLiteral("tempoGo_A"));
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
    auto* strip = findItem(window->contentItem(), QStringLiteral("bpm_A"));
    // The question must fit in its window: a sentence that runs off the edge is a sentence nobody
    // finishes reading. Wrapping is what makes the window narrow instead of as wide as the text.
    auto* question = findItem(window->contentItem(), QStringLiteral("tempoText_A"));
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
    auto* badge = findItem(window->contentItem(), QStringLiteral("analyzing_A"));
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
    // two after the file does — waiting for it is the difference between a slow screen and a stale
    // one.
    waitFor([&]() { return strip != nullptr && strip->property("text").toString() != shownBefore; },
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

int runGridHereCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                     QCommandLineParser& parser, const QCommandLineOption& loadOption,
                     const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
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
    const auto click = [&](const QString& name) {
        auto* item = findItem(window->contentItem(), name);
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
    // Half a beat and a few frames past it: as far from a beat of the grid as the middle of the
    // beat is, and not on a round number of frames either.
    const qint64 target = before + static_cast<qint64>(period / 2.0) + 137;
    // The track has to reach the frame the head is sent to: what is left of it is the deck's own
    // word.
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
    // The mark on the button has to be a mark: the glyph for a target is drawn as a dot in most
    // fonts, and a dot is a button nobody presses. The ring is the mark, and it is measured, not
    // assumed.
    auto* mark = findItem(window->contentItem(), QStringLiteral("gridHereRing_A"));
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
    std::fprintf(
        stdout,
        "dj-ui: the grid came to the play head: a beat landed %.1f frames from frame %lld, "
        "the mark on the button is %.0f by %.0f pixels, and the file kept the grid across a "
        "reload\n",
        error, target, mark->width(), mark->height());
    return 0;
}

int runRefusalCheck(QQmlApplicationEngine& engine, FileBrowser& browser, BridgeClient* bridge,
                    UiSettings& settings, QCommandLineParser& parser,
                    const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
        for (const QVariant& entry : decks) {
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
    auto* panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    auto* list = findItem(window->contentItem(), QStringLiteral("browserList"));
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

    auto* deckButton = findItem(window->contentItem(), QStringLiteral("deckButton_A"));
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
    const auto faceOf = [](QQuickItem* button) {
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
        std::fprintf(stdout,
                     "dj-ui: a load into a playing deck is refused and the button blinks\n");
        return 0;
    }
    std::fprintf(stderr, "dj-ui: the refusal was not signalled\n");
    return 1;
}

int runGainCurveCheck(QQmlApplicationEngine& engine, UiSettings& settings) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    QQuickItem* content = window->contentItem();
    auto* trim = findItem(content, QStringLiteral("trim_A"));
    auto* master = findItem(content, QStringLiteral("masterKnob"));
    if (trim == nullptr || master == nullptr) {
        std::fprintf(stderr, "dj-ui: no level knobs in the mixer\n");
        return 1;
    }
    // Both knobs answer the same questions, which is what makes one check enough.
    // A QML function takes and returns `QVariant`, not `double`, whatever it is written as.
    const auto ask = [&](QQuickItem* knob, const char* name, double argument, double* out) {
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
    // A rate, not an average: a hair of a degree and a hundredth of a pixel, so the numbers below
    // are the slope at a spot rather than a chord across a fifth of the travel. The chord is what
    // made an earlier version of this check pass for the wrong shape.
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
    if (!ask(trim, "dbOf", 0.0, &unity) || !ask(trim, "dbOf", -135.0, &bottom) ||
        !ask(trim, "dbOf", 135.0, &top) || !ask(trim, "dbOf", -hair, &firstHair) ||
        !ask(trim, "dbOf", -135.0 + hair, &lastHairBefore) ||
        !ask(trim, "dbOf", -135.0, &lastHair)) {
        return 1;
    }
    const double trimAtUnity = std::abs(firstHair) / hair * degreesPerPixel;
    const double trimAtEnd = std::abs(lastHair - lastHairBefore) / hair * degreesPerPixel;
    // The master drags in pixels, so its law takes two arguments.
    const auto drag = [&](double pixels, double fromDb, double* out) {
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
    if (!drag(0.01, 0.0, &masterFirst) || !drag(masterTravel - 0.01, 0.0, &masterLastBefore) ||
        !drag(masterTravel, 0.0, &masterLast)) {
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
    const double promisedMaster = masterRange * (curved ? middle : 1.0) / masterTravel;
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
    auto* fader = findItem(content, QStringLiteral("fader_A"));
    if (fader == nullptr) {
        std::fprintf(stderr, "dj-ui: no channel fader in the mixer\n");
        return 1;
    }
    double faderTop = 0.0;
    double faderBottom = 0.0;
    double faderFirst = 0.0;
    double faderLastBefore = 0.0;
    if (!ask(fader, "valueAtHandle", 1.0, &faderTop) ||
        !ask(fader, "valueAtHandle", 0.0, &faderBottom) ||
        !ask(fader, "valueAtHandle", 1.0 - hair, &faderFirst) ||
        !ask(fader, "valueAtHandle", hair, &faderLastBefore)) {
        return 1;
    }
    const double faderAtTop = (faderTop - faderFirst) / hair;
    const double faderAtBottom = faderLastBefore / hair;
    // The fader's own shape, not the knobs': the two are configured apart, and a check that reads
    // the wrong one passes for the wrong reason.
    const bool faderCubic = settings.faderCurve() == QStringLiteral("cubic");
    const double promisedFader = faderCubic ? settings.faderCurveMiddle() : 1.0;
    const double promisedFaderBottom = faderCubic ? 2.0 - settings.faderCurveMiddle() : 1.0;
    std::fprintf(stdout,
                 "dj-ui: the channel fader moves %.3f of its range per unit of travel at the top "
                 "(curve %s at %.2f) and %.3f at the bottom\n",
                 faderAtTop, qPrintable(settings.faderCurve()), settings.faderCurveMiddle(),
                 faderAtBottom);
    if (std::abs(faderTop - 1.0) > 0.001 || std::abs(faderBottom) > 0.001) {
        std::fprintf(stderr, "dj-ui: the fader's ends are not unity and silence: %.3f / %.3f\n",
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
        if (!ask(fader, "valueAtHandle", 0.75, &quarter) ||
            !ask(fader, "valueAtHandle", 0.5, &half)) {
            return 1;
        }
        const double quarterDb = 20.0 * std::log10(std::max(quarter, 1e-9));
        const double halfDb = 20.0 * std::log10(std::max(half, 1e-9));
        std::fprintf(stdout,
                     "dj-ui: the fader at a quarter of the throw is %.1f dB, at half %.1f dB, and "
                     "it moves %.2f times as fast at the top as at the bottom\n",
                     quarterDb, halfDb, faderAtBottom > 0.0 ? faderAtTop / faderAtBottom : 0.0);
        if (quarterDb > -8.0 || quarterDb < -14.0) {
            std::fprintf(
                stderr,
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
            std::fprintf(
                stderr,
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

int runCurveCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    for (const QString& law : laws) {
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
        for (const QString& candidate : laws) {
            auto* button =
                findItem(window->contentItem(), QStringLiteral("curveButton_") + candidate);
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

    bridge->setCrossfaderCurve(QStringLiteral("constant_power"));

    // The cut-in law carries its own number: the engine must publish the object, the button must
    // light, and the field must show the number in force. A law that travelled without its value
    // would open somewhere other than the DJ sees.
    const auto cutInPublished = [&]() {
        const QVariant raw = bridge->snapshot().value(QStringLiteral("crossfader_curve"));
        const QVariantMap map = raw.toMap();
        if (!map.contains(QStringLiteral("cut"))) {
            return -1;
        }
        return map.value(QStringLiteral("cut")).toMap().value(QStringLiteral("cut_in")).toInt();
    };
    for (const int percent : {5, 7}) {
        bridge->setCrossfaderCurve(QStringLiteral("cut:%1").arg(percent));
        QElapsedTimer waiting;
        waiting.start();
        int seen = cutInPublished();
        while (waiting.elapsed() < 3000 && seen != percent) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
            seen = cutInPublished();
            QThread::msleep(10);
        }
        QCoreApplication::processEvents();
        auto* button = findItem(window->contentItem(), QStringLiteral("curveButton_cut"));
        if (button == nullptr) {
            std::fprintf(stderr, "dj-ui: the curve row has no cut button\n");
            return 1;
        }
        const bool lit = button->property("lit").toBool();
        auto* value = findItem(window->contentItem(), QStringLiteral("cutInValue"));
        const QString shown =
            value == nullptr ? QStringLiteral("nothing") : value->property("text").toString();
        std::fprintf(stdout, "dj-ui: cut-in %d -> engine says %d, button %s, field says %s\n",
                     percent, seen, lit ? "lit" : "plain", qPrintable(shown));
        if (seen != percent) {
            std::fprintf(stderr, "dj-ui: the engine did not take the cut-in at %d percent\n",
                         percent);
            return 1;
        }
        if (!lit) {
            std::fprintf(stderr, "dj-ui: the cut button does not show the law in force\n");
            return 1;
        }
        if (value == nullptr || !shown.startsWith(QString::number(percent))) {
            std::fprintf(stderr, "dj-ui: the cut-in field says \"%s\" while the engine has %d\n",
                         qPrintable(shown), percent);
            return 1;
        }
    }

    // Back to the default, so the next check in the set meets an ordinary law.
    bridge->setCrossfaderCurve(QStringLiteral("constant_power"));

    std::fprintf(stdout, "dj-ui: every crossfader law travels to the engine and back\n");
    return 0;
}

int runWheelCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    auto* trimA = findItem(window->contentItem(), QStringLiteral("trim_A"));
    if (trimA == nullptr) {
        std::fprintf(stderr, "dj-ui: no gain knob for deck A\n");
        return 1;
    }
    const auto fieldOfA = [&](const QString& field) {
        const QVariantList decks = bridge->snapshot().value(QStringLiteral("decks")).toList();
        for (const QVariant& entry : decks) {
            const QVariantMap deck = entry.toMap();
            if (deck.value(QStringLiteral("deck")).toString() == QStringLiteral("A")) {
                return deck.value(field).toDouble();
            }
        }
        return 0.0;
    };
    const double gainBefore = fieldOfA(QStringLiteral("linear_gain"));
    const double faderBefore = fieldOfA(QStringLiteral("fader"));
    const QPointF centre = trimA->mapToScene(QPointF(trimA->width() / 2.0, trimA->height() / 2.0));
    for (int notch = 0; notch < 3; ++notch) {
        QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(), QPoint(0, 120),
                          Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase, false);
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

    // A notch is a fixed turn, not a fixed number of decibels: scroll one notch up and one back
    // down and the pointer must land where it started. The decibels need not come back — the two
    // halves of the law are asymmetric on purpose — which is why the angle is what is measured
    // here.
    const auto pump = [&]() {
        for (int step = 0; step < 40; ++step) {
            QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
            QThread::msleep(5);
        }
    };
    const auto wheelAngle = [&](int notches) {
        QWheelEvent event(centre, window->mapToGlobal(centre.toPoint()), QPoint(),
                          QPoint(0, 120 * notches), Qt::NoButton, Qt::NoModifier, Qt::NoScrollPhase,
                          false);
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
    if (std::abs((angleUp - angleAtRest) - 13.5) > 1.0 ||
        std::abs((angleBack - angleDown) + 13.5) > 1.0) {
        std::fprintf(stderr, "dj-ui: the wheel turns the gain knob by different steps each way\n");
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the wheel trims the gain and leaves the fader where it was\n");
    return 0;
}

int runSpaceCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
                for (const QVariant& entry :
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
        for (const QVariant& entry : decks) {
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
    auto* toggle = window->findChild<QObject*>(QStringLiteral("browserToggle"));
    auto* browserPanel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    auto* shell = findItem(window->contentItem(), QStringLiteral("globalBar"));
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

int runNeutralCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                    const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
    const auto doubleClick = [&](QQuickItem* target, const char* name) {
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
    const auto fieldOf = [&](const char* deck, const char* field) {
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() != QString::fromLatin1(deck)) {
                continue;
            }
            return map.value(QString::fromLatin1(field)).toDouble();
        }
        return -1.0;
    };
    const auto bandOf = [&](const char* deck, const char* band) {
        for (const QVariant& entry : bridge->snapshot().value(QStringLiteral("decks")).toList()) {
            const QVariantMap map = entry.toMap();
            if (map.value(QStringLiteral("deck")).toString() != QString::fromLatin1(deck)) {
                continue;
            }
            const QVariantMap eq = map.value(QStringLiteral("eq")).toMap();
            return eq.value(QString::fromLatin1(band) + QStringLiteral("_db")).toDouble();
        }
        return 999.0;
    };

    // Every control away from where it should land: a double click that is not wired would then
    // leave the old number in place and the check would say which one.
    bridge->setChannelGain(QStringLiteral("A"), 0.5);
    bridge->setChannelGain(QStringLiteral("B"), 2.0);
    bridge->setChannelEq(QStringLiteral("A"), QStringLiteral("low"), 9.0);
    bridge->setChannelEq(QStringLiteral("B"), QStringLiteral("high"), -12.0);
    bridge->setMasterGain(0.25);
    bridge->setChannelFader(QStringLiteral("A"), 0.4);
    bridge->setChannelFader(QStringLiteral("B"), 0.7);
    bridge->setCrossfader(0.15);
    pump(60);

    if (!doubleClick(findItem(window->contentItem(), QStringLiteral("trim_A")), "the gain of A") ||
        !doubleClick(findItem(window->contentItem(), QStringLiteral("trim_B")), "the gain of B") ||
        !doubleClick(findItem(window->contentItem(), QStringLiteral("eq_low_A")), "the low band") ||
        !doubleClick(findItem(window->contentItem(), QStringLiteral("eq_high_B")),
                     "the high band") ||
        !doubleClick(findItem(window->contentItem(), QStringLiteral("masterKnob")), "the master") ||
        !doubleClick(findItem(window->contentItem(), QStringLiteral("fader_A")),
                     "the fader of A") ||
        !doubleClick(findItem(window->contentItem(), QStringLiteral("fader_B")),
                     "the fader of B") ||
        !doubleClick(findItem(window->contentItem(), QStringLiteral("crossfaderHandle")),
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
    std::fprintf(
        stdout,
        "dj-ui: double clicks put the gains at %.3f / %.3f, the bands at %+.1f / %+.1f dB, "
        "the master at %.3f (%.1f dB), the faders at %.3f / %.3f and the crossfader at "
        "%.2f\n",
        gainA, gainB, lowA, highB, master, 20.0 * std::log10(master), faderA, faderB, crossfader);
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

int runGridReconnectCheck(QQmlApplicationEngine& engine, BridgeClient* bridge,
                          QCommandLineParser& parser, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    const auto waitFor = [](const std::function<bool()>& ready, int budget) {
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
        for (const QVariant& entry : decks) {
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
    auto* strip = findItem(window->contentItem(), QStringLiteral("waveStrip_A"));
    auto* right = findItem(window->contentItem(), QStringLiteral("gridBeatRight_A"));
    auto* left = findItem(window->contentItem(), QStringLiteral("gridBeatLeft_A"));
    if (strip == nullptr || right == nullptr || left == nullptr) {
        std::fprintf(stderr, "dj-ui: the deck has no grid buttons to press\n");
        return 1;
    }
    const auto firstFrame = [&]() {
        const QVariantList list = segments();
        return list.isEmpty() ? -1.0
                              : list.first().toMap().value(QStringLiteral("frame")).toDouble();
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

int runTempoCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
        for (const QVariant& entry : decks) {
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
    auto* label = findItem(window->contentItem(), QStringLiteral("bpm_A"));
    const QString shown = label != nullptr ? label->property("text").toString() : QString();
    std::fprintf(stdout, "dj-ui: tempo %.2f BPM, first beat %llu, label \"%s\"\n", bpm,
                 static_cast<unsigned long long>(firstBeat), qPrintable(shown));
    if (bpm <= 0.0 || firstBeat == 0) {
        std::fprintf(stderr, "dj-ui: the deck published no usable grid\n");
        return 1;
    }
    if (!shown.contains(QStringLiteral("BPM"))) {
        std::fprintf(stderr, "dj-ui: the deck shows no tempo (label \"%s\")\n", qPrintable(shown));
        return 1;
    }
    // Two numbers, not one: the tempo the deck sounds at and the tempo the file was measured at. A DJ reads
    // both — with the pitch fader at zero they are the same number, and after a shift the first one moves.
    const auto numbersIn = [](const QString& text) {
        QStringList found;
        static const QRegularExpression number(QStringLiteral("[0-9]+[.,][0-9]"));
        auto match = number.globalMatch(text);
        while (match.hasNext()) {
            found.append(match.next().captured(0).replace(QLatin1Char(','), QLatin1Char('.')));
        }
        return found;
    };
    const QStringList atZero = numbersIn(shown);
    // Either separator: the path is drawn the way the platform spells it, and a check that insists on `/` is a
    // check that fails on the machine the shell is shipped to.
    if (atZero.size() != 2
        || (!shown.contains(QLatin1Char('/')) && !shown.contains(QLatin1Char('\\')))) {
        std::fprintf(stderr,
                     "dj-ui: the deck shows one tempo instead of two (label \"%s\")\n",
                     qPrintable(shown));
        return 1;
    }
    if (std::abs(atZero.at(0).toDouble() - atZero.at(1).toDouble()) > 0.15) {
        std::fprintf(stderr, "dj-ui: the two numbers differ with the pitch at zero (\"%s\")\n",
                     qPrintable(shown));
        return 1;
    }
    std::fprintf(stdout, "dj-ui: the deck shows the tempo twice (\"%s\")\n", qPrintable(shown));

    // The fader moves the first number and leaves the second alone: the file's tempo is not a setting.
    bridge->setTempo(QStringLiteral("A"), 1.06);
    QString moved;
    QElapsedTimer shifted;
    shifted.start();
    while (shifted.elapsed() < 4000) {
        QCoreApplication::processEvents();
        QThread::msleep(20);
        moved = label != nullptr ? label->property("text").toString() : QString();
        const QStringList pair = numbersIn(moved);
        if (pair.size() == 2 && pair.at(0).toDouble() > atZero.at(1).toDouble() + 1.0) {
            break;
        }
    }
    const QStringList afterShift = numbersIn(moved);
    if (afterShift.size() != 2) {
        std::fprintf(stderr, "dj-ui: the shifted label lost a number (\"%s\")\n", qPrintable(moved));
        return 1;
    }
    const double sounded = afterShift.at(0).toDouble();
    const double measuredAfter = afterShift.at(1).toDouble();
    if (std::abs(measuredAfter - atZero.at(1).toDouble()) > 0.15) {
        std::fprintf(stderr, "dj-ui: the file's own tempo moved with the fader (\"%s\")\n",
                     qPrintable(moved));
        return 1;
    }
    if (std::abs(sounded - atZero.at(1).toDouble() * 1.06) > 0.3) {
        std::fprintf(stderr,
                     "dj-ui: the sounding tempo did not follow the fader (\"%s\", expected %.1f)\n",
                     qPrintable(moved), atZero.at(1).toDouble() * 1.06);
        return 1;
    }
    bridge->setTempo(QStringLiteral("A"), 1.0);
    std::fprintf(stdout, "dj-ui: the pitch moved the sounding tempo to %.1f and left the file's %.1f (\"%s\")\n",
                 sounded, measuredAfter, qPrintable(moved));
    return 0;
}

int runBrowserRestoreCheck(QQmlApplicationEngine& engine, LayoutSettings& layout,
                           UiSettings& settings) {
    // The second start: a new process, a layout file left by the first one, and no other hint. What
    // the panel and the deck hold now is what a person would see, not what the file says.
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
    if (window == nullptr) {
        std::fprintf(stderr, "dj-ui: the root object is not a window\n");
        return 1;
    }
    // The splitter has not laid anything out until the scene has had a frame: reading geometry
    // before that measures the minimum sizes, not the settings.
    QEventLoop settle;
    QTimer::singleShot(600, &settle, &QEventLoop::quit);
    settle.exec();
    QQuickItem* panel = findItem(window->contentItem(), QStringLiteral("browserPanel"));
    QQuickItem* detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
    if (panel == nullptr || detail == nullptr) {
        std::fprintf(stderr, "dj-ui: the shell has no browser panel or no deck waveform\n");
        return 1;
    }
    QQuickItem* shellItem = findItem(window->contentItem(), QStringLiteral("shellLayout"));
    if (shellItem == nullptr || shellItem->height() <= 0.0) {
        std::fprintf(stderr, "dj-ui: the shell has no height to divide\n");
        return 1;
    }
    const int rail = panel->property("railWidth").toInt();
    const int font = panel->property("listFont").toInt();
    const double beats = detail->property("beatsOnScreen").toDouble();
    const double heightShare = panel->height() / shellItem->height() * 100.0;
    if (qAbs(heightShare - layout.browserPercent()) > 2.0) {
        std::fprintf(stderr, "dj-ui: the browser came back %0.1f%% tall instead of %0.1f%%\n",
                     heightShare, layout.browserPercent());
        return 1;
    }
    if (rail != layout.browserRailWidth() || font != layout.browserFont() ||
        qAbs(beats - layout.waveformBeats()) > 1e-6) {
        std::fprintf(stderr,
                     "dj-ui: the shell did not come back as it was left (rail %d of %d, font %d "
                     "of %d, beats %.1f of %.1f)\n",
                     rail, layout.browserRailWidth(), font, layout.browserFont(), beats,
                     layout.waveformBeats());
        return 1;
    }
    std::fprintf(stdout,
                 "dj-ui: a new run opens with the rail at %d px, the browser text at %d px, the "
                 "browser %0.1f%% tall and the deck window at %s beats, the numbers the last run "
                 "left\n",
                 rail, font, heightShare, qPrintable(QString::number(beats, 'f', 0)));
    // The modes of the deck are settings too: the last run left snap and quantize off and the scrub
    // audible, and both the runtime and the switches have to say so.
    auto* snapSwitch = findItem(window->contentItem(), QStringLiteral("snapGlobal"));
    auto* quantSwitch = findItem(window->contentItem(), QStringLiteral("quantizeGlobal"));
    if (snapSwitch == nullptr || quantSwitch == nullptr) {
        std::fprintf(stderr, "dj-ui: the mixer has no global switches to read back\n");
        return 1;
    }
    const bool snapLit = snapSwitch->property("on").toBool();
    const bool quantLit = quantSwitch->property("on").toBool();
    std::fprintf(stdout,
                 "dj-ui: the deck's modes came back as they were left: snap %s, quantize %s, scrub "
                 "%s; the switches read %s and %s\n",
                 settings.snapGrid() ? "on" : "off", settings.quantizeGrid() ? "on" : "off",
                 settings.scrubAudible() ? "audible" : "silent", snapLit ? "lit" : "dark",
                 quantLit ? "lit" : "dark");
    if (settings.snapGrid() || settings.quantizeGrid() || !settings.scrubAudible() || snapLit ||
        quantLit) {
        std::fprintf(stderr,
                     "dj-ui: the deck's modes did not come back as the last run left them\n");
        return 1;
    }
    return 0;
}

int runStripCheck(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                  const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    // The picture is the thing the user sees, so it gets its own measurement: the envelope the
    // engine publishes has to reach the strip that draws it.
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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
        std::fprintf(stdout, "dj-ui: strip check link %s\n", bridge->isConnected() ? "up" : "down");
    }

    auto* detail = findItem(window->contentItem(), QStringLiteral("deckDetail_A"));
    if (detail == nullptr) {
        std::fprintf(stderr, "dj-ui: deck A has no strip\n");
        return 1;
    }
    auto* envelope = qobject_cast<QObject*>(detail->property("envelope").value<QObject*>());
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

int runScenario(QQmlApplicationEngine& engine, BridgeClient* bridge, QCommandLineParser& parser,
                const QCommandLineOption& loadOption, const QCommandLineOption& connectOption) {
    auto* window = qobject_cast<QQuickWindow*>(engine.rootObjects().first());
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

} // namespace checks
