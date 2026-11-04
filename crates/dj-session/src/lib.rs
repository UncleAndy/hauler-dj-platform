//! Session and settings persistence with atomic writes and recovery.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A08
//! - Requirements: DJP-ENV-006, DJP-SET-002, DJP-SET-003
//! - Autosave must not block audio; a bad migration rolls back.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
