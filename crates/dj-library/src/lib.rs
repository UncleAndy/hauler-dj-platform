//! SQLite collection, migrations and queries.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A08
//! - Requirements: DJP-DATA-001, DJP-DATA-003, DJP-DATA-004, DJP-DATA-005
//! - WAL, one serialized writer, never on the audio thread.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
