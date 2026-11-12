//! A08: the metadata travels with the music, and the index is only a cache over it.
//!
//! Every test here is one of the acceptance criteria of the task, written so that it fails if the
//! promise breaks: copy a folder, delete the database, rename a file, mount a volume read-only, or
//! hand the reader a broken file.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use dj_library::meta::Cue;
use dj_library::store::{Source, Store};
use dj_library::{Index, TrackMeta, TrackRow, fingerprint};

/// A directory of its own for one test: the tests run in parallel and share nothing.
fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-lib-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A file with stable bytes, standing in for a track. The name goes into the bytes: two tracks with
/// identical content are the same track, and a fixture that ignored this would hide real behaviour.
fn track(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let seed = name.bytes().map(u32::from).sum::<u32>();
    let bytes: Vec<u8> = (0..4096u32)
        .map(|index| ((index.wrapping_add(seed)) % 251) as u8)
        .collect();
    std::fs::write(&path, bytes).expect("write a track");
    path
}

fn record(path: &Path, revision: u64, cue: u64) -> TrackMeta {
    let fingerprint = fingerprint::of_path(path).expect("fingerprint");
    let mut meta = TrackMeta::new(fingerprint, 1_000 * revision);
    meta.revision = revision;
    meta.cues.push(Cue {
        slot: 0,
        frame: cue,
        label: Some("drop".to_owned()),
        // A pad's colour is part of the record; this one is written without it on purpose — an
        // optional field must not become a required one for older files.
        color: None,
    });
    meta
}

fn row(meta: &TrackMeta, path: &Path, title: &str) -> TrackRow {
    TrackRow {
        fingerprint: meta.fingerprint.key(),
        path: path.display().to_string(),
        artist: "Test Artist".to_owned(),
        title: title.to_owned(),
        album: "Test Album".to_owned(),
        genre: "House".to_owned(),
        year: Some(2026),
        bpm: Some(128.0),
        musical_key: Some("8A".to_owned()),
        rating: 4,
        duration_frames: Some(8_000_000),
        sample_rate: None,
        gain: meta.gain,
        play_count: 0,
        last_played_ms: None,
        analysis_lock: meta.analysis_lock,
    }
}

#[test]
fn cue_points_travel_with_a_copied_folder() {
    let source = workspace("travel-src");
    let music = track(&source, "Song.flac");
    let store = Store::new(workspace("travel-mirror"));
    let meta = record(&music, 3, 96_000);
    store.save("vol1", &source, &music, &meta).expect("save");

    // Another machine, another home: only the folder was copied.
    let copy = workspace("travel-dst");
    copy_tree(&source, &copy);
    let copied = copy.join("Song.flac");
    let elsewhere = Store::new(workspace("travel-mirror-2"));
    let loaded = elsewhere.load("vol1", &copy, &copied).expect("load");
    assert_eq!(loaded.source, Source::Sidecar);
    assert_eq!(
        loaded.meta.cues, meta.cues,
        "the cue must arrive with the file"
    );
    assert_eq!(loaded.meta.revision, 3);
}

#[test]
fn a_renamed_file_keeps_its_metadata() {
    let root = workspace("rename");
    let music = track(&root, "Old.flac");
    let store = Store::new(workspace("rename-mirror"));
    store
        .save("vol1", &root, &music, &record(&music, 2, 48_000))
        .expect("save");

    let renamed = root.join("New.flac");
    std::fs::rename(&music, &renamed).expect("rename");

    // The bytes did not change, so the fingerprint did not either.
    let before = fingerprint::of_path(&renamed).expect("fingerprint");
    assert_eq!(before.key(), record(&renamed, 2, 48_000).fingerprint.key());

    let adopted = store.adopt(&renamed).expect("adopt").expect("found");
    assert!(adopted.ends_with(".djmeta/New.flac.json"), "{adopted:?}");
    let loaded = store.load("vol1", &root, &renamed).expect("load");
    assert_eq!(loaded.meta.cues.len(), 1);

    // And the index still holds one row, keyed by the bytes rather than by the name.
    let mut index = Index::open(&workspace("rename-index").join("library.db")).expect("open");
    index
        .upsert(&row(&loaded.meta, &renamed, "New"))
        .expect("upsert");
    assert_eq!(index.count().expect("count"), 1);
    index
        .upsert(&row(&loaded.meta, &renamed, "New"))
        .expect("again");
    assert_eq!(index.count().expect("count"), 1, "no duplicate row");
}

#[test]
fn a_read_only_folder_falls_back_to_the_mirror() {
    let root = workspace("readonly");
    let music = track(&root, "Song.flac");
    let mirror = workspace("readonly-mirror");
    let store = Store::new(&mirror);
    let meta = record(&music, 1, 24_000);

    let permissions = std::fs::metadata(&root).expect("metadata").permissions();
    let mut readonly = permissions.clone();
    readonly.set_mode(0o555);
    std::fs::set_permissions(&root, readonly).expect("chmod");

    let saved = store.save("vol1", &root, &music, &meta).expect("save");
    assert!(!saved.sidecar, "a read-only folder cannot take a sidecar");
    assert!(saved.mirror, "the mirror must hold the data instead");

    let loaded = store.load("vol1", &root, &music).expect("load");
    assert_eq!(loaded.source, Source::Mirror);
    assert_eq!(loaded.meta.cues, meta.cues);

    std::fs::set_permissions(&root, permissions).expect("chmod back");
}

#[test]
fn a_disagreement_is_reported_rather_than_swallowed() {
    let root = workspace("conflict");
    let music = track(&root, "Song.flac");
    let store = Store::new(workspace("conflict-mirror"));

    // The mirror has the newer record: it wins, and the loser is handed back.
    store
        .save("vol1", &root, &music, &record(&music, 4, 10_000))
        .expect("save");
    let mut newer = record(&music, 5, 20_000);
    newer.updated_at_ms = 9_000;
    let mirror_path = store.mirror_path("vol1", &root, &music);
    std::fs::create_dir_all(mirror_path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&mirror_path, serde_json::to_vec(&newer).expect("serialise")).expect("write");

    let loaded = store.load("vol1", &root, &music).expect("load");
    assert_eq!(loaded.source, Source::Mirror);
    assert_eq!(loaded.meta.revision, 5);
    assert!(loaded.conflict.is_some(), "the loser must be visible");

    // Same revision and same time but different content: two edits from a common ancestor. Neither
    // may be thrown away, so the caller gets both.
    let mut twin = record(&music, 6, 30_000);
    twin.updated_at_ms = 1_000;
    std::fs::write(&mirror_path, serde_json::to_vec(&twin).expect("serialise")).expect("write");
    let same_revision = record(&music, 6, 40_000);
    std::fs::write(
        sidecar_path(&music),
        serde_json::to_vec(&same_revision).expect("serialise"),
    )
    .expect("write sidecar");
    let loaded = store.load("vol1", &root, &music).expect("load");
    let (left, right) = loaded.conflict.expect("a conflict must be reported");
    assert_ne!(left.cues, right.cues, "both versions survive");
}

#[test]
fn a_broken_sidecar_is_reported_and_the_mirror_still_answers() {
    let root = workspace("corrupt");
    let music = track(&root, "Song.flac");
    let store = Store::new(workspace("corrupt-mirror"));
    let meta = record(&music, 2, 5_000);
    store.save("vol1", &root, &music, &meta).expect("save");

    let path = sidecar_path(&music);
    std::fs::write(&path, b"{ half a file").expect("corrupt it");
    let loaded = store
        .load("vol1", &root, &music)
        .expect("the mirror answers");
    assert_eq!(loaded.source, Source::Mirror);
    assert_eq!(loaded.meta.cues, meta.cues);
    assert_eq!(
        std::fs::read(&path).expect("read"),
        b"{ half a file",
        "the broken file stays as it was"
    );
}

#[test]
fn deleting_the_index_restores_it_by_scanning() {
    let root = workspace("rebuild");
    let store = Store::new(workspace("rebuild-mirror"));
    let first = track(&root, "One.flac");
    let second = track(&root, "Two.flac");
    store
        .save("vol1", &root, &first, &record(&first, 1, 1_000))
        .expect("save one");
    store
        .save("vol1", &root, &second, &record(&second, 1, 2_000))
        .expect("save two");

    let database = workspace("rebuild-index").join("library.db");
    let mut index = Index::open(&database).expect("open");
    let scanned = store.scan(&root);
    assert_eq!(scanned.len(), 2, "both metadata files are found");
    let rows: Vec<TrackRow> = scanned
        .iter()
        .map(|(track, meta)| row(meta, track, &track.display().to_string()))
        .collect();
    assert_eq!(index.rebuild(&rows).expect("rebuild"), 2);
    assert_eq!(index.count().expect("count"), 2);
    let found = index.search("One").expect("search");
    assert_eq!(found.len(), 1, "search reads the index, not the folder");

    // The database is a cache: throw it away and a scan brings everything back.
    drop(index);
    std::fs::remove_file(&database).expect("delete the database");
    let mut fresh = Index::open(&database).expect("reopen");
    assert_eq!(fresh.count().expect("count"), 0);
    let scanned = store.scan(&root);
    let rows: Vec<TrackRow> = scanned
        .iter()
        .map(|(track, meta)| row(meta, track, &track.display().to_string()))
        .collect();
    fresh.rebuild(&rows).expect("rebuild");
    assert_eq!(fresh.count().expect("count"), 2);
}

#[test]
fn the_index_carries_no_cue_points() {
    // The promise is that cue points live in the metadata file only. If a column ever sneaks in, the
    // database stops being a cache and a rebuild starts meaning something else.
    let dir = workspace("schema");
    let index = Index::open(&dir.join("library.db")).expect("open");
    let columns = index.columns().expect("columns");
    for forbidden in ["cue", "beatgrid", "loop", "grid"] {
        assert!(
            !columns.iter().any(|column| column.contains(forbidden)),
            "the index must not hold {forbidden}: {columns:?}"
        );
    }
}

fn sidecar_path(music: &Path) -> PathBuf {
    music
        .parent()
        .expect("parent")
        .join(".djmeta")
        .join(format!(
            "{}.json",
            music.file_name().expect("name").to_string_lossy()
        ))
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("list").flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy");
        }
    }
}
