//! What the planner decides, and what it refuses.
#![allow(clippy::expect_used)]

use dj_domain::BeatGrid;
use dj_sync::{
    BeatClock, COAST_MS, DeckClock, Phase, PitchLimits, Refusal, SyncMode, matching_ratio, plan,
};

const RATE: f64 = 48_000.0;

fn grid(bpm: f64, first_beat_frame: u64) -> BeatGrid {
    BeatGrid::single(bpm, first_beat_frame).expect("a grid of one tempo")
}

fn deck<'a>(grid: &'a BeatGrid, frame: u64, ratio: f64) -> DeckClock<'a> {
    DeckClock {
        grid,
        source_rate: RATE,
        frame,
        ratio,
    }
}

/// A clock that is not a deck: it says its tempo, its phase and how long ago it spoke, which is all a
/// MIDI clock, a network clock or a future source has to say.
struct ExternalClock {
    name: &'static str,
    bpm: f64,
    phase: f64,
    age_ms: f64,
    running: bool,
}

impl BeatClock for ExternalClock {
    fn name(&self) -> &str {
        self.name
    }

    fn tempo_bpm(&self) -> f64 {
        self.bpm
    }

    fn phase(&self) -> Phase {
        Phase::of(self.phase)
    }

    fn freshness_ms(&self) -> f64 {
        self.age_ms
    }

    fn running(&self) -> bool {
        self.running
    }
}

#[test]
fn tempo_sync_matches_the_speed_the_master_is_heard_at() {
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 0);
    let master = deck(&master_grid, 0, 1.0);
    let follower = deck(&follower_grid, 0, 1.0);

    let plan = plan(&master, &follower, SyncMode::Tempo, PitchLimits::default())
        .expect("128 against 120 is a small move");
    assert!((plan.ratio - 128.0 / 120.0).abs() < 1e-12, "{plan:?}");
    assert!(plan.jump.is_none(), "tempo sync does not move the beat");
    assert!((plan.master_bpm - 128.0).abs() < 1e-12);
    assert!((plan.follower_bpm - 120.0).abs() < 1e-12);
    assert!((plan.phase_error_ms).abs() < 1e-9);
}

#[test]
fn the_master_s_own_pitch_counts_as_its_tempo() {
    // A master running five percent fast is heard at 134.4, and that is what the follower matches.
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 0);
    let master = deck(&master_grid, 0, 1.05);
    let follower = deck(&follower_grid, 0, 1.0);
    let plan = plan(&master, &follower, SyncMode::Tempo, PitchLimits::default()).expect("in range");
    assert!((plan.ratio - 134.4 / 120.0).abs() < 1e-12, "{plan:?}");
}

#[test]
fn beat_sync_asks_for_the_shortest_jump_and_it_lands_on_the_beat() {
    // The follower stands a quarter of a beat late; a quarter of a beat is 6000 frames at 120 BPM.
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 0);
    let master = deck(&master_grid, 0, 1.0);
    let late = deck(&follower_grid, 6_000, 1.0);

    let plan = plan(&master, &late, SyncMode::Beat, PitchLimits::default()).expect("in range");
    let jump = plan.jump.expect("beat sync moves the beat");
    // The follower is a quarter beat ahead, so the correction is backwards.
    assert!((jump.beats + 0.25).abs() < 1e-9, "{jump:?}");
    assert_eq!(jump.frames, -6_000);
    // Applying the jump — the deck moves by exactly the frames the plan asks for — leaves the follower
    // on the master's beat, in the same instant.
    let moved = 6_000_i64 + jump.frames;
    let landed = deck(&follower_grid, moved as u64, 1.0);
    assert!((landed.phase().value()).abs() < 1e-9);
    assert!(!plan.is_on_beat());
}

#[test]
fn a_pitch_the_fader_cannot_reach_is_refused_rather_than_rounded() {
    let master_grid = grid(150.0, 0);
    let follower_grid = grid(120.0, 0);
    let master = deck(&master_grid, 0, 1.0);
    let follower = deck(&follower_grid, 0, 1.0);
    let refusal = plan(
        &master,
        &follower,
        SyncMode::Tempo,
        PitchLimits::percent(16.0),
    )
    .expect_err("125 percent is outside a sixteen percent fader");
    match refusal {
        Refusal::OutOfRange {
            wanted,
            limit_percent,
        } => {
            assert!((wanted - 1.25).abs() < 1e-12);
            assert!((limit_percent - 16.0).abs() < 1e-12);
        }
        other => panic!("expected a refusal for the range, got {other:?}"),
    }
}

#[test]
fn an_external_clock_is_planned_against_like_a_deck() {
    // The same follower, a clock that is not a deck: 128 BPM, on the beat, fresh.
    let follower_grid = grid(120.0, 0);
    let follower = deck(&follower_grid, 0, 1.0);
    let midi = ExternalClock {
        name: "midi clock",
        bpm: 128.0,
        phase: 0.0,
        age_ms: 12.0,
        running: true,
    };
    let plan = plan(&midi, &follower, SyncMode::Beat, PitchLimits::default()).expect("fresh clock");
    assert!((plan.ratio - 128.0 / 120.0).abs() < 1e-12);
    assert_eq!(plan.jump.map(|jump| jump.frames), Some(0));
    assert!(plan.is_on_beat());
}

#[test]
fn a_clock_that_stopped_speaking_is_coasted_and_then_dropped() {
    let follower_grid = grid(120.0, 0);
    let follower = deck(&follower_grid, 0, 1.0);
    let late = ExternalClock {
        name: "midi clock",
        bpm: 128.0,
        phase: 0.25,
        age_ms: 120.0,
        running: true,
    };
    // Late but not gone: still followed, so a dropped tick does not stop the music.
    let plan = plan(&late, &follower, SyncMode::Tempo, PitchLimits::default()).expect("coasting");
    assert!(plan.phase_error_ms.abs() > 0.0);

    let gone = ExternalClock {
        age_ms: COAST_MS + 1.0,
        ..late
    };
    match plan_against(&gone, &follower) {
        Err(Refusal::ClockLost { name, age_ms }) => {
            assert_eq!(name, "midi clock");
            assert!(age_ms > COAST_MS);
        }
        other => panic!("expected the clock to be lost, got {other:?}"),
    }
}

fn plan_against(
    clock: &ExternalClock,
    follower: &DeckClock<'_>,
) -> Result<dj_sync::SyncPlan, Refusal> {
    plan(clock, follower, SyncMode::Tempo, PitchLimits::default())
}

#[test]
fn a_stopped_clock_has_no_beat_to_follow() {
    let follower_grid = grid(120.0, 0);
    let follower = deck(&follower_grid, 0, 1.0);
    let stopped = ExternalClock {
        name: "link",
        bpm: 128.0,
        phase: 0.0,
        age_ms: 0.0,
        running: false,
    };
    match plan(&stopped, &follower, SyncMode::Tempo, PitchLimits::default()) {
        Err(Refusal::ClockStopped { name }) => assert_eq!(name, "link"),
        other => panic!("expected a stopped clock, got {other:?}"),
    }
}

#[test]
fn sync_off_reports_the_distance_and_leaves_the_deck_alone() {
    let master_grid = grid(128.0, 0);
    let follower_grid = grid(120.0, 0);
    let master = deck(&master_grid, 0, 1.0);
    let follower = deck(&follower_grid, 6_000, 1.07);
    let plan =
        plan(&master, &follower, SyncMode::Off, PitchLimits::default()).expect("off is legal");
    assert!(
        (plan.ratio - 1.07).abs() < 1e-12,
        "the deck keeps its speed"
    );
    assert!(plan.jump.is_none());
    // A quarter of a beat at 128 BPM is 117.1875 ms. The follower stands at a quarter past the beat,
    // which is a quarter beat **ahead** of the master, so the error is negative: it has to wait.
    assert!((plan.phase_error_ms + 117.1875).abs() < 1e-9, "{plan:?}");
}

#[test]
fn a_ratio_needs_two_real_tempos() {
    assert!(matching_ratio(128.0, 0.0).is_none());
    assert!(matching_ratio(f64::NAN, 120.0).is_none());
    assert!(matching_ratio(-128.0, 120.0).is_none());
    assert!((matching_ratio(128.0, 120.0).expect("both tempos") - 128.0 / 120.0).abs() < 1e-12);
}
