//! Session and settings persistence with atomic writes and recovery.
//!
//! A session is what a set-up looks like when the application closes: the audio device, the mixer,
//! and which track sat on which deck and where (DJP-SET-002). It holds paths and numbers, never audio.
//!
//! Writes are atomic and the previous document is kept (DJP-SET-003), so a crash or a bad file costs
//! one write rather than the whole set-up. Migrations are a chain by version number; a document from a
//! newer build is refused by name instead of being read as if it were this one. Autosave runs on its
//! own thread and coalesces requests, because the thread that owns the audio graph must never wait for
//! a disk.
//!
//! - Backlog task: A08
//! - Requirements: DJP-ENV-006, DJP-SET-002, DJP-SET-003
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod autosave;
pub mod doc;
pub mod migrate;
pub mod store;

pub use autosave::Autosave;
pub use doc::{AudioSettings, DeckState, MixerSettings, SCHEMA, SessionDoc, VERSION};
pub use store::{Loaded, SessionError, SessionStore, Source};
