//! The beat grid arithmetic: beats to frames and back, across tempos (DJP-SYNC-003).

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_domain::{BeatGrid, BeatSegment};

const RATE: f64 = 48_000.0;

#[test]
fn one_tempo_is_the_grid_a_measured_track_has() {
    // 120 BPM at 48 kHz: one beat every 24 000 frames, the first one 12 000 frames in.
    let grid = BeatGrid::single(120.0, 12_000).expect("a grid");
    assert!(grid.is_single());
    assert_eq!(grid.bpm(), 120.0);
    assert_eq!(grid.first_beat_frame(), 12_000);
    assert!((grid.beat_at(12_000, RATE) - 0.0).abs() < 1e-9);
    assert!((grid.beat_at(36_000, RATE) - 1.0).abs() < 1e-9);
    assert!((grid.beat_at(48_000, RATE) - 1.5).abs() < 1e-9);
    // Before the first beat the answer is negative: "before the grid" is not "on its first beat".
    assert!((grid.beat_at(0, RATE) + 0.5).abs() < 1e-9);
    // And the way back lands in the same place.
    assert!((grid.frame_of_beat(2.0, RATE) - 60_000.0).abs() < 1e-6);
    assert!((grid.bpm_at(96_000) - 120.0).abs() < 1e-9);
}

#[test]
fn a_cut_into_a_new_tempo_keeps_the_beats_running() {
    // 120 BPM for eight beats, then the same grid at 128 from the ninth beat on: an edit, not a new
    // track, so the beat numbers continue across the cut.
    let first = BeatSegment {
        frame: 0,
        beat: 0,
        bpm: 120.0,
    };
    let second = BeatSegment {
        frame: 192_000, // eight beats of 120
        beat: 8,
        bpm: 128.0,
    };
    let grid = BeatGrid::from_segments(vec![first, second]).expect("a grid");
    assert!(!grid.is_single());
    assert!((grid.beat_at(192_000, RATE) - 8.0).abs() < 1e-9);
    assert!((grid.beat_at(0, RATE) - 0.0).abs() < 1e-9);

    // A beat after the cut is shorter: 128 BPM is 22 500 frames per beat, not 24 000.
    let after_one_beat = 192_000 + 22_500;
    assert!((grid.beat_at(after_one_beat, RATE) - 9.0).abs() < 1e-9);
    assert!((grid.bpm_at(after_one_beat) - 128.0).abs() < 1e-9);

    // The tempo the deck shows follows the play head, and the first beat stays where it was.
    assert!((grid.bpm_at(0) - 120.0).abs() < 1e-9);
    assert!((grid.bpm() - 120.0).abs() < 1e-9);
    assert_eq!(grid.first_beat_frame(), 0);

    // Both ways round agree on either side of the cut.
    assert!((grid.frame_of_beat(9.0, RATE) - after_one_beat as f64).abs() < 1e-6);
    assert!((grid.frame_of_beat(4.0, RATE) - 96_000.0).abs() < 1e-6);
}

#[test]
fn a_grid_that_does_not_move_forwards_is_refused() {
    let good = BeatSegment {
        frame: 1_000,
        beat: 0,
        bpm: 120.0,
    };
    assert!(BeatGrid::from_segments(Vec::new()).is_none(), "empty");
    assert!(
        BeatGrid::single(0.0, 0).is_none() && BeatGrid::single(-120.0, 0).is_none(),
        "a tempo has to be a positive number"
    );
    assert!(BeatGrid::single(f64::NAN, 0).is_none(), "and a finite one");
    assert!(
        BeatGrid::from_segments(vec![
            good,
            BeatSegment {
                frame: 1_000,
                beat: 4,
                bpm: 120.0
            }
        ])
        .is_none(),
        "two segments cannot start on the same frame"
    );
    assert!(
        BeatGrid::from_segments(vec![
            good,
            BeatSegment {
                frame: 50_000,
                beat: 0,
                bpm: 120.0
            }
        ])
        .is_none(),
        "and the beat count has to grow"
    );
}

#[test]
fn a_shift_moves_the_whole_grid_and_nothing_else() {
    let grid = BeatGrid::single(120.0, 12_000).expect("a grid");
    let moved = grid.shifted(480).expect("a shifted grid");
    assert_eq!(moved.first_beat_frame(), 12_480);
    assert_eq!(moved.bpm(), 120.0);
    let back = moved.shifted(-480).expect("back again");
    assert_eq!(back.first_beat_frame(), 12_000);
    // Off the start of the file is not a grid: the beats would be before the audio they belong to.
    assert!(grid.shifted(-12_001).is_none());
}

#[test]
fn a_new_tempo_stretches_the_grid_around_its_first_beat() {
    let grid = BeatGrid::single(120.0, 12_000).expect("a grid");
    // Saying "this is 128" keeps the first beat and pulls the rest closer together: 12 000 + 22 500
    // instead of 12 000 + 24 000.
    let retimed = grid.retimed(128.0).expect("a retimed grid");
    assert_eq!(retimed.first_beat_frame(), 12_000);
    assert!((retimed.bpm() - 128.0).abs() < 1e-9);
    let second_beat = retimed.frame_of_beat(1.0, RATE);
    assert!(
        (second_beat - 34_500.0).abs() < 1e-6,
        "second beat at {second_beat} instead of 34 500"
    );
    // Half the tempo is the same grid twice as slow, first beat still where it was.
    let halved = grid.retimed(60.0).expect("a halved grid");
    assert_eq!(halved.first_beat_frame(), 12_000);
    assert!((halved.frame_of_beat(1.0, RATE) - 60_000.0).abs() < 1e-6);
    assert!(grid.retimed(0.0).is_none() && grid.retimed(-1.0).is_none());
}

#[test]
fn retiming_keeps_the_shape_of_a_track_with_two_tempos() {
    let grid = BeatGrid::from_segments(vec![
        BeatSegment {
            frame: 0,
            beat: 0,
            bpm: 120.0,
        },
        BeatSegment {
            frame: 192_000,
            beat: 8,
            bpm: 128.0,
        },
    ])
    .expect("a grid");
    // Read the whole track twice as fast: every stretch keeps its relationship to the first beat, and the
    // cut lands at half the frame.
    let halved = grid.retimed(60.0).expect("a retimed grid");
    assert_eq!(halved.segments().len(), 2);
    assert!((halved.segments()[0].bpm - 60.0).abs() < 1e-9);
    assert!((halved.segments()[1].bpm - 64.0).abs() < 1e-9);
    assert_eq!(halved.segments()[1].beat, 8);
    assert_eq!(halved.segments()[1].frame, 384_000);
}

#[test]
fn what_a_file_holds_becomes_a_grid() {
    // A file with one tempo and no anchors: exactly the old pair.
    let stored = BeatGrid::from_stored(120.0, 12_000, &[], RATE).expect("a grid");
    assert!(stored.is_single());
    assert!((stored.bpm() - 120.0).abs() < 1e-9);

    // A file with one anchor: the stretch before it gets the tempo that makes the grid pass through
    // both points, the last stretch keeps the stated tempo.
    let anchors = [(0u64, 0u64), (190_000, 8)];
    let stored = BeatGrid::from_stored(120.0, 0, &[anchors[1]], RATE).expect("a grid");
    assert_eq!(stored.segments().len(), 2);
    let first = stored.segments()[0];
    let expected = 8.0 * 60.0 * RATE / 190_000.0;
    assert!(
        (first.bpm - expected).abs() < 1e-9,
        "the stretch before the anchor: {} instead of {expected}",
        first.bpm
    );
    // The last stretch has no next anchor, so it keeps what the file said.
    assert!((stored.segments()[1].bpm - 120.0).abs() < 1e-9);
    assert!((stored.beat_at(190_000, RATE) - 8.0).abs() < 1e-9);

    // And the way back: what is stored is the first segment plus the anchors.
    assert_eq!(stored.stored_anchors(), vec![(190_000, 8)]);
}
