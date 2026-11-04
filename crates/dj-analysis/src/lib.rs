//! Waveform peak pyramid, cache fingerprints and background analysis.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A07
//! - Requirements: DJP-MEDIA-003
//! - The waveform timebase must equal source frames exactly.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
