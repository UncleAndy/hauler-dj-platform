//! Portable per-track metadata and the index over it.
//!
//! Two stores, one meaning. `.djmeta/<file name>.json` beside the music is the source of truth for
//! cue points, grids, loops and the rest of what the user decided (DJP-DATA-006), so copying a folder
//! carries the work with it. The SQLite index holds only what a library is searched and sorted by and
//! is rebuilt by scanning, which is why losing it loses nothing.
//!
//! - Backlog task: A08
//! - Requirements: DJP-DATA-001…006
//! - Nothing here touches the audio thread: reads and writes happen on the loader side.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod fingerprint;
pub mod import;
pub mod index;
pub mod layout;
pub mod meta;
pub mod queue;
pub mod sidecar;
pub mod store;
pub mod tags;

pub use import::{ImportError, Imported, import};
pub use index::{Index, IndexError, TrackRow};
pub use meta::{Anchor, BeatGrid, Cue, Fingerprint, Loop, SCHEMA, TrackMeta};
pub use queue::{Progress, Report, Run, threads_for};
pub use sidecar::MetaError;
pub use store::{Loaded, Saved, Source, Store};
