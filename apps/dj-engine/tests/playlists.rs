//! The playlists, engine side (DJP-MEDIA-004).
//!
//! Two marks per edit — the row in the index and the file in the mirror — and the rules the owner set:
//! the database is the truth a session works from, the file is how a playlist outlives it, a file the index
//! does not know is a playlist to restore, and a file that is newer wins by its revision.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use dj_domain::{PlaylistAdd, PlaylistOp, PlaylistQuery};
use dj_engine::playlists;
use dj_library::Index;
use dj_library::playlist_file;

struct Stand {
    folder: PathBuf,
    mirror: PathBuf,
}

fn stand(name: &str) -> Stand {
    let folder = std::env::temp_dir().join(format!("dj-playlists-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&folder).ok();
    std::fs::create_dir_all(&folder).expect("scratch");
    let mirror = playlist_file::mirror_root(&folder);
    Stand { folder, mirror }
}

impl Stand {
    fn index(&self) -> Index {
        Index::open(&self.folder.join("library.sqlite")).expect("index")
    }

    fn files(&self) -> Vec<String> {
        playlist_file::files(&self.mirror)
            .expect("mirror")
            .iter()
            .map(|path| {
                path.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .to_string()
            })
            .collect()
    }

    fn cleanup(&self) {
        std::fs::remove_dir_all(&self.folder).ok();
    }
}

fn add(fingerprint: &str) -> PlaylistAdd {
    PlaylistAdd {
        fingerprint: fingerprint.to_string(),
        path: format!("/music/{fingerprint}.wav"),
    }
}

fn fingerprints(index: &Index, playlist: i64) -> Vec<String> {
    index
        .playlist_tracks(playlist)
        .expect("tracks")
        .into_iter()
        .map(|row| row.fingerprint)
        .collect()
}

fn only_playlist(index: &Index) -> i64 {
    let all = index.playlists().expect("playlists");
    assert_eq!(all.len(), 1, "one playlist was asked for");
    all[0].id
}

#[test]
fn a_playlist_is_written_to_the_index_and_to_its_file() {
    let stand = stand("both");
    let mut index = stand.index();
    let tree = playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Warm up".to_string(),
            folder: "Sets".to_string(),
        },
        1_794_000_000_000,
    )
    .expect("create");
    assert_eq!(tree.playlists.len(), 1);
    assert_eq!(tree.folders, vec!["Sets".to_string()]);
    let id = only_playlist(&index);
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::AddTracks {
            playlist: id,
            tracks: vec![add("one"), add("two"), add("three")],
        },
        1_794_000_001_000,
    )
    .expect("add");

    assert_eq!(fingerprints(&index, id), vec!["one", "two", "three"]);
    assert_eq!(stand.files(), vec!["Warm up.djplaylist.json".to_string()]);
    let file = playlist_file::read(&playlist_file::path_of(&stand.mirror, "Sets", "Warm up"))
        .expect("file");
    assert_eq!(file.tracks.len(), 3, "{}", file.tracks.len());
    assert_eq!(file.revision, 2, "the file keeps step with the index");
    stand.cleanup();
}

#[test]
fn renaming_a_playlist_moves_its_file_and_leaves_nothing_behind() {
    let stand = stand("rename");
    let mut index = stand.index();
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Old".to_string(),
            folder: String::new(),
        },
        1,
    )
    .expect("create");
    let id = only_playlist(&index);
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Rename {
            playlist: id,
            name: "New".to_string(),
        },
        2,
    )
    .expect("rename");
    assert_eq!(stand.files(), vec!["New.djplaylist.json".to_string()]);
    stand.cleanup();
}

#[test]
fn dragging_a_row_up_changes_the_order_in_the_index_and_in_the_file() {
    let stand = stand("reorder");
    let mut index = stand.index();
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Set".to_string(),
            folder: String::new(),
        },
        1,
    )
    .expect("create");
    let id = only_playlist(&index);
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::AddTracks {
            playlist: id,
            tracks: vec![add("a"), add("b"), add("c")],
        },
        2,
    )
    .expect("add");
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Reorder {
            playlist: id,
            from: 2,
            to: 0,
        },
        3,
    )
    .expect("reorder");

    assert_eq!(fingerprints(&index, id), vec!["c", "a", "b"]);
    let file =
        playlist_file::read(&playlist_file::path_of(&stand.mirror, "", "Set")).expect("file");
    let on_disk: Vec<String> = file
        .tracks
        .iter()
        .map(|track| track.fingerprint.clone())
        .collect();
    assert_eq!(on_disk, vec!["c", "a", "b"], "the file is the same list");
    stand.cleanup();
}

#[test]
fn remove_dups_keeps_the_first_of_each_track() {
    let stand = stand("dups");
    let mut index = stand.index();
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Set".to_string(),
            folder: String::new(),
        },
        1,
    )
    .expect("create");
    let id = only_playlist(&index);
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::AddTracks {
            playlist: id,
            tracks: vec![add("a"), add("b"), add("a"), add("a"), add("b")],
        },
        2,
    )
    .expect("add");
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::RemoveDuplicates { playlist: id },
        3,
    )
    .expect("dups");
    assert_eq!(fingerprints(&index, id), vec!["a", "b"]);
    stand.cleanup();
}

#[test]
fn deleting_a_playlist_takes_its_file_with_it() {
    let stand = stand("delete");
    let mut index = stand.index();
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Gone".to_string(),
            folder: "Sets".to_string(),
        },
        1,
    )
    .expect("create");
    let id = only_playlist(&index);
    assert_eq!(stand.files().len(), 1);
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Delete { playlist: id },
        2,
    )
    .expect("delete");
    assert!(stand.files().is_empty(), "{:?}", stand.files());
    stand.cleanup();
}

#[test]
fn a_wiped_index_comes_back_from_the_mirror() {
    let stand = stand("restore");
    let mut index = stand.index();
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Friday".to_string(),
            folder: "Sets/Deep".to_string(),
        },
        10,
    )
    .expect("create");
    let id = only_playlist(&index);
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::AddTracks {
            playlist: id,
            tracks: vec![add("first"), add("second")],
        },
        20,
    )
    .expect("add");
    drop(index);

    // The database is gone; the mirror is what is left of the DJ's work.
    std::fs::remove_file(stand.folder.join("library.sqlite")).expect("remove");
    let mut fresh = stand.index();
    assert!(fresh.playlists().expect("playlists").is_empty());
    let tree = playlists::startup_pass(&mut fresh, &stand.mirror).expect("startup");

    assert_eq!(tree.playlists.len(), 1, "the playlist came back");
    assert_eq!(tree.playlists[0].name, "Friday");
    assert_eq!(tree.playlists[0].folder, "Sets/Deep");
    assert_eq!(tree.playlists[0].tracks, 2);
    let back = only_playlist(&fresh);
    assert_eq!(fingerprints(&fresh, back), vec!["first", "second"]);
    stand.cleanup();
}

#[test]
fn a_file_newer_than_the_row_wins_the_argument() {
    let stand = stand("newer");
    let mut index = stand.index();
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Set".to_string(),
            folder: String::new(),
        },
        1,
    )
    .expect("create");
    let id = only_playlist(&index);
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::AddTracks {
            playlist: id,
            tracks: vec![add("old")],
        },
        2,
    )
    .expect("add");

    // A hand edit, a copy from another machine: the file is ahead of the row.
    let path = playlist_file::path_of(&stand.mirror, "", "Set");
    let mut file = playlist_file::read(&path).expect("file");
    file.revision += 10;
    file.tracks = vec![
        playlist_file::PlaylistFileTrack {
            fingerprint: "edited".to_string(),
            path: "/music/edited.wav".to_string(),
        },
        playlist_file::PlaylistFileTrack {
            fingerprint: "again".to_string(),
            path: "/music/again.wav".to_string(),
        },
    ];
    playlist_file::write(&path, &file).expect("write");

    let tree = playlists::startup_pass(&mut index, &stand.mirror).expect("startup");
    assert_eq!(tree.playlists[0].revision, file.revision);
    assert_eq!(fingerprints(&index, id), vec!["edited", "again"]);
    stand.cleanup();
}

#[test]
fn a_page_of_a_playlist_keeps_the_rows_whose_files_have_gone() {
    let stand = stand("page");
    let mut index = stand.index();
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::Create {
            name: "Set".to_string(),
            folder: String::new(),
        },
        1,
    )
    .expect("create");
    let id = only_playlist(&index);
    // One of them was never imported: the row stays, with an empty path, so the DJ sees what he lost.
    playlists::apply(
        &mut index,
        &stand.mirror,
        &PlaylistOp::AddTracks {
            playlist: id,
            tracks: vec![add("lost"), add("known")],
        },
        2,
    )
    .expect("add");
    index
        .upsert(&dj_library::TrackRow {
            fingerprint: "known".to_string(),
            path: "/music/known.wav".to_string(),
            artist: "Artist".to_string(),
            title: "Known".to_string(),
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
            variable_bpm: false,
        })
        .expect("a track the index knows");

    let page = playlists::page(
        &index,
        id,
        &PlaylistQuery {
            id: 5,
            playlist: id,
            search: None,
            offset: 0,
            limit: 10,
        },
    )
    .expect("page");
    assert_eq!(page.id, 5, "the answer carries the question's marker");
    assert_eq!(page.total, 2);
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.rows[0].path, "/music/lost.wav");
    assert_eq!(page.rows[0].fingerprint, "lost");
    assert_eq!(page.rows[1].path, "/music/known.wav");
    stand.cleanup();
}
