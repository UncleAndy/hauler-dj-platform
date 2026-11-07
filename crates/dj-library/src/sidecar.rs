//! The metadata file next to the music: `.djmeta/<file name>.json` (DJP-DATA-006).
//!
//! One file per track, named after the track with its extension, so `Song.flac` and `Song.mp3` in
//! one folder never collide. Writes are atomic: a half-written file would be worse than none.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::meta::{SCHEMA, TrackMeta};

/// The catalogue that sits beside the audio files.
pub const DIRECTORY: &str = ".djmeta";

/// What can go wrong with a metadata file.
#[derive(Debug)]
pub enum MetaError {
    /// The filesystem refused.
    Io {
        /// Which file.
        path: PathBuf,
        /// What the OS said.
        source: io::Error,
    },
    /// The file exists but cannot be parsed; the original is left untouched.
    Corrupt {
        /// Which file.
        path: PathBuf,
        /// Why it did not parse.
        message: String,
    },
    /// The file was written by a schema this build does not know.
    Schema {
        /// Which file.
        path: PathBuf,
        /// What it says it is.
        found: String,
    },
    /// Nothing has been recorded for this track yet.
    NotFound,
}

impl fmt::Display for MetaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Corrupt { path, message } => {
                write!(
                    formatter,
                    "{}: unreadable metadata: {message}",
                    path.display()
                )
            }
            Self::Schema { path, found } => write!(
                formatter,
                "{}: schema {found} is not {SCHEMA}",
                path.display()
            ),
            Self::NotFound => write!(formatter, "no metadata recorded"),
        }
    }
}

impl std::error::Error for MetaError {}

/// The `.djmeta` directory that belongs to a music file's folder.
pub fn dir_of(music: &Path) -> Option<PathBuf> {
    music.parent().map(|parent| parent.join(DIRECTORY))
}

/// The metadata file of one music file.
pub fn file_of(music: &Path) -> Option<PathBuf> {
    let name = music.file_name()?.to_str()?;
    Some(dir_of(music)?.join(format!("{name}.json")))
}

/// Read a metadata file.
pub fn read(path: &Path) -> Result<TrackMeta, MetaError> {
    let io_error = |source| MetaError::Io {
        path: path.to_path_buf(),
        source,
    };
    let bytes = std::fs::read(path).map_err(io_error)?;
    let meta: TrackMeta = serde_json::from_slice(&bytes).map_err(|error| MetaError::Corrupt {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if meta.schema != SCHEMA {
        return Err(MetaError::Schema {
            path: path.to_path_buf(),
            found: meta.schema,
        });
    }
    Ok(meta)
}

/// Write a metadata file atomically: a temporary file in the same directory, then a rename.
pub fn write(path: &Path, meta: &TrackMeta) -> Result<(), MetaError> {
    let io_error = |source| MetaError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    let text = serde_json::to_vec_pretty(meta).map_err(|error| MetaError::Corrupt {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let temporary = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&temporary).map_err(io_error)?;
        file.write_all(&text).map_err(io_error)?;
        file.write_all(b"\n").map_err(io_error)?;
        // The rename is the commit: without it, a crash leaves the previous file intact.
        file.sync_all().map_err(io_error)?;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    Ok(())
}

#[cfg(test)]
// A failed expectation in a test is a failure message; the workspace forbids them in library code.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::meta::{Fingerprint, SCHEMA};

    fn fingerprint() -> Fingerprint {
        Fingerprint {
            size: 42,
            head: "0123456789abcdef".to_owned(),
            tail: "fedcba9876543210".to_owned(),
            duration_frames: Some(48_000),
            sample_rate: Some(48_000),
        }
    }

    #[test]
    fn the_file_sits_beside_the_track_and_carries_its_extension() {
        let music = Path::new("/Music/Album/Song.flac");
        assert_eq!(
            file_of(music).as_deref(),
            Some(Path::new("/Music/Album/.djmeta/Song.flac.json"))
        );
        let sibling = Path::new("/Music/Album/Song.mp3");
        assert_ne!(
            file_of(music),
            file_of(sibling),
            "same stem, different file"
        );
    }

    #[test]
    fn a_round_trip_keeps_fields_this_build_does_not_know() {
        let dir = std::env::temp_dir().join(format!("dj-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let music = dir.join("Song.wav");
        std::fs::write(&music, b"not really audio").expect("write");
        let path = file_of(&music).expect("path");

        let mut meta = TrackMeta::new(fingerprint(), 1_000);
        meta.extra
            .insert("future_field".to_owned(), serde_json::json!({"kept": true}));
        write(&path, &meta).expect("write meta");
        let read_back = read(&path).expect("read meta");
        assert_eq!(read_back, meta);
        assert_eq!(read_back.schema, SCHEMA);

        // No temporary file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
            .expect("list")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_reported_and_kept() {
        let dir = std::env::temp_dir().join(format!("dj-sidecar-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(".djmeta").join("Song.wav.json");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, b"{ this is not json").expect("write");
        match read(&path) {
            Err(MetaError::Corrupt { .. }) => {}
            other => panic!("expected a corrupt error, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).expect("read"),
            b"{ this is not json",
            "the broken file must stay exactly as it was"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
