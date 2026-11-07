//! Where metadata lives, and what happens when two copies of it disagree (DJP-DATA-006).
//!
//! Two places are written: `.djmeta` next to the music, which travels with a copied folder, and a
//! mirror under the user's data directory, which is the only home available on a read-only volume.
//! On load the newer record wins by revision and then by time; a genuine disagreement is reported
//! rather than resolved behind the user's back.

use std::path::{Path, PathBuf};

use crate::meta::TrackMeta;
use crate::sidecar::{self, MetaError};

/// Which copy was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `.djmeta` beside the music.
    Sidecar,
    /// The mirror under the data directory.
    Mirror,
}

/// The record that won, and both sides when they disagreed.
#[derive(Debug)]
pub struct Loaded {
    /// The record to use.
    pub meta: TrackMeta,
    /// Where it came from.
    pub source: Source,
    /// Both copies, when they differ and neither clearly supersedes the other.
    pub conflict: Option<(TrackMeta, TrackMeta)>,
}

/// What a save managed to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saved {
    /// Written next to the music.
    pub sidecar: bool,
    /// Written to the mirror.
    pub mirror: bool,
}

/// The metadata store: the mirror root plus the rules for merging.
#[derive(Debug, Clone)]
pub struct Store {
    mirror_root: PathBuf,
}

impl Store {
    /// Use an explicit mirror root.
    pub fn new(mirror_root: impl Into<PathBuf>) -> Self {
        Self {
            mirror_root: mirror_root.into(),
        }
    }

    /// `$XDG_DATA_HOME/hauler-dj/meta`, or `~/.local/share/hauler-dj/meta` when the variable is unset.
    pub fn with_default_mirror() -> Self {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(base.join("hauler-dj").join("meta"))
    }

    /// The mirror root in use.
    pub fn mirror_root(&self) -> &Path {
        &self.mirror_root
    }

    /// Where this track's mirror copy lives: `<root>/<volume>/<path inside the volume>.json`.
    pub fn mirror_path(&self, volume: &str, music_root: &Path, music: &Path) -> PathBuf {
        let volume = if volume.is_empty() { "root" } else { volume };
        let volume = volume.replace(['/', '\\'], "_");
        let relative = music
            .strip_prefix(music_root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| PathBuf::from(music.file_name().unwrap_or_default()));
        let name = relative
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = relative.parent().unwrap_or(Path::new(""));
        self.mirror_root
            .join(volume)
            .join(parent)
            .join(format!("{name}.json"))
    }

    /// Read whatever exists and decide which copy to trust.
    pub fn load(&self, volume: &str, music_root: &Path, music: &Path) -> Result<Loaded, MetaError> {
        let sidecar_path = sidecar::file_of(music).ok_or(MetaError::NotFound)?;
        let sidecar = read_optional(&sidecar_path);
        let mirror_path = self.mirror_path(volume, music_root, music);
        let mirror = read_optional(&mirror_path);

        match (sidecar, mirror) {
            (Ok(Some(near)), Ok(Some(far))) => {
                if near == far {
                    return Ok(Loaded {
                        meta: near,
                        source: Source::Sidecar,
                        conflict: None,
                    });
                }
                let (winner, loser, source) = if near.wins_over(&far) {
                    (near, far, Source::Sidecar)
                } else if far.wins_over(&near) {
                    (far, near, Source::Mirror)
                } else {
                    // Same revision and same time but different content: two machines edited from a
                    // common ancestor. Neither may be discarded silently.
                    return Ok(Loaded {
                        meta: near.clone(),
                        source: Source::Sidecar,
                        conflict: Some((near, far)),
                    });
                };
                Ok(Loaded {
                    conflict: Some((loser, winner.clone())),
                    meta: winner,
                    source,
                })
            }
            (Ok(Some(near)), _) => Ok(Loaded {
                meta: near,
                source: Source::Sidecar,
                conflict: None,
            }),
            (_, Ok(Some(far))) => Ok(Loaded {
                meta: far,
                source: Source::Mirror,
                conflict: None,
            }),
            // Nothing readable: report the first reason, preferring the copy beside the music, and
            // leave the broken file where it is.
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
            (Ok(None), Ok(None)) => Err(MetaError::NotFound),
        }
    }

    /// Write both copies. A read-only music folder is not an error: the mirror still holds the data.
    pub fn save(
        &self,
        volume: &str,
        music_root: &Path,
        music: &Path,
        meta: &TrackMeta,
    ) -> Result<Saved, MetaError> {
        let mut saved = Saved {
            sidecar: false,
            mirror: false,
        };
        if let Some(path) = sidecar::file_of(music) {
            match sidecar::write(&path, meta) {
                Ok(()) => saved.sidecar = true,
                Err(MetaError::Io { ref source, .. })
                    if source.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    // A volume mounted read-only, or a folder that belongs to someone else.
                }
                Err(error) => return Err(error),
            }
        }
        let path = self.mirror_path(volume, music_root, music);
        sidecar::write(&path, meta)?;
        saved.mirror = true;
        Ok(saved)
    }

    /// Find the metadata that belongs to this file by its bytes, and put it under the canonical name.
    ///
    /// A plain `mv` renames the audio and not `.djmeta`, so the record is still in the folder under
    /// the old name. Renaming it here is what makes "the file was renamed" a non-event (DJP-DATA-004).
    pub fn adopt(&self, music: &Path) -> Result<Option<PathBuf>, MetaError> {
        let Some(canonical) = sidecar::file_of(music) else {
            return Ok(None);
        };
        if canonical.exists() {
            return Ok(Some(canonical));
        }
        let Some(directory) = sidecar::dir_of(music) else {
            return Ok(None);
        };
        let wanted = crate::fingerprint::of_path(music).map_err(|source| MetaError::Io {
            path: music.to_path_buf(),
            source,
        })?;
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return Ok(None);
        };
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("json")
            {
                continue;
            }
            let Ok(meta) = sidecar::read(&candidate) else {
                continue;
            };
            if meta.fingerprint.key() != wanted.key() {
                continue;
            }
            std::fs::rename(&candidate, &canonical).map_err(|source| MetaError::Io {
                path: candidate.clone(),
                source,
            })?;
            return Ok(Some(canonical));
        }
        Ok(None)
    }

    /// Every metadata file under a music root, with the track it belongs to.
    pub fn scan(&self, root: &Path) -> Vec<(PathBuf, TrackMeta)> {
        let mut found = Vec::new();
        walk(root, &mut found, 0);
        found
    }
}

/// Read a copy, treating "no file" as an answer rather than an error.
fn read_optional(path: &Path) -> Result<Option<TrackMeta>, MetaError> {
    match sidecar::read(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(MetaError::Io { ref source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Depth-limited walk for `.djmeta` directories. Symlinks are not followed: a link loop must not
/// turn a scan into a hang.
fn walk(directory: &Path, found: &mut Vec<(PathBuf, TrackMeta)>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(sidecar::DIRECTORY) {
                collect(&path, found);
            } else if kind.is_dir() {
                walk(&path, found, depth + 1);
            }
        }
    }
}

/// Read every `.json` in a `.djmeta` directory, skipping files that do not parse.
fn collect(directory: &Path, found: &mut Vec<(PathBuf, TrackMeta)>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if let Ok(meta) = sidecar::read(&path) {
            // `/Music/Album/.djmeta/Song.flac.json` belongs to `/Music/Album/Song.flac`.
            let track = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".json"))
                .map(|name| directory.join("..").join(name))
                .map(|path| path.components().collect::<PathBuf>())
                .unwrap_or_else(|| path.clone());
            found.push((track, meta));
        }
    }
}
