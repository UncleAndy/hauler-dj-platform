//! Decoders, indexed seek and bounded PCM prefetch.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A02
//! - Requirements: DJP-MEDIA-001, DJP-MEDIA-002
//! - Decoding runs on media workers; the RT thread only reads the prefilled ring.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
