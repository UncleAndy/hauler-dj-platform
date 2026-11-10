//! The grid as a clock: beats, phases, and the distance between two beats.
#![allow(clippy::expect_used)]

use dj_domain::BeatGrid;
use dj_sync::{BeatClock, DeckClock, Phase, error_beats};

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

#[test]
fn a_beat_is_where_the_grid_says_it_is() {
    // 128 BPM at 48 kHz is 22500 frames to the beat, which is what the whole check leans on.
    let beat_at = grid(128.0, 15_360);
    let clock = deck(&beat_at, 15_360, 1.0);
    assert!(
        (clock.frames_per_beat() - 22_500.0).abs() < 1e-9,
        "{clock:?}"
    );
    assert!((clock.beat() - 0.0).abs() < 1e-12);
    assert!((clock.phase().value() - 0.0).abs() < 1e-12);

    let half_later = deck(&beat_at, 15_360 + 11_250, 1.0);
    assert!((half_later.beat() - 0.5).abs() < 1e-9);
    assert!((half_later.phase().value() - 0.5).abs() < 1e-9);

    let next_beat = deck(&beat_at, 15_360 + 22_500, 1.0);
    assert!((next_beat.beat() - 1.0).abs() < 1e-9);
    assert!((next_beat.phase().value() - 0.0).abs() < 1e-9);
}

#[test]
fn a_beat_before_the_grid_wraps_into_the_same_phase() {
    // A deck parked half a beat before its first beat is on the half-beat, not at minus zero point five.
    let beat_at = grid(128.0, 15_360);
    let before = deck(&beat_at, 15_360 - 11_250, 1.0);
    assert!((before.beat() + 0.5).abs() < 1e-9);
    assert!((before.phase().value() - 0.5).abs() < 1e-9);
}

#[test]
fn the_pitch_fader_changes_the_tempo_heard_and_not_the_beat_under_the_needle() {
    let beat_at = grid(128.0, 0);
    let plain = deck(&beat_at, 0, 1.0);
    let pitched = deck(&beat_at, 0, 1.05);
    assert!((plain.heard_bpm() - 128.0).abs() < 1e-9);
    assert!((pitched.heard_bpm() - 134.4).abs() < 1e-9);
    // The music's own position does not move: the beat is pinned to the file, not to the fader.
    assert!((plain.beat() - pitched.beat()).abs() < 1e-12);
}

#[test]
fn the_distance_between_two_beats_is_the_shorter_way_round() {
    // The follower is past the beat and the master is just before it: the way forward is short.
    assert!((error_beats(Phase::of(0.9), Phase::of(0.1)) - 0.2).abs() < 1e-12);
    // The other way round: the follower has to wait.
    assert!((error_beats(Phase::of(0.1), Phase::of(0.9)) + 0.2).abs() < 1e-12);
    assert!((error_beats(Phase::of(0.25), Phase::of(0.25))).abs() < 1e-12);
    // Exactly half a beat is the edge of the circle: forward and back are the same move, and the answer
    // is always "wait", in both directions, so a plan is reproducible rather than a coin toss.
    assert!((error_beats(Phase::of(0.0), Phase::of(0.5)) + 0.5).abs() < 1e-12);
    assert!((error_beats(Phase::of(0.5), Phase::of(0.0)) + 0.5).abs() < 1e-12);
}

#[test]
fn a_phase_near_the_beat_counts_as_on_it() {
    assert!(Phase::of(0.98).is_on_beat(0.05));
    assert!(Phase::of(0.02).is_on_beat(0.05));
    assert!(!Phase::of(0.5).is_on_beat(0.05));
}
