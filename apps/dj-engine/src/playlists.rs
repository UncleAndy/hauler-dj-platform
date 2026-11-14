//! The playlists, from the engine's side (DJP-MEDIA-004).
//!
//! Two marks per edit: the row in the index and the file in the mirror. The database is the truth a
//! session works from; the file is how a playlist outlives it. Both are written here, and both are written
//! together, because a playlist that exists in one and not the other is a playlist the DJ cannot trust.
//!
//! Nothing here touches the audio path: the caller runs it on its own thread.

use std::path::{Path, PathBuf};

use dj_domain::{CollectionPage, PlaylistOp, PlaylistQuery, PlaylistStub, PlaylistTree};
use dj_library::Index;
use dj_library::index::PageQuery;
use dj_library::playlist_file::{self, PlaylistFile, PlaylistFileTrack};

/// The mirror under the data folder: `<data>/hauler-dj/playlists`.
pub fn default_mirror() -> Option<PathBuf> {
    dj_library::index::data_folder().map(|data| playlist_file::mirror_root(&data))
}

/// The tree as the panel draws it: folders first, then the playlists in their folders.
pub fn tree(index: &Index) -> Result<PlaylistTree, String> {
    let mut folders = index
        .playlist_folders()
        .map_err(|error| format!("playlist folders: {error}"))?;
    let playlists: Vec<PlaylistStub> = index
        .playlists()
        .map_err(|error| format!("playlists: {error}"))?
        .into_iter()
        .map(|row| PlaylistStub {
            id: row.id,
            name: row.name,
            folder: row.folder,
            revision: row.revision,
            tracks: row.tracks,
        })
        .collect();
    // A folder that only exists because a playlist lives in it is a folder the DJ must see.
    for stub in &playlists {
        if !stub.folder.is_empty() && !folders.contains(&stub.folder) {
            folders.push(stub.folder.clone());
        }
    }
    Ok(PlaylistTree { folders, playlists })
}

/// The rows of one playlist, in the shape the collection's table already draws.
pub fn page(index: &Index, playlist: i64, query: &PlaylistQuery) -> Result<CollectionPage, String> {
    let asked = PageQuery {
        search: query.search.clone(),
        offset: query.offset,
        limit: query.limit,
        ..PageQuery::default()
    };
    let page = index
        .playlist_page(playlist, &asked)
        .map_err(|error| format!("playlist {}: {error}", playlist))?;
    Ok(CollectionPage {
        id: query.id,
        total: page.total,
        rows: page.rows.iter().map(crate::collection_row).collect(),
    })
}

/// Every playlist as its file: what the mirror is written from.
pub fn files(index: &Index) -> Result<Vec<PlaylistFile>, String> {
    let mut files = Vec::new();
    for row in index
        .playlists()
        .map_err(|error| format!("playlists: {error}"))?
    {
        let tracks = index
            .playlist_tracks(row.id)
            .map_err(|error| format!("playlist {}: {error}", row.id))?
            .into_iter()
            .map(|track| PlaylistFileTrack {
                fingerprint: track.fingerprint,
                path: track.path,
            })
            .collect();
        files.push(PlaylistFile {
            format: playlist_file::FORMAT.to_string(),
            id: row.id,
            name: row.name,
            folder: row.folder,
            revision: row.revision,
            updated_at_ms: row.updated_at_ms,
            tracks,
        });
    }
    Ok(files)
}

/// Make the mirror say what the index says: one file per playlist, and the files of playlists that are
/// gone are removed — a rename or a move is the same case, the old path is no longer wanted.
pub fn sync(index: &Index, mirror: &Path) -> Result<usize, String> {
    let wanted = files(index)?;
    playlist_file::sync(mirror, &wanted).map_err(|error| format!("mirror: {error}"))
}

/// Apply one edit: the index first, the mirror after, and the tree comes back so the panel redraws.
pub fn apply(
    index: &mut Index,
    mirror: &Path,
    op: &PlaylistOp,
    now_ms: i64,
) -> Result<PlaylistTree, String> {
    match op {
        PlaylistOp::Create { name, folder } => {
            let name = name.trim();
            if name.is_empty() {
                return Err("a playlist needs a name".to_string());
            }
            index
                .playlist_create(name, folder, now_ms)
                .map_err(|error| format!("create: {error}"))?;
            index
                .playlist_folder_create(folder)
                .map_err(|error| format!("folder: {error}"))?;
        }
        PlaylistOp::Rename { playlist, name } => {
            let name = name.trim();
            if name.is_empty() {
                return Err("a playlist needs a name".to_string());
            }
            index
                .playlist_rename(*playlist, name, now_ms)
                .map_err(|error| format!("rename: {error}"))?;
        }
        PlaylistOp::Delete { playlist } => {
            index
                .playlist_delete(*playlist)
                .map_err(|error| format!("delete: {error}"))?;
        }
        PlaylistOp::Move { playlist, folder } => {
            index
                .playlist_move(*playlist, folder, now_ms)
                .map_err(|error| format!("move: {error}"))?;
            index
                .playlist_folder_create(folder)
                .map_err(|error| format!("folder: {error}"))?;
        }
        PlaylistOp::AddTracks { playlist, tracks } => {
            let mut rows = index
                .playlist_tracks(*playlist)
                .map_err(|error| format!("playlist {playlist}: {error}"))?;
            for track in tracks {
                rows.push(dj_library::PlaylistTrackRow {
                    fingerprint: track.fingerprint.clone(),
                    path: track.path.clone(),
                });
            }
            index
                .playlist_set_tracks(*playlist, &rows, now_ms)
                .map_err(|error| format!("add: {error}"))?;
        }
        PlaylistOp::RemoveRows {
            playlist,
            positions,
        } => {
            let rows = index
                .playlist_tracks(*playlist)
                .map_err(|error| format!("playlist {playlist}: {error}"))?;
            let kept: Vec<dj_library::PlaylistTrackRow> = rows
                .into_iter()
                .enumerate()
                .filter(|(at, _)| !positions.contains(at))
                .map(|(_, row)| row)
                .collect();
            index
                .playlist_set_tracks(*playlist, &kept, now_ms)
                .map_err(|error| format!("remove rows: {error}"))?;
        }
        PlaylistOp::Reorder { playlist, from, to } => {
            let mut rows = index
                .playlist_tracks(*playlist)
                .map_err(|error| format!("playlist {playlist}: {error}"))?;
            if *from >= rows.len() || *to >= rows.len() {
                return Err(format!("row {from} or {to} is outside playlist {playlist}"));
            }
            let moved = rows.remove(*from);
            rows.insert(*to, moved);
            index
                .playlist_set_tracks(*playlist, &rows, now_ms)
                .map_err(|error| format!("reorder: {error}"))?;
        }
        PlaylistOp::RemoveDuplicates { playlist } => {
            index
                .playlist_remove_duplicates(*playlist, now_ms)
                .map_err(|error| format!("remove dups: {error}"))?;
        }
        PlaylistOp::FolderCreate { path } => {
            index
                .playlist_folder_create(path)
                .map_err(|error| format!("folder: {error}"))?;
        }
        PlaylistOp::FolderRename { from, to } => {
            index
                .playlist_folder_rename(from, to)
                .map_err(|error| format!("folder rename: {error}"))?;
        }
        PlaylistOp::FolderDelete { path } => {
            index
                .playlist_folder_delete(path)
                .map_err(|error| format!("folder delete: {error}"))?;
        }
    }
    sync(index, mirror)?;
    tree(index)
}

/// What the mirror says, read back into the index.
///
/// The rule the owner set: the database is the truth a session works from, and the file is how a playlist
/// outlives it — so a file the index does not know is a playlist to restore, and a file that is *newer*
/// than the row (a hand edit, a copy from another machine) wins the argument by its revision. The rest is
/// written back by the sync.
pub fn startup_pass(index: &mut Index, mirror: &Path) -> Result<PlaylistTree, String> {
    let files = playlist_file::files(mirror).map_err(|error| format!("mirror: {error}"))?;
    let known = index
        .playlists()
        .map_err(|error| format!("playlists: {error}"))?;
    for path in files {
        let file = match playlist_file::read(&path) {
            Ok(file) => file,
            Err(error) => {
                // A file that cannot be read is named and left alone: guessing at it would be inventing
                // the DJ's work.
                eprintln!("dj-engine: {error}");
                continue;
            }
        };
        let existing = known
            .iter()
            .find(|row| row.name == file.name && row.folder == file.folder);
        let rows: Vec<dj_library::PlaylistTrackRow> = file
            .tracks
            .iter()
            .map(|track| dj_library::PlaylistTrackRow {
                fingerprint: track.fingerprint.clone(),
                path: track.path.clone(),
            })
            .collect();
        match existing {
            None => {
                let id = index
                    .playlist_create(&file.name, &file.folder, file.updated_at_ms)
                    .map_err(|error| format!("restore {}: {error}", file.name))?;
                index
                    .playlist_set_tracks(id, &rows, file.updated_at_ms)
                    .map_err(|error| format!("restore {}: {error}", file.name))?;
                index
                    .playlist_set_revision(id, file.revision)
                    .map_err(|error| format!("restore {}: {error}", file.name))?;
                index
                    .playlist_folder_create(&file.folder)
                    .map_err(|error| format!("restore {}: {error}", file.name))?;
                eprintln!(
                    "dj-engine: playlist {:?} came back from {}",
                    file.name,
                    path.display()
                );
            }
            Some(row) if file.revision > row.revision => {
                index
                    .playlist_set_tracks(row.id, &rows, file.updated_at_ms)
                    .map_err(|error| format!("newer {}: {error}", file.name))?;
                // The file was ahead: the index keeps its revision, so the next sync does not write the older
                // number back over the DJ's edit.
                index
                    .playlist_set_revision(row.id, file.revision)
                    .map_err(|error| format!("newer {}: {error}", file.name))?;
                eprintln!(
                    "dj-engine: playlist {:?} took revision {} from its file",
                    file.name, file.revision
                );
            }
            Some(_) => {}
        }
    }
    let _ = sync(index, mirror)?;
    tree(index)
}
