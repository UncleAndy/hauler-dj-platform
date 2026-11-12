//! Rebuilding the index by walking the collection: the `.djmeta` records are the source, the database
//! is a cache that can be thrown away and made again.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use dj_library::{Index, TrackMeta, fingerprint};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-rebuild-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    root
}

/// A record beside a track. The rebuild reads the metadata, not the audio, so the bytes do not matter —
/// only the fact that a file is there to hang the record on.
fn record(root: &Path, name: &str, genres: &[&str]) -> PathBuf {
    let track = root.join("A").join("T").join(name);
    std::fs::create_dir_all(track.parent().expect("parent")).expect("dirs");
    // Своё содержимое у каждого файла: одинаковые байты — один отпечаток, а значит одна строка.
    std::fs::write(&track, format!("not audio: {name}")).expect("track");
    let mut meta = TrackMeta::new(fingerprint::of_path(&track).expect("fingerprint"), 1_000);
    meta.tags = Some(dj_library::meta::FileTags {
        artist: "A".to_owned(),
        title: name.trim_end_matches(".wav").to_owned(),
        album: "T".to_owned(),
        genres: genres.iter().map(|genre| (*genre).to_owned()).collect(),
    });
    let path = dj_library::sidecar::file_of(&track).expect("record path");
    dj_library::sidecar::write(&path, &meta).expect("record");
    track
}

#[test]
fn a_rebuild_reads_the_records_and_their_genres() {
    let dir = scratch("three");
    record(&dir, "One.wav", &["House", "Techno"]);
    record(&dir, "Two.wav", &["House"]);
    record(&dir, "Three.wav", &[]);
    let mut index = Index::open(&dir.join("library.db")).expect("index");

    let report = index
        .rebuild_from(std::slice::from_ref(&dir))
        .expect("rebuild");
    assert_eq!(report.tracks, 3);
    assert_eq!(report.genres, 3, "две связи у первого и одна у второго");
    assert_eq!(index.count().expect("count"), 3);
    assert_eq!(
        index
            .tracks_with_genres(&["house".to_owned()])
            .expect("filter")
            .len(),
        2
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_second_rebuild_leaves_the_same_rows() {
    let dir = scratch("again");
    record(&dir, "One.wav", &["House"]);
    record(&dir, "Two.wav", &["Techno"]);
    let mut index = Index::open(&dir.join("library.db")).expect("index");
    index
        .rebuild_from(std::slice::from_ref(&dir))
        .expect("first");
    let before = index.genre_list().expect("list");
    let report = index
        .rebuild_from(std::slice::from_ref(&dir))
        .expect("second");
    assert_eq!(report.tracks, 2);
    assert_eq!(index.count().expect("count"), 2);
    assert_eq!(index.genre_list().expect("list"), before);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_broken_record_is_skipped_and_the_rest_are_kept() {
    let dir = scratch("broken");
    record(&dir, "One.wav", &["House"]);
    let broken = dir
        .join("A")
        .join("T")
        .join(dj_library::sidecar::DIRECTORY)
        .join("Broken.wav.json");
    std::fs::write(&broken, b"{ not json").expect("broken");
    let mut index = Index::open(&dir.join("library.db")).expect("index");
    let report = index
        .rebuild_from(std::slice::from_ref(&dir))
        .expect("rebuild");
    assert_eq!(report.tracks, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(index.count().expect("count"), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_rebuild_ignores_the_duplicates_folder() {
    // `_duplicates/` is not a collection: a record there must not become a row.
    let dir = scratch("dups");
    record(&dir, "One.wav", &["House"]);
    let loser = dir.join("A").join("T").join("_duplicates").join("Two.wav");
    std::fs::create_dir_all(loser.parent().expect("parent")).expect("dirs");
    std::fs::write(&loser, b"the copy that lost").expect("loser");
    let mut meta = TrackMeta::new(fingerprint::of_path(&loser).expect("fingerprint"), 1_000);
    meta.tags = Some(dj_library::meta::FileTags {
        artist: "A".to_owned(),
        title: "Two".to_owned(),
        album: "T".to_owned(),
        genres: vec!["Techno".to_owned()],
    });
    let path = dj_library::sidecar::file_of(&loser).expect("record path");
    dj_library::sidecar::write(&path, &meta).expect("record");

    let mut index = Index::open(&dir.join("library.db")).expect("index");
    let report = index
        .rebuild_from(std::slice::from_ref(&dir))
        .expect("rebuild");
    assert_eq!(report.tracks, 1, "проигравший не строка коллекции");
    std::fs::remove_dir_all(&dir).ok();
}
