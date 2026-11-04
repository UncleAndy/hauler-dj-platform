//! Domain contracts: commands, acks, snapshots and the audio-frame timebase.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A01
//! - Requirements: DJP-ARCH-001, DJP-ARCH-002, DJP-ARCH-004, DJP-DATA-002
//! - Every other crate depends on this one and it depends on none of them.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
