//! Playlists in the index (DJP-MEDIA-004).
//!
//! The storage half of the feature: a playlist is a name, a folder and an ordered list of tracks, and
//! everything a DJ does to it — make, rename, move, fill, reorder, clean up, delete — ends in the same
//! shape. The file mirror and the protocol are other tests' business.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use dj_library::{Index, PlaylistTrackRow};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-playlists-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn track(fingerprint: &str) -> PlaylistTrackRow {
    PlaylistTrackRow {
        fingerprint: fingerprint.to_string(),
        path: format!("/music/{fingerprint}.wav"),
    }
}

#[test]
fn a_playlist_can_be_made_renamed_moved_and_deleted() {
    let dir = scratch("crud");
    let mut index = Index::open(&dir.join("library.db")).expect("open");

    let id = index.playlist_create("Warm up", "", 1_000).expect("create");
    assert_eq!(index.playlists().expect("list").len(), 1);
    let made = index.playlists().expect("list").remove(0);
    assert_eq!(made.name, "Warm up");
    assert_eq!(made.folder, "");
    assert_eq!(made.revision, 1);

    index.playlist_folder_create("Sets/Deep").expect("folder");
    index.playlist_rename(id, "Opening", 2_000).expect("rename");
    index.playlist_move(id, "Sets/Deep", 3_000).expect("move");
    let moved = index.playlists().expect("list").remove(0);
    assert_eq!(moved.name, "Opening");
    assert_eq!(moved.folder, "Sets/Deep");
    assert_eq!(moved.revision, 3, "every change moves the revision on");
    assert_eq!(moved.updated_at_ms, 3_000);

    index.playlist_delete(id).expect("delete");
    assert!(index.playlists().expect("list").is_empty());
    assert!(index.playlist_tracks(id).expect("tracks").is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_folder_is_a_path_and_deleting_it_keeps_the_playlists() {
    let dir = scratch("folders");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    index.playlist_folder_create("Sets").expect("folder");
    index.playlist_folder_create("Sets/Deep").expect("folder");
    let deep = index
        .playlist_create("Deep house", "Sets/Deep", 1_000)
        .expect("create");

    index
        .playlist_folder_rename("Sets", "Live")
        .expect("rename folder");
    let folders = index.playlist_folders().expect("folders");
    assert!(folders.contains(&"Live/Deep".to_string()), "{folders:?}");
    assert_eq!(index.playlists().expect("list")[0].folder, "Live/Deep");

    // Deleting a folder is deleting the arrangement, not the records: the playlist comes back to the root.
    index.playlist_folder_delete("Live").expect("delete folder");
    assert!(index.playlist_folders().expect("folders").is_empty());
    let kept = index.playlists().expect("list");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, deep);
    assert_eq!(kept[0].folder, "");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_rows_of_a_playlist_keep_the_order_they_were_given() {
    let dir = scratch("order");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    let id = index.playlist_create("Set", "", 1_000).expect("create");

    index
        .playlist_set_tracks(id, &[track("one"), track("two"), track("three")], 2_000)
        .expect("fill");
    let rows = index.playlist_tracks(id).expect("tracks");
    assert_eq!(
        rows.iter()
            .map(|row| row.fingerprint.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two", "three"]
    );

    // A reorder is a new list, and the list is what is stored.
    index
        .playlist_set_tracks(id, &[track("three"), track("one"), track("two")], 3_000)
        .expect("reorder");
    let rows = index.playlist_tracks(id).expect("tracks");
    assert_eq!(
        rows.iter()
            .map(|row| row.fingerprint.as_str())
            .collect::<Vec<_>>(),
        vec!["three", "one", "two"]
    );

    // Removing the middle one leaves the rest in order.
    index
        .playlist_set_tracks(id, &[track("three"), track("two")], 4_000)
        .expect("remove");
    assert_eq!(index.playlist_tracks(id).expect("tracks").len(), 2);
    assert_eq!(index.playlists().expect("list")[0].revision, 4);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn duplicates_are_the_same_sound_and_go_with_their_place_kept() {
    let dir = scratch("dups");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    let id = index.playlist_create("Set", "", 1_000).expect("create");
    index
        .playlist_set_tracks(
            id,
            &[
                track("one"),
                track("two"),
                track("one"),
                track("three"),
                track("two"),
            ],
            2_000,
        )
        .expect("fill");

    let removed = index.playlist_remove_duplicates(id, 3_000).expect("dups");
    assert_eq!(removed, 2, "two rows repeated an earlier track");
    let rows = index.playlist_tracks(id).expect("tracks");
    assert_eq!(
        rows.iter()
            .map(|row| row.fingerprint.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two", "three"],
        "the first of each stays, and the order of the rest does not move"
    );

    // A list with nothing to clean is left untouched: no revision moves for nothing.
    let before = index.playlists().expect("list")[0].revision;
    assert_eq!(
        index.playlist_remove_duplicates(id, 4_000).expect("dups"),
        0
    );
    assert_eq!(index.playlists().expect("list")[0].revision, before);
    std::fs::remove_dir_all(&dir).ok();
}
