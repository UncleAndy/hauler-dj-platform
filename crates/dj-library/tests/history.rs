//! The journal of plays in the index (DJP-MEDIA-004).
//!
//! The engine decides *when* a play happened (a continuous interval of playing, long enough to count);
//! the index decides what it keeps. These tests are about the second half: a row in `plays`, and the
//! summary on the track the collection sorts by.

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use dj_library::{Index, PlayRecord, TrackRow};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-history-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn row(fingerprint: &str, path: &str) -> TrackRow {
    TrackRow {
        fingerprint: fingerprint.to_string(),
        path: path.to_string(),
        artist: String::new(),
        title: String::new(),
        album: String::new(),
        genre: String::new(),
        year: None,
        bpm: None,
        musical_key: None,
        rating: 0,
        duration_frames: None,
        sample_rate: None,
        gain: 1.0,
        play_count: 0,
        last_played_ms: None,
        analysis_lock: false,
    }
}

fn play(fingerprint: &str, path: &str, deck: u8, started_at_ms: u64, seconds: f64) -> PlayRecord {
    PlayRecord {
        fingerprint: fingerprint.to_string(),
        path: path.to_string(),
        deck,
        started_at_ms,
        seconds,
    }
}

#[test]
fn a_play_lands_in_the_journal_and_in_the_summary() {
    let dir = scratch("one-play");
    let path = dir.join("library.db");
    let mut index = Index::open(&path).expect("open");
    index.upsert(&row("fp-a", "/music/a.wav")).expect("row");

    index
        .record_play(&play("fp-a", "/music/a.wav", 1, 1_700_000_000_000, 42.5))
        .expect("record");

    let journal = index.plays(10).expect("plays");
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].deck, 1);
    assert!((journal[0].seconds - 42.5).abs() < 1e-9);
    assert_eq!(journal[0].started_at_ms, 1_700_000_000_000);

    // The summary the collection sorts by: how many times, and when last.
    let track = index
        .by_fingerprint("fp-a")
        .expect("track lookup")
        .expect("the row is there");
    assert_eq!(track.play_count, 1);
    assert_eq!(track.last_played_ms, Some(1_700_000_000_000));
}

#[test]
fn two_plays_of_one_track_are_two_rows_and_a_count_of_two() {
    let dir = scratch("two-plays");
    let path = dir.join("library.db");
    let mut index = Index::open(&path).expect("open");
    index.upsert(&row("fp-b", "/music/b.wav")).expect("row");

    index
        .record_play(&play("fp-b", "/music/b.wav", 0, 1_000, 31.0))
        .expect("first");
    index
        .record_play(&play("fp-b", "/music/b.wav", 0, 2_000, 60.0))
        .expect("second");

    let journal = index.plays(10).expect("plays");
    assert_eq!(journal.len(), 2, "the journal keeps every play");
    assert_eq!(journal[0].started_at_ms, 2_000, "newest first");
    let track = index
        .by_fingerprint("fp-b")
        .expect("track lookup")
        .expect("the row is there");
    assert_eq!(track.play_count, 2);
    assert_eq!(
        track.last_played_ms,
        Some(2_000),
        "the summary keeps the last one"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_track_the_index_does_not_know_keeps_its_journal_row() {
    let dir = scratch("stranger");
    let path = dir.join("library.db");
    let mut index = Index::open(&path).expect("open");

    // Played from the files section: no row in the collection, and still a play in the journal.
    index
        .record_play(&play("fp-stranger", "/elsewhere/x.wav", 2, 5_000, 33.0))
        .expect("record");

    assert_eq!(index.plays(10).expect("plays").len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}
