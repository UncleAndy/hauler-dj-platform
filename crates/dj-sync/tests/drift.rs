//! Ten minutes of sync, measured the way AC-SYNC measures it: the drift between two beats stays inside
//! two milliseconds. The control arm is here on purpose — a meter that cannot see a deck running away
//! is a meter nobody should believe.
#![allow(clippy::expect_used)]

use dj_domain::BeatGrid;
use dj_sync::{BeatClock, DeckClock, DriftMeter, Phase, PitchLimits, SyncMode, plan};

const RATE: f64 = 48_000.0;
/// The control loop of the engine: one reading every 50 ms.
const TICK_FRAMES: u64 = 2_400;
/// Ten minutes, as the acceptance criterion asks.
const TICKS: u64 = 12_000;

fn grid(bpm: f64, first_beat_frame: u64) -> BeatGrid {
    BeatGrid::single(bpm, first_beat_frame).expect("a grid of one tempo")
}

#[test]
fn ten_minutes_of_tempo_sync_stay_inside_two_milliseconds() {
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 1_000);
    let limits = PitchLimits::default();

    let mut master_frame: f64 = 0.0;
    // On the beat of a 120 BPM grid whose first beat sits at frame 1000.
    let mut follower_frame: f64 = 1_000.0;
    let mut ratio = 1.0;
    let mut meter = DriftMeter::new();

    for tick in 0..TICKS {
        let master = DeckClock {
            grid: &master_grid,
            source_rate: RATE,
            frame: master_frame.round() as u64,
            ratio: 1.0,
        };
        let follower = DeckClock {
            grid: &follower_grid,
            source_rate: RATE,
            frame: follower_frame.round() as u64,
            ratio,
        };
        let planned = plan(&master, &follower, SyncMode::Tempo, limits).expect("in range");
        meter.record(planned.phase_error_ms, tick * TICK_FRAMES);
        ratio = planned.ratio;

        master_frame += TICK_FRAMES as f64;
        follower_frame += TICK_FRAMES as f64 * ratio;
    }

    println!(
        "tempo sync: {} readings, worst {:.4} ms at frame {:?}, mean {:.4} ms, ratio {ratio:.9}",
        meter.samples(),
        meter.worst_ms(),
        meter.worst_at(),
        meter.mean_abs_ms()
    );
    assert!((ratio - 128.0 / 120.0).abs() < 1e-12);
    assert!(
        meter.within(2.0),
        "the decks drifted {:.3} ms apart, the criterion allows 2",
        meter.worst_ms()
    );
}

#[test]
fn without_sync_the_same_two_decks_run_away_which_is_what_the_meter_is_for() {
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 1_000);

    let mut master_frame: f64 = 0.0;
    let mut follower_frame: f64 = 1_000.0;
    let mut meter = DriftMeter::new();
    let mut gap_beats = 0.0;

    for tick in 0..TICKS {
        let master = DeckClock {
            grid: &master_grid,
            source_rate: RATE,
            frame: master_frame.round() as u64,
            ratio: 1.0,
        };
        let follower = DeckClock {
            grid: &follower_grid,
            source_rate: RATE,
            frame: follower_frame.round() as u64,
            ratio: 1.0,
        };
        let reading =
            plan(&master, &follower, SyncMode::Off, PitchLimits::default()).expect("off is legal");
        meter.record(reading.phase_error_ms, tick * TICK_FRAMES);
        // The phase is a circle and cannot show more than half a beat, so the count of beats is kept
        // apart: two decks can sit in phase and be eighty beats apart, which is a different defect and
        // a worse one.
        gap_beats = master.beat() - follower.beat();

        master_frame += TICK_FRAMES as f64;
        follower_frame += TICK_FRAMES as f64;
    }

    // Half a beat at 128 BPM, the furthest the phase can ever read.
    let half_a_beat_ms = 60_000.0 / 128.0 / 2.0;
    println!(
        "no sync: worst phase {:.1} ms (half a beat is {:.1}), mean {:.1}, the decks are {:.1} beats \
         apart",
        meter.worst_ms(),
        half_a_beat_ms,
        meter.mean_abs_ms(),
        gap_beats
    );
    assert!(
        (meter.worst_ms() - half_a_beat_ms).abs() < 1.0,
        "the control arm should sit at the edge of the circle, it reported {:.1} ms",
        meter.worst_ms()
    );
    // Ten minutes at eight BPM apart: eighty beats, which is what "out of sync" really means.
    assert!(
        gap_beats.abs() > 50.0,
        "the control arm should be tens of beats apart, it reported {gap_beats:.1}"
    );
}

#[test]
fn a_follower_at_another_sample_rate_stays_in_sync_for_ten_minutes() {
    // A 44.1 kHz file on a 48 kHz engine: the ratio is the same number, but the frames the deck consumes
    // per tick are not whole, so the beat lands between frames and the meter has something real to see.
    let master_grid = grid(128.0, 0);
    let follower_grid = BeatGrid::single(120.0, 44_100).expect("a grid of one tempo");
    let follower_rate = 44_100.0;
    let limits = PitchLimits::default();

    let mut master_frame: f64 = 0.0;
    let mut follower_frame: f64 = 44_100.0;
    let mut ratio = 1.0;
    let mut meter = DriftMeter::new();

    for tick in 0..TICKS {
        let master = DeckClock {
            grid: &master_grid,
            source_rate: RATE,
            frame: master_frame.round() as u64,
            ratio: 1.0,
        };
        let follower = DeckClock {
            grid: &follower_grid,
            source_rate: follower_rate,
            frame: follower_frame.round() as u64,
            ratio,
        };
        let planned = plan(&master, &follower, SyncMode::Tempo, limits).expect("in range");
        meter.record(planned.phase_error_ms, tick * TICK_FRAMES);
        ratio = planned.ratio;

        master_frame += TICK_FRAMES as f64;
        // Source frames of the follower per engine frame: the file runs at its own rate, at this ratio.
        follower_frame += TICK_FRAMES as f64 * ratio * follower_rate / RATE;
    }

    println!(
        "mixed rates: worst {:.4} ms, mean {:.4} ms, ratio {ratio:.9}",
        meter.worst_ms(),
        meter.mean_abs_ms()
    );
    assert!((ratio - 128.0 / 120.0).abs() < 1e-12);
    assert!(meter.within(2.0), "worst {:.3} ms", meter.worst_ms());
}

#[test]
fn a_tempo_change_on_the_master_is_followed_without_a_burst_of_drift() {
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 1_000);
    let limits = PitchLimits::default();

    let mut master_frame: f64 = 0.0;
    let mut follower_frame: f64 = 1_000.0;
    let mut master_ratio = 1.0;
    let mut ratio = 1.0;
    let mut meter = DriftMeter::new();

    for tick in 0..TICKS {
        // Halfway through the set the master's fader moves: 128 heard as 126.
        if tick == TICKS / 2 {
            master_ratio = 126.0 / 128.0;
        }
        let master = DeckClock {
            grid: &master_grid,
            source_rate: RATE,
            frame: master_frame.round() as u64,
            ratio: master_ratio,
        };
        let follower = DeckClock {
            grid: &follower_grid,
            source_rate: RATE,
            frame: follower_frame.round() as u64,
            ratio,
        };
        let planned = plan(&master, &follower, SyncMode::Tempo, limits).expect("in range");
        meter.record(planned.phase_error_ms, tick * TICK_FRAMES);
        ratio = planned.ratio;

        master_frame += TICK_FRAMES as f64 * master_ratio;
        follower_frame += TICK_FRAMES as f64 * ratio;
    }

    println!(
        "tempo change: worst {:.4} ms, mean {:.4} ms, ratio after the move {ratio:.9}",
        meter.worst_ms(),
        meter.mean_abs_ms()
    );
    assert!((ratio - 126.0 / 120.0).abs() < 1e-9);
    assert!(meter.within(2.0), "worst {:.3} ms", meter.worst_ms());
}

#[test]
fn a_ratio_one_hair_off_is_already_too_much_for_ten_minutes() {
    // The reason the engine re-plans on every control tick instead of setting the speed once: a ratio
    // wrong by one part in a hundred thousand is inaudible for a minute and five milliseconds out at
    // the end of the set, which is more than the criterion allows.
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 1_000);

    let mut master_frame: f64 = 0.0;
    let mut follower_frame: f64 = 1_000.0;
    let ratio = 128.0 / 120.0 * (1.0 + 1e-5);
    let mut meter = DriftMeter::new();

    for tick in 0..TICKS {
        let master = DeckClock {
            grid: &master_grid,
            source_rate: RATE,
            frame: master_frame.round() as u64,
            ratio: 1.0,
        };
        let follower = DeckClock {
            grid: &follower_grid,
            source_rate: RATE,
            frame: follower_frame.round() as u64,
            ratio,
        };
        let reading =
            plan(&master, &follower, SyncMode::Off, PitchLimits::default()).expect("off is legal");
        meter.record(reading.phase_error_ms, tick * TICK_FRAMES);

        master_frame += TICK_FRAMES as f64;
        follower_frame += TICK_FRAMES as f64 * ratio;
    }

    println!(
        "one part in a hundred thousand: worst {:.2} ms over ten minutes",
        meter.worst_ms()
    );
    assert!(
        meter.worst_ms() > 2.0,
        "the meter should see a permanent one-part-in-a-hundred-thousand error, it reported {:.2} ms",
        meter.worst_ms()
    );
}

#[test]
fn beat_sync_puts_the_follower_on_the_master_s_beat() {
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 0);
    let master = DeckClock {
        grid: &master_grid,
        source_rate: RATE,
        frame: 20_000,
        ratio: 1.0,
    };
    // Somewhere in the middle of a beat, with the master ahead of it.
    let follower = DeckClock {
        grid: &follower_grid,
        source_rate: RATE,
        frame: 40_000,
        ratio: 1.0,
    };
    let before =
        plan(&master, &follower, SyncMode::Beat, PitchLimits::default()).expect("in range");
    let jump = before.jump.expect("beat sync moves the beat");

    let landed_frame = (40_000_i64 + jump.frames).max(0) as u64;
    let landed = DeckClock {
        grid: &follower_grid,
        source_rate: RATE,
        frame: landed_frame,
        ratio: 1.0,
    };
    let after = plan(&master, &landed, SyncMode::Beat, PitchLimits::default()).expect("in range");
    println!(
        "beat sync: error before {:.3} ms, jump {} frames, after {:.6} ms",
        before.phase_error_ms, jump.frames, after.phase_error_ms
    );
    assert!(
        after.phase_error_ms.abs() < 0.05,
        "after the jump the decks are still {:.3} ms apart",
        after.phase_error_ms
    );
}

/// A clock that is not a deck: it counts its own beats and reports how long ago it last spoke.
struct ExternalClock {
    beats: f64,
    bpm: f64,
    age_ms: f64,
}

impl BeatClock for ExternalClock {
    fn name(&self) -> &str {
        "external"
    }

    fn tempo_bpm(&self) -> f64 {
        self.bpm
    }

    fn phase(&self) -> Phase {
        Phase::of(self.beats)
    }

    fn freshness_ms(&self) -> f64 {
        self.age_ms
    }
}

#[test]
fn an_external_clock_drives_the_same_planner_for_ten_minutes() {
    let follower_grid = grid(120.0, 0);
    let mut clock = ExternalClock {
        beats: 0.0,
        bpm: 128.0,
        age_ms: 5.0,
    };
    let mut follower_frame: f64 = 0.0;
    let mut ratio = 1.0;
    let mut meter = DriftMeter::new();

    for tick in 0..TICKS {
        let follower = DeckClock {
            grid: &follower_grid,
            source_rate: RATE,
            frame: follower_frame.round() as u64,
            ratio,
        };
        let planned = plan(&clock, &follower, SyncMode::Tempo, PitchLimits::default())
            .expect("the external clock is fresh");
        meter.record(planned.phase_error_ms, tick * TICK_FRAMES);
        ratio = planned.ratio;

        // The external clock advances in beats of its own, and speaks once per control tick.
        clock.beats += TICK_FRAMES as f64 / RATE * clock.bpm / 60.0;
        follower_frame += TICK_FRAMES as f64 * ratio;
    }

    println!(
        "external clock: worst {:.4} ms, mean {:.4} ms, ratio {ratio:.9}",
        meter.worst_ms(),
        meter.mean_abs_ms()
    );
    assert!(meter.within(2.0), "worst {:.3} ms", meter.worst_ms());
}
