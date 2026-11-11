//! Turning a file from anywhere into a row of the collection (DJP-MEDIA-004, DJP-MEDIA-005).
//!
//! The source is copied, never moved: the collection is a library, not a file manager. The copy gets
//! the name its tags earn, broken tags are repaired in the copy, and a `.djmeta` beside it records the
//! fingerprint and the chromaprint so the track is recognised after a move (DJP-DATA-004).
//!
//! The order matters and is the owner's: tags first, then the copy with the right name, then the tags
//! of the copy brought into line with that name. The source file is never opened for writing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::identity::{self, Identity};
use crate::quality::{self, Quality};
use crate::{TrackMeta, fingerprint, layout, meta, sidecar, tags};

/// The folder that holds the files the collection refused.
pub const DUPLICATES_DIR: &str = "_duplicates";

/// What to do when the file offered is the same recording as one already in the collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Do not copy it and tell the caller: only the DJ knows whether the spare file is a better master.
    #[default]
    Ask,
    /// Keep the better copy: the loser goes to `_duplicates/`, whichever of the two it is.
    ReplaceIfBetter,
    /// Keep the new copy, whatever the numbers say: the DJ was shown both and chose this one. The copy
    /// already in the collection goes to `_duplicates/`.
    Replace,
    /// Keep both: the new file lands with a suffix and is marked as a variant.
    KeepBoth,
    /// Keep both without the marker: the DJ wants the copy, but not the promise that the pair will
    /// never be a duplicate again — the next addition will ask about it once more.
    KeepBothUnmarked,
}

/// Why an import did not happen.
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
    /// What the track is recognised by, measured before the copy was made.
    pub identity: Identity,
    /// True when a broken encoding was repaired on the way in.
    pub repaired: bool,
    /// True when the name had to come from the file itself, because the tags had nothing to say.
    pub named_from_file: bool,
    /// The marker that tells this copy from the one it duplicates, when the policy keeps both.
    pub variant: Option<String>,
}

/// What an import decided about a file that may already be in the collection.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Nothing like it was found: the file is in the collection.
    Placed(Imported),
    /// The same recording and the same edition. Nothing was copied: the DJ decides, because only he
    /// knows whether the spare file is a better master.
    Duplicate {
        existing: PathBuf,
        verdict: identity::Verdict,
        candidate: Identity,
        /// Where the file would have landed, for the question.
        target: PathBuf,
    },
    /// The same recording, another edition. Both belong here, and the caller is told which one was
    /// already there.
    Version {
        imported: Imported,
        existing: PathBuf,
        verdict: identity::Verdict,
    },
    /// The policy kept the copy already in the collection: the new file went to `_duplicates/`.
    Rejected {
        existing: PathBuf,
        /// Where the refused copy was put.
        stored: PathBuf,
        verdict: identity::Verdict,
    },
    /// The policy kept the new file: the old one went to `_duplicates/`, the new one took its place.
    Replaced {
        imported: Imported,
        /// Where the old copy was put.
        rejected: PathBuf,
        verdict: identity::Verdict,
    },
}

/// Everything decided before a byte is copied.
struct Prepared {
    artist: String,
    title: String,
    extension: String,
    tags: tags::Tags,
    identity: Identity,
    repaired: bool,
    named_from_file: bool,
}

impl Prepared {
    fn relative(&self) -> PathBuf {
        layout::collection_path(&self.artist, &self.title, &self.extension)
    }
}

/// Import one file into the collection rooted at `root`, without asking about duplicates.
pub fn import(source: &Path, root: &Path, now_ms: u64) -> Result<Imported, ImportError> {
    let prepared = prepare(source)?;
    place(source, root, prepared, now_ms, None)
}

/// Import one file, telling a duplicate from a version by duration and chromaprint (DJP-MEDIA-005).
///
/// `find` answers "is this recording already here?" — the caller owns the catalogue, so the search
/// holds whatever lock it needs for the search only, not for the copy.
pub fn import_checked(
    source: &Path,
    root: &Path,
    now_ms: u64,
    find: impl Fn(&Identity) -> Option<(PathBuf, Identity, identity::Verdict)>,
    policy: Policy,
) -> Result<Outcome, ImportError> {
    let prepared = prepare(source)?;
    let target = root.join(prepared.relative());
    let matched = find(&prepared.identity);
    let Some((existing, existing_identity, verdict)) = matched else {
        return Ok(Outcome::Placed(place(
            source, root, prepared, now_ms, None,
        )?));
    };
    match verdict {
        identity::Verdict::Different => Ok(Outcome::Placed(place(
            source, root, prepared, now_ms, None,
        )?)),
        identity::Verdict::Version { .. } => {
            let imported = place(source, root, prepared, now_ms, None)?;
            Ok(Outcome::Version {
                imported,
                existing,
                verdict,
            })
        }
        identity::Verdict::Duplicate { .. } => match policy {
            Policy::Ask => Ok(Outcome::Duplicate {
                existing,
                verdict,
                candidate: prepared.identity.clone(),
                target,
            }),
            Policy::KeepBoth => {
                let marker = variant_marker(&target);
                let imported = place(source, root, prepared, now_ms, Some(marker.clone()))?;
                Ok(Outcome::Placed(Imported {
                    variant: Some(marker),
                    ..imported
                }))
            }
            Policy::KeepBothUnmarked => Ok(Outcome::Placed(place(
                source, root, prepared, now_ms, None,
            )?)),
            Policy::Replace => {
                // No comparison: the question was asked and answered, and the answer was this file.
                let rejected = retire(root, &existing)?;
                let imported = place(source, root, prepared, now_ms, None)?;
                Ok(Outcome::Replaced {
                    imported,
                    rejected,
                    verdict,
                })
            }
            Policy::ReplaceIfBetter => {
                let candidate_quality = Quality::of(&prepared.identity);
                let existing_quality = Quality::of(&existing_identity);
                if quality::better(&candidate_quality, &existing_quality) {
                    let rejected = retire(root, &existing)?;
                    let imported = place(source, root, prepared, now_ms, None)?;
                    Ok(Outcome::Replaced {
                        imported,
                        rejected,
                        verdict,
                    })
                } else {
                    let stored = keep_as_duplicate(root, source, &prepared, now_ms)?;
                    Ok(Outcome::Rejected {
                        existing,
                        stored,
                        verdict,
                    })
                }
            }
        },
    }
}

/// Read the tags, repair them, name the track and measure its identity.
fn prepare(source: &Path) -> Result<Prepared, ImportError> {
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
    read.artist = artist.clone();
    read.title = title.clone();

    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_owned();
    // Identity is measured on the source: the copy is byte-identical until the tags are written, and
    // the audio does not change with a tag.
    let measured = identity::of_file(source, &title).map_err(io_error(source))?;

    Ok(Prepared {
        artist,
        title,
        extension,
        tags: read,
        identity: measured,
        repaired,
        named_from_file,
    })
}

/// Copy the file in and record what it is.
fn place(
    source: &Path,
    root: &Path,
    prepared: Prepared,
    now_ms: u64,
    variant: Option<String>,
) -> Result<Imported, ImportError> {
    let io_error = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ImportError::Io {
            path: path.clone(),
            source,
        }
    };
    let target = free_path(&root.join(prepared.relative()));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    fs::copy(source, &target).map_err(io_error(&target))?;

    if prepared.repaired || prepared.named_from_file {
        // The tags of the copy are brought into line with the name the copy now has; the source keeps
        // whatever it had.
        tags::write(&target, &prepared.tags).map_err(io_error(&target))?;
    }

    let mut print = fingerprint::of_path(&target).map_err(io_error(&target))?;
    // Duration and rate travel in the metadata file: the duplicate check needs them, and decoding a
    // track again to learn how long it is would cost more than the check itself.
    if prepared.identity.sample_rate > 0 {
        let frames = prepared
            .identity
            .duration_ms
            .saturating_mul(u64::from(prepared.identity.sample_rate))
            / 1_000;
        print.duration_frames = Some(frames);
        print.sample_rate = Some(prepared.identity.sample_rate);
    }
    let mut record = TrackMeta::new(print, now_ms);
    if !prepared.identity.chromaprint.is_empty() {
        record.chromaprint = Some(meta::encode_print(&prepared.identity.chromaprint));
    }
    record.variant = variant.clone();
    let meta_path = sidecar::file_of(&target).ok_or_else(|| ImportError::Io {
        path: target.clone(),
        source: io::Error::other("the file has no name to hang metadata on"),
    })?;
    sidecar::write(&meta_path, &record).map_err(|error| ImportError::Meta(Box::new(error)))?;

    Ok(Imported {
        path: target,
        meta: meta_path,
        tags: prepared.tags,
        identity: prepared.identity,
        repaired: prepared.repaired,
        named_from_file: prepared.named_from_file,
        variant,
    })
}

/// Move a track out of the collection into `_duplicates/`, with its metadata file.
fn retire(root: &Path, music: &Path) -> Result<PathBuf, ImportError> {
    let io_error = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ImportError::Io {
            path: path.clone(),
            source,
        }
    };
    let relative = music
        .strip_prefix(root)
        .map_err(|_| ImportError::Io {
            path: music.to_path_buf(),
            source: io::Error::other("the track is not inside the collection"),
        })?
        .to_path_buf();
    let target = free_path(&root.join(DUPLICATES_DIR).join(relative));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    move_file(music, &target)?;
    let meta_path = sidecar::file_of(music).filter(|path| path.is_file());
    let target_meta = sidecar::file_of(&target);
    if let (Some(meta_path), Some(target_meta)) = (meta_path, target_meta)
        && let Some(parent) = target_meta.parent()
    {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
        move_file(&meta_path, &target_meta)?;
    }
    Ok(target)
}

/// Put the refused copy into `_duplicates/` under the name it would have had.
fn keep_as_duplicate(
    root: &Path,
    source: &Path,
    prepared: &Prepared,
    now_ms: u64,
) -> Result<PathBuf, ImportError> {
    let io_error = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ImportError::Io {
            path: path.clone(),
            source,
        }
    };
    let target = free_path(&root.join(DUPLICATES_DIR).join(prepared.relative()));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_error(parent))?;
    }
    fs::copy(source, &target).map_err(io_error(&target))?;
    if let Some(meta_path) = sidecar::file_of(&target) {
        let mut record = TrackMeta::new(
            fingerprint::of_path(&target).map_err(io_error(&target))?,
            now_ms,
        );
        if !prepared.identity.chromaprint.is_empty() {
            record.chromaprint = Some(meta::encode_print(&prepared.identity.chromaprint));
        }
        sidecar::write(&meta_path, &record).map_err(|error| ImportError::Meta(Box::new(error)))?;
    }
    Ok(target)
}

/// A rename, with a copy for the case where the two paths are on different filesystems.
fn move_file(from: &Path, to: &Path) -> Result<(), ImportError> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).map_err(|source| ImportError::Io {
        path: to.to_path_buf(),
        source,
    })?;
    fs::remove_file(from).map_err(|source| ImportError::Io {
        path: from.to_path_buf(),
        source,
    })
}

/// The marker that tells a kept variant from the copy it duplicates.
fn variant_marker(target: &Path) -> String {
    let name = target
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("track");
    // The next free number is what the file will get, so the marker and the name agree.
    let mut number = 2_u32;
    while numbered(target, number).exists() {
        number = number.saturating_add(1);
    }
    let _ = name;
    number.to_string()
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
/// overwrite. Two files with the same tags are a question about duplicates, and that question needs
/// both files to exist.
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
