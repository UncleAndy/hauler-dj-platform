//! The playlist mirror: one JSON file per playlist (DJP-MEDIA-004).
//!
//! The database is the truth and the file is how a playlist outlives it: delete the database and the
//! playlists come back from these files. That is the whole point of the mirror, so the file has to carry
//! everything a playlist is — its name, the folder it sits in, a revision to lose arguments by, when it was
//! last written, and its rows in order — and it has to be readable by a person, because a DJ who opens one
//! should see a playlist and not a blob.
//!
//! Files live under `<data>/hauler-dj/playlists/<folder…>/<name>.djplaylist.json`. A name is the DJ's, a
//! file name is the filesystem's: a name that cannot be one is repaired, and two playlists that repair to
//! the same name in one folder get the ` (2)` suffix the importer gives a track.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The format tag every mirror file carries. A file without it is not a playlist.
pub const FORMAT: &str = "djplaylist-v1";

/// What makes a file in the mirror a playlist file.
pub const EXTENSION: &str = "djplaylist.json";

/// One row of a mirrored playlist: the track and where its file was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistFileTrack {
    pub fingerprint: String,
    #[serde(default)]
    pub path: String,
}

/// A whole playlist, as its file keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistFile {
    /// The format tag: `djplaylist-v1`.
    pub format: String,
    /// The database's row id, so a file can be matched to its playlist. A rebuild assigns new ones — the
    /// id is a convenience here, never an identity.
    #[serde(default)]
    pub id: i64,
    pub name: String,
    /// The folder as a path like `Sets/Deep`; empty is the root.
    #[serde(default)]
    pub folder: String,
    /// Older revisions lose to newer ones when the file and the database disagree.
    #[serde(default = "one")]
    pub revision: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default)]
    pub tracks: Vec<PlaylistFileTrack>,
}

fn one() -> i64 {
    1
}

/// What the mirror can fail at. Every message names the file: a mirror problem the DJ cannot locate is a
/// mirror problem he cannot fix by hand, and fixing it by hand is what the mirror is for.
#[derive(Debug)]
pub enum PlaylistFileError {
    Io { path: PathBuf, source: io::Error },
    Corrupt { path: PathBuf, message: String },
}

impl std::fmt::Display for PlaylistFileError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(out, "{}: {source}", path.display()),
            Self::Corrupt { path, message } => write!(out, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for PlaylistFileError {}

/// Where the mirror lives: `<data>/hauler-dj/playlists`, beside the database under the same data folder.
pub fn mirror_root(data: &Path) -> PathBuf {
    data.join("hauler-dj").join("playlists")
}

/// The file name a playlist name becomes.
pub fn file_name(name: &str) -> String {
    let mut safe = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => safe.push('_'),
            character if character.is_control() => safe.push('_'),
            character => safe.push(character),
        }
    }
    let safe = safe.trim().trim_matches('.').trim().to_string();
    if safe.is_empty() {
        "playlist".to_string()
    } else {
        safe
    }
}

/// The path of one playlist's file: the folder is a path, the name is a file name.
pub fn path_of(root: &Path, folder: &str, name: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in folder.split('/').filter(|part| !part.is_empty()) {
        path.push(file_name(part));
    }
    path.join(format!("{}.{EXTENSION}", file_name(name)))
}

/// Write one playlist file atomically: a temporary file in the same folder, then a rename. A crash leaves
/// the previous file, never half of a new one.
pub fn write(path: &Path, file: &PlaylistFile) -> Result<(), PlaylistFileError> {
    let io_error = |source| PlaylistFileError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    let text = serde_json::to_vec_pretty(file).map_err(|error| PlaylistFileError::Corrupt {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let temporary = path.with_extension("json.tmp");
    {
        let mut handle = File::create(&temporary).map_err(io_error)?;
        handle.write_all(&text).map_err(io_error)?;
        handle.write_all(b"\n").map_err(io_error)?;
        handle.sync_all().map_err(io_error)?;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    Ok(())
}

/// Read one playlist file. A file of another format, or one that does not parse, is refused by name.
pub fn read(path: &Path) -> Result<PlaylistFile, PlaylistFileError> {
    let mut text = String::new();
    File::open(path)
        .and_then(|mut handle| handle.read_to_string(&mut text))
        .map_err(|source| PlaylistFileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let file: PlaylistFile =
        serde_json::from_str(&text).map_err(|error| PlaylistFileError::Corrupt {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    if file.format != FORMAT {
        return Err(PlaylistFileError::Corrupt {
            path: path.to_path_buf(),
            message: format!("format {} is not {FORMAT}", file.format),
        });
    }
    Ok(file)
}

/// Every playlist file under the mirror root, deepest folders included, in a stable order.
pub fn files(root: &Path) -> Result<Vec<PathBuf>, PlaylistFileError> {
    let mut found = Vec::new();
    collect(root, &mut found).map_err(|source| PlaylistFileError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    found.sort();
    Ok(found)
}

fn collect(folder: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
    if !folder.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(EXTENSION) || name.ends_with("json.tmp"))
        {
            found.push(path);
        }
    }
    Ok(())
}

/// Make the mirror say what the database says: write a file per playlist and remove the files of playlists
/// that are gone (a rename or a move is the same thing — the old path is no longer wanted). Answers how many
/// playlists were written.
pub fn sync(root: &Path, wanted: &[PlaylistFile]) -> Result<usize, PlaylistFileError> {
    let mut taken: Vec<(String, String)> = Vec::new();
    let mut keep: Vec<PathBuf> = Vec::new();
    for file in wanted {
        let base = file_name(&file.name);
        let mut name = base.clone();
        let mut serial = 2;
        while taken
            .iter()
            .any(|(kept_folder, kept_name)| *kept_folder == file.folder && *kept_name == name)
        {
            name = format!("{base} ({serial})");
            serial += 1;
        }
        taken.push((file.folder.clone(), name.clone()));
        let path = path_of(root, &file.folder, &name);
        write(&path, file)?;
        keep.push(path);
    }
    for path in files(root)? {
        if !keep.contains(&path) {
            std::fs::remove_file(&path).map_err(|source| PlaylistFileError::Io {
                path: path.clone(),
                source,
            })?;
        }
    }
    Ok(keep.len())
}
