//! The grid the loader measures, stores and reads back (B02, DJP-MEDIA-006, DJP-SYNC-003).

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-engine-grid-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A 128 BPM click track, 16-bit stereo, first click 4800 frames in.
fn write_click_track(path: &Path, seconds: f64) {
    let bpm = 128.0f64;
    let frames = (f64::from(RATE) * seconds) as usize;
    let period = f64::from(RATE) * 60.0 / bpm;
    let mut samples = vec![0.0f32; frames];
    let mut position = 4_800.0f64;
    while (position as usize) + 256 < frames {
        let start = position as usize;
        for i in 0..192 {
            let decay = 1.0 - i as f32 / 192.0;
            samples[start + i] = decay * if i % 2 == 0 { 0.8 } else { -0.8 };
        }
        position += period;
    }
    let data_bytes = frames * 2 * 2;
    let mut wav = Vec::with_capacity(44 + data_bytes);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for sample in &samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        wav.extend_from_slice(&value.to_le_bytes());
        wav.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, wav).expect("write the click track");
}

#[test]
fn a_beat_goes_where_the_play_head_is() {
    // The button between the arrows: the grid moves so that one of its beats sits exactly on the frame the
    // DJ is listening at. The tempo, the counting and the bar stay as they were — only the place changes.
    let grid = dj_domain::BeatGrid::single(120.0, 4_800)
        .expect("a grid")
        .with_beats_per_bar(4);
    let period = f64::from(RATE) * 60.0 / grid.bpm();
    let target = 4_800 + (period / 2.0) as u64 + 137;
    let moved = dj_engine::edit_grid(&grid, &dj_domain::GridEdit::Here { frame: target }, RATE)
        .expect("the grid takes the frame");
    assert_eq!(
        moved.bpm(),
        grid.bpm(),
        "the tempo is not the question here"
    );
    assert_eq!(moved.beats_per_bar(), grid.beats_per_bar(), "nor the bar");
    let place = (target as f64 - moved.first_beat_frame() as f64) / period;
    let error = (place - place.round()).abs() * period;
    assert!(
        error <= 1.0,
        "the nearest beat is {error:.1} frames from the play head"
    );
    assert_ne!(
        moved.first_beat_frame(),
        grid.first_beat_frame(),
        "the grid did not move"
    );
}

#[test]
fn the_loader_measures_the_grid_of_a_file() {
    let dir = scratch("measure");
    let track = dir.join("clicks.wav");
    write_click_track(&track, 16.0);

    let prepared = dj_engine::prepare_track(&track, RATE, 2.0, 1.0, None).expect("prepared");
    let grid = prepared.grid.expect("a measured grid");
    assert!(
        (grid.bpm() - 128.0).abs() < 0.5,
        "measured {} BPM",
        grid.bpm()
    );
    let published = prepared
        .static_info
        .track
        .as_ref()
        .expect("track description");
    assert_eq!(
        published.bpm,
        Some(grid.bpm()),
        "the snapshot carries the tempo"
    );
    assert_eq!(
        published.first_beat_frame,
        Some(grid.first_beat_frame()),
        "and the anchor the beat lines are drawn from"
    );
    assert_eq!(
        published.grid.as_ref().map(|held| held.segments().len()),
        Some(1),
        "a track that holds its tempo is one segment"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_grid_is_stored_and_a_stored_grid_is_not_overwritten() {
    let dir = scratch("store");
    let track = dir.join("clicks.wav");
    write_click_track(&track, 16.0);
    let store = dj_library::Store::new(dir.join("mirror"));

    let written = dj_engine::meta::write_beatgrid(
        &store,
        "vol",
        &dir,
        &track,
        &dj_domain::BeatGrid::single(128.0, 4_800).expect("a grid"),
        false,
    )
    .expect("write");
    assert!(written, "an empty record takes the grid");

    let loaded = store.load("vol", &dir, &track).expect("load");
    let grid = loaded.meta.beatgrid.expect("a stored grid");
    assert!((grid.bpm - 128.0).abs() < 1e-9);
    assert_eq!(grid.first_beat_frame, 4_800);

    // The same file measured again must not lose the grid the DJ may have adjusted.
    let again = dj_engine::meta::write_beatgrid(
        &store,
        "vol",
        &dir,
        &track,
        &dj_domain::BeatGrid::single(127.0, 0).expect("a grid"),
        false,
    )
    .expect("second write");
    assert!(!again, "a stored grid is a decision, not a cache");
    let loaded = store.load("vol", &dir, &track).expect("load again");
    let grid = loaded.meta.beatgrid.expect("the grid is still there");
    assert!((grid.bpm - 128.0).abs() < 1e-9, "the stored tempo survived");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_measurement_the_dj_asked_for_replaces_the_stored_grid() {
    let dir = scratch("force");
    let track = dir.join("clicks.wav");
    write_click_track(&track, 16.0);
    let store = dj_library::Store::new(dir.join("mirror"));
    let grid = dj_domain::BeatGrid::single(128.0, 4_800).expect("a grid");
    dj_engine::meta::write_beatgrid(&store, "vol", &dir, &track, &grid, false).expect("first");

    // The button says: measure it again. That is the DJ overruling the file, so the answer lands even
    // though a grid is already there.
    let replaced = dj_engine::meta::write_beatgrid(
        &store,
        "vol",
        &dir,
        &track,
        &dj_domain::BeatGrid::single(124.0, 9_000).expect("a grid"),
        true,
    )
    .expect("replace");
    assert!(replaced, "an asked-for measurement is written");
    let loaded = store.load("vol", &dir, &track).expect("load");
    let grid = loaded.meta.beatgrid.expect("a grid");
    assert!(
        (grid.bpm - 124.0).abs() < 1e-9,
        "the new tempo: {}",
        grid.bpm
    );
    assert_eq!(grid.first_beat_frame, 9_000);
    // A grid the DJ asked for is locked against the next automatic measurement: a hand edit outranks a
    // detector, and this flag is the only thing that remembers it after a restart.
    assert!(
        loaded.meta.analysis_lock,
        "a hand edit must lock the grid against re-measurement"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stored_grid_comes_back_on_the_deck() {
    let dir = scratch("restore");
    let track = dir.join("clicks.wav");
    write_click_track(&track, 16.0);
    let store = dj_library::Store::new(dir.join("mirror"));
    dj_engine::meta::write_beatgrid(
        &store,
        "vol",
        &dir,
        &track,
        &dj_domain::BeatGrid::single(128.25, 5_000).expect("a grid"),
        false,
    )
    .expect("write");

    // A fresh load: the measurement may disagree, the stored grid must win.
    let mut prepared = dj_engine::prepare_track(&track, RATE, 2.0, 1.0, None).expect("prepared");
    let problem = dj_engine::meta::restore(&mut prepared, &store, "vol", &dir);
    assert!(problem.is_none(), "unexpected problem: {problem:?}");
    let grid = prepared.grid.expect("a grid");
    assert!(
        (grid.bpm() - 128.25).abs() < 1e-9,
        "stored tempo: {}",
        grid.bpm()
    );
    assert_eq!(grid.first_beat_frame(), 5_000);
    let published = prepared
        .static_info
        .track
        .as_ref()
        .expect("track description");
    assert_eq!(published.bpm, Some(128.25));
    assert_eq!(published.first_beat_frame, Some(5_000));
    let _ = std::fs::remove_dir_all(&dir);
}
