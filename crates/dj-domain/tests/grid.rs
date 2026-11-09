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
    let moved = grid.shifted(480, 48_000.0).expect("a shifted grid");
    assert_eq!(moved.first_beat_frame(), 12_480);
    assert_eq!(moved.bpm(), 120.0);
    let back = moved.shifted(-480, 48_000.0).expect("back again");
    assert_eq!(back.first_beat_frame(), 12_000);
}

#[test]
fn a_shift_past_the_start_of_the_file_rebases_the_grid_and_keeps_its_lines() {
    // 120 BPM at 48 kHz: 24 000 frames a beat, and a square of four bars is 384 000 frames.
    let rate = 48_000.0;
    let near = BeatGrid::single(120.0, 12_000).expect("a grid");
    // The same lines, described from the next square along: the anchor sits sixteen beats later.
    let far = BeatGrid::single(120.0, 396_000).expect("the same grid a square later");
    // One beat earlier, which takes the near anchor below zero and leaves the far one inside the file.
    let rebased = near
        .shifted(-24_000, rate)
        .expect("a grid shifted past the start");
    let plain = far
        .shifted(-24_000, rate)
        .expect("a grid shifted inside the file");
    // One beat earlier from 12 000 would be −12 000, so the grid is rebased forward by one whole
    // square — 384 000 frames, sixteen beats — and lands exactly where the far grid put the same lines.
    assert_eq!(rebased.first_beat_frame(), 372_000);
    // The rebase is a description, not an edit: the lines are where the plain shift put them.
    for frame in [0u64, 5_000, 50_000, 200_000, 380_000] {
        let a = rebased.beat_at(frame, rate).rem_euclid(1.0);
        let b = plain.beat_at(frame, rate).rem_euclid(1.0);
        assert!(
            (a - b).abs() < 0.01,
            "the rebase moved a line at {frame}: {a} against {b}"
        );
    }
    // And the shift itself still happened: one beat earlier means every frame carries one beat more.
    assert!((rebased.beat_at(0, rate) - near.beat_at(0, rate) - 1.0).abs() < 0.01);
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
    let stored = BeatGrid::from_stored(120.0, 12_000, &[], RATE, 4).expect("a grid");
    assert!(stored.is_single());
    assert!((stored.bpm() - 120.0).abs() < 1e-9);

    // A file with one anchor: the stretch before it gets the tempo that makes the grid pass through
    // both points, the last stretch keeps the stated tempo.
    let anchors = [(0u64, 0u64), (190_000, 8)];
    let stored = BeatGrid::from_stored(120.0, 0, &[anchors[1]], RATE, 4).expect("a grid");
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

#[test]
fn a_waltz_keeps_its_three_beat_bar_through_a_shift_and_through_the_file() {
    // A bar is measured, not assumed: three beats make a square of twelve, not sixteen, and a shift that
    // rebases by whole squares has to rebase by these twelve or the bars move under the DJ's hand.
    let grid = BeatGrid::from_segments(vec![BeatSegment {
        frame: 12_000,
        beat: 0,
        bpm: 120.0,
    }])
    .expect("a grid")
    .with_beats_per_bar(3);
    assert_eq!(grid.beats_per_bar(), 3);
    assert_eq!(grid.square_beats(), 12, "four bars of three");

    let moved = grid.shifted(-12_001, RATE).expect("a shift past the start");
    assert_eq!(
        moved.beats_per_bar(),
        3,
        "a shift does not re-count the bar"
    );
    assert_eq!(
        moved.segments()[0].beat % 12,
        0,
        "the anchor was rebased by a square of three-beat bars"
    );

    // The file states it too, and a file that does not is read as four: the metre is new, the files are
    // not, and a stored grid without it was drawn in four.
    let json = serde_json::to_string(&grid).expect("json");
    let back: BeatGrid = serde_json::from_str(&json).expect("json back");
    assert_eq!(back.beats_per_bar(), 3);
    let old: BeatGrid =
        serde_json::from_str(r#"{"segments":[{"frame":12000,"beat":0,"bpm":120.0}]}"#)
            .expect("an old grid");
    assert_eq!(
        old.beats_per_bar(),
        4,
        "a file written before the metre was measured"
    );
    assert_eq!(old.square_beats(), 16);
}
