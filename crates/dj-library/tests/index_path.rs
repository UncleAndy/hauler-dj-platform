//! Where the database lives: one door, one path, and the folders made on the way.
//!
//! The database is a cache over the `.djmeta` records, so the door must not be a decision a caller
//! makes — every writer and every reader has to land on the same file.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use dj_library::{Index, TrackRow};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-index-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    root
}

fn row() -> TrackRow {
    TrackRow {
        fingerprint: "1:a:b".to_owned(),
        path: "/music/A/T/One.flac".to_owned(),
        artist: "A".to_owned(),
        title: "One".to_owned(),
        album: "T".to_owned(),
        genre: String::new(),
        year: None,
        bpm: None,
        musical_key: None,
        rating: 0,
        duration_frames: None,
        gain: 1.0,
        play_count: 0,
        last_played_ms: None,
        analysis_lock: false,
    }
}

#[test]
fn the_door_makes_the_folders_and_the_second_open_finds_the_same_database() {
    let dir = scratch("door");
    let mut index = Index::open_in(&dir).expect("open");
    let path = dir.join("hauler-dj").join("db").join("library.db");
    assert!(
        path.is_file(),
        "база должна появиться по пути двери: {}",
        path.display()
    );
    index.upsert(&row()).expect("write");
    drop(index);

    let again = Index::open_in(&dir).expect("second open");
    assert_eq!(
        again.count().expect("count"),
        1,
        "второй вызов открывает ту же базу, а не новую"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_data_folder_follows_the_environment() {
    // An absolute `XDG_DATA_HOME` wins; a relative one is ignored, as the spec says; without either
    // there is no folder to name.
    assert_eq!(
        dj_library::index::data_folder_from(Some(Path::new("/tmp/x")), Some(Path::new("/home/u"))),
        Some(PathBuf::from("/tmp/x"))
    );
    assert_eq!(
        dj_library::index::data_folder_from(
            Some(Path::new("relative")),
            Some(Path::new("/home/u"))
        ),
        Some(PathBuf::from("/home/u/.local/share"))
    );
    assert_eq!(
        dj_library::index::data_folder_from(None, Some(Path::new("/home/u"))),
        Some(PathBuf::from("/home/u/.local/share"))
    );
    assert_eq!(dj_library::index::data_folder_from(None, None), None);
}
