//! Reading and writing the session document (DJP-SET-002, DJP-SET-003).
//!
//! Two rules decide the shape of this module. A write is atomic, so a crash leaves either the old
//! document or the new one and never half of either. And the previous document is kept beside it, so
//! a file that cannot be parsed — a bad disk, a hand edit, a bug — costs the last write, not the
//! whole set-up.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::doc::{SCHEMA, SessionDoc};
use crate::migrate;

/// What went wrong with the session.
#[derive(Debug)]
pub enum SessionError {
    /// The filesystem refused.
    Io {
        /// Which file.
        path: PathBuf,
        /// What the OS said.
        source: std::io::Error,
    },
    /// The document is not valid JSON, or not the shape the schema promises.
    Parse(serde_json::Error),
    /// The document is newer than this build: refusing is honest, guessing is not.
    TooNew {
        /// Version in the file.
        found: u32,
        /// Version this build writes.
        known: u32,
    },
    /// A step of the chain is missing.
    NoRoute {
        /// Version the document is at.
        from: u32,
        /// Version it must reach.
        to: u32,
    },
    /// The file belongs to another schema.
    ForeignSchema(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Parse(error) => write!(formatter, "unreadable session: {error}"),
            Self::TooNew { found, known } => write!(
                formatter,
                "the session was written by a newer build (version {found}, this one knows {known})"
            ),
            Self::NoRoute { from, to } => {
                write!(formatter, "no migration from version {from} to {to}")
            }
            Self::ForeignSchema(found) => {
                write!(formatter, "schema {found} is not {SCHEMA}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

/// Which file answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The document itself.
    Current,
    /// The copy kept from the previous write, after the current one failed to parse.
    Backup,
    /// Nothing usable on disk: the caller starts from defaults.
    Defaults,
}

/// The loaded document and where it came from.
#[derive(Debug)]
pub struct Loaded {
    /// The document to use.
    pub document: SessionDoc,
    /// Where it came from.
    pub source: Source,
    /// The reason the current document was not used, when it was not.
    pub problem: Option<SessionError>,
}

/// The session file and its backup.
#[derive(Debug, Clone)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    /// A store at an explicit path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `$XDG_CONFIG_HOME/hauler-dj/session.json`, or `~/.config/hauler-dj/session.json`.
    pub fn with_default_path() -> Self {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(base.join("hauler-dj").join("session.json"))
    }

    /// The document path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The copy of the previous write.
    pub fn backup_path(&self) -> PathBuf {
        self.path.with_extension("json.bak")
    }

    /// Load the newest document that can be read.
    pub fn load(&self) -> Loaded {
        match self.read(&self.path) {
            Ok(document) => Loaded {
                document,
                source: Source::Current,
                problem: None,
            },
            Err(problem) => match self.read(&self.backup_path()) {
                Ok(document) => Loaded {
                    document,
                    source: Source::Backup,
                    problem: Some(problem),
                },
                Err(_) => Loaded {
                    document: SessionDoc::default(),
                    source: Source::Defaults,
                    problem: Some(problem),
                },
            },
        }
    }

    /// Write the document: atomically, with the previous one kept as a backup.
    pub fn save(&self, document: &SessionDoc) -> Result<(), SessionError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SessionError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        // Keep what is there now: a document that cannot be parsed tomorrow is still worth having if
        // it was readable today.
        if self.path.exists() {
            let backup = self.backup_path();
            std::fs::copy(&self.path, &backup).map_err(|source| SessionError::Io {
                path: backup.clone(),
                source,
            })?;
        }
        let text = serde_json::to_vec_pretty(document).map_err(SessionError::Parse)?;
        let temporary = self.path.with_extension("json.tmp");
        {
            let mut file =
                std::fs::File::create(&temporary).map_err(|source| SessionError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            file.write_all(&text).map_err(|source| SessionError::Io {
                path: temporary.clone(),
                source,
            })?;
            file.write_all(b"\n").map_err(|source| SessionError::Io {
                path: temporary.clone(),
                source,
            })?;
            file.sync_all().map_err(|source| SessionError::Io {
                path: temporary.clone(),
                source,
            })?;
        }
        std::fs::rename(&temporary, &self.path).map_err(|source| {
            let _ = std::fs::remove_file(&temporary);
            SessionError::Io {
                path: self.path.clone(),
                source,
            }
        })
    }

    fn read(&self, path: &Path) -> Result<SessionDoc, SessionError> {
        let bytes = std::fs::read(path).map_err(|source| SessionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let raw: Value = serde_json::from_slice(&bytes).map_err(SessionError::Parse)?;
        migrate::bring_forward(raw)
    }
}
