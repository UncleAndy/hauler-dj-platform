//! Turning a file from anywhere into a row of the collection (DJP-MEDIA-004).
//!
//! The source is copied, never moved: the collection is a library, not a file manager. The copy gets
//! the name its tags earn, broken tags are repaired in the copy, and a `.djmeta` beside it records the
//! fingerprint so the track is recognised after a move (DJP-DATA-004).
//!
//! The order matters and is the owner's: tags first, then the copy with the right name, then the tags
//! of the copy brought into line with that name. The source file is never opened for writing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{TrackMeta, fingerprint, layout, sidecar, tags};

/// Why an import did not happen. A file that fails leaves nothing behind: the copy is written before
/// the metadata, and a failure between them removes the copy.
#[derive(Debug)]
pub enum ImportError {
    Io { path: PathBuf, source: io::Error },
    Meta(Box<sidecar::MetaError>),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Meta(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// What an import produced.
#[derive(Debug, Clone)]
pub struct Imported {
    /// The file in the collection.
    pub path: PathBuf,
    /// Its metadata, in the `.djmeta` directory of the same folder.
    pub meta: PathBuf,
    /// The tags the copy carries now: repaired, and filled from the file name when they were empty.
    pub tags: tags::Tags,
    /// True when a broken encoding was repaired on the way in.
    pub repaired: bool,
    /// True when the name had to come from the file itself, because the tags had nothing to say.
    pub named_from_file: bool,
}

/// Import one file into the collection rooted at `root`.
pub fn import(source: &Path, root: &Path, now_ms: u64) -> Result<Imported, ImportError> {
    let io_error = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ImportError::Io {
            path: path.clone(),
            source,
        }
    };

    let mut read = tags::read(source).map_err(io_error(source))?;
    let (artist, artist_verdict) = tags::repair(&read.artist);
    let (title, title_verdict) = tags::repair(&read.title);
    let repaired = artist_verdict.changed() || title_verdict.changed();

    // A tag that says nothing about the artist or the title leaves that half to the file name, which
    // is what a collection imported from another program usually has to work with.
    let (file_artist, file_title) = from_file_name(source);
    let named_from_file = artist.trim().is_empty() || title.trim().is_empty();
    let artist = if artist.trim().is_empty() {
        file_artist
    } else {
        artist
    };
    let title = if title.trim().is_empty() {
        file_title
    } else {
        title
    };

    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let relative = layout::collection_path(&artist, &title, extension);
    let target = free_path(&root.join(relative));

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    fs::copy(source, &target).map_err(io_error(&target))?;

    read.artist = artist;
    read.title = title;
    if repaired || named_from_file {
        // The tags of the copy are brought into line with the name the copy now has; the source keeps
        // whatever it had.
        tags::write(&target, &read).map_err(io_error(&target))?;
    }

    let print = fingerprint::of_path(&target).map_err(io_error(&target))?;
    let meta = TrackMeta::new(print, now_ms);
    let meta_path = sidecar::file_of(&target).ok_or_else(|| ImportError::Io {
        path: target.clone(),
        source: io::Error::other("the file has no name to hang metadata on"),
    })?;
    sidecar::write(&meta_path, &meta).map_err(|error| ImportError::Meta(Box::new(error)))?;

    Ok(Imported {
        path: target,
        meta: meta_path,
        tags: read,
        repaired,
        named_from_file,
    })
}

/// The artist and title a file name claims, when it has the shape the collection writes.
fn from_file_name(source: &Path) -> (String, String) {
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .trim();
    match stem.split_once(" - ") {
        Some((artist, title)) => (artist.trim().to_owned(), title.trim().to_owned()),
        None => (String::new(), stem.to_owned()),
    }
}

/// The first free name at or after `target`: a second copy of a track is `… (2).flac`, not an
/// overwrite. Two files with the same tags are a question about duplicates (DJP-MEDIA-005), and that
/// question needs both files to exist.
fn free_path(target: &Path) -> PathBuf {
    let mut candidate = target.to_path_buf();
    let mut number = 2_u32;
    while candidate.exists() {
        candidate = numbered(target, number);
        number = number.saturating_add(1);
    }
    candidate
}

fn numbered(target: &Path, number: u32) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new(""));
    let stem = target
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("track");
    let extension = target.extension().and_then(|extension| extension.to_str());
    let name = match extension {
        Some(extension) => format!("{stem} ({number}).{extension}"),
        None => format!("{stem} ({number})"),
    };
    parent.join(name)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_with_a_separator_gives_both_halves() {
        assert_eq!(
            from_file_name(Path::new("/music/Abba - Dancing Queen.flac")),
            ("Abba".to_owned(), "Dancing Queen".to_owned())
        );
    }

    #[test]
    fn a_file_name_without_a_separator_gives_a_title_only() {
        assert_eq!(
            from_file_name(Path::new("/music/Dancing Queen.flac")),
            (String::new(), "Dancing Queen".to_owned())
        );
    }

    #[test]
    fn a_second_copy_does_not_overwrite_the_first() {
        let root = std::env::temp_dir().join(format!("dj-import-free-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch");
        let first = root.join("A - B.flac");
        fs::write(&first, b"one").expect("first");
        let second = free_path(&first);
        assert_eq!(second, root.join("A - B (2).flac"));
        fs::write(&second, b"two").expect("second");
        assert_eq!(free_path(&first), root.join("A - B (3).flac"));
        let _ = fs::remove_dir_all(&root);
    }
}
