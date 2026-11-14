//! The playlist mirror on disk (DJP-MEDIA-004).
//!
//! The database is the truth; the file is how a playlist outlives it. These tests are about the file: it
//! carries the whole playlist, a person can read it, its name survives a filesystem, and a file of a
//! playlist that is gone does not stay behind pretending otherwise.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use dj_library::playlist_file::{
    FORMAT, PlaylistFile, PlaylistFileTrack, files, mirror_root, path_of, read, sync, write,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-playlist-file-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn playlist(id: i64, name: &str, folder: &str, tracks: &[&str]) -> PlaylistFile {
    PlaylistFile {
        format: FORMAT.to_string(),
        id,
        name: name.to_string(),
        folder: folder.to_string(),
        revision: 3,
        updated_at_ms: 1_794_000_000_000,
        tracks: tracks
            .iter()
            .map(|fingerprint| PlaylistFileTrack {
                fingerprint: fingerprint.to_string(),
                path: format!("/music/{fingerprint}.wav"),
            })
            .collect(),
    }
}

#[test]
fn a_playlist_round_trips_through_its_file() {
    let dir = scratch("round-trip");
    let root = mirror_root(&dir);
    let wanted = playlist(7, "Warm up", "Sets/Deep", &["one", "two", "three"]);
    let path = path_of(&root, &wanted.folder, &wanted.name);

    write(&path, &wanted).expect("write");
    let back = read(&path).expect("read");
    assert_eq!(
        back, wanted,
        "the file is the whole playlist, not a summary of it"
    );
    assert_eq!(back.tracks.len(), 3);
    assert_eq!(back.tracks[2].fingerprint, "three");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_name_that_cannot_be_a_file_name_is_repaired_and_the_folder_is_a_folder() {
    let dir = scratch("names");
    let root = mirror_root(&dir);
    let awkward = playlist(1, "Deep/House: set?", "Sets/Deep", &[]);
    let path = path_of(&root, &awkward.folder, &awkward.name);
    assert!(!path.to_string_lossy().contains("Deep/House"), "{path:?}");
    assert_eq!(
        path.file_name().expect("file name").to_string_lossy(),
        "Deep_House_ set_.djplaylist.json"
    );
    assert!(
        path.parent().expect("parent").ends_with("Sets/Deep"),
        "{path:?}"
    );

    write(&path, &awkward).expect("write");
    let found = files(&root).expect("files");
    assert_eq!(found, vec![path]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn two_playlists_that_repair_to_one_name_get_a_suffix() {
    let dir = scratch("suffix");
    let root = mirror_root(&dir);
    let written = sync(
        &root,
        &[
            playlist(1, "Set:1", "Sets", &["one"]),
            playlist(2, "Set/1", "Sets", &["two"]),
        ],
    )
    .expect("sync");
    assert_eq!(written, 2);
    let found = files(&root).expect("files");
    assert_eq!(found.len(), 2, "{found:?}");
    let names: Vec<String> = found
        .iter()
        .map(|path| {
            path.file_name()
                .expect("name")
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert!(
        names.contains(&"Set_1.djplaylist.json".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"Set_1 (2).djplaylist.json".to_string()),
        "{names:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_mirror_drops_the_files_of_playlists_that_are_gone() {
    let dir = scratch("gone");
    let root = mirror_root(&dir);
    sync(
        &root,
        &[
            playlist(1, "Keep", "", &["one"]),
            playlist(2, "Gone", "", &["two"]),
        ],
    )
    .expect("first");
    assert_eq!(files(&root).expect("files").len(), 2);

    // The second sync is the database as it is now: one playlist, renamed as well.
    sync(&root, &[playlist(1, "Kept", "", &["one"])]).expect("second");
    let found = files(&root).expect("files");
    assert_eq!(
        found.len(),
        1,
        "the file of a deleted playlist does not linger: {found:?}"
    );
    assert!(found[0].ends_with("Kept.djplaylist.json"), "{found:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_file_of_another_format_is_refused_by_name() {
    let dir = scratch("format");
    let root = mirror_root(&dir);
    let path = path_of(&root, "", "Strange");
    std::fs::create_dir_all(&root).expect("mirror");
    std::fs::write(&path, "{\"format\":\"djplaylist-v9\",\"name\":\"Strange\"}").expect("write");
    let error = read(&path).expect_err("another format is not a playlist");
    let message = error.to_string();
    assert!(message.contains("Strange"), "{message}");
    assert!(message.contains("djplaylist-v9"), "{message}");
    std::fs::remove_dir_all(&dir).ok();
}
