//! Rust side of the UI bridge: command intake and snapshot publication.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A06
//! - Requirements: DJP-ARCH-004, DJP-UI-001, DJP-UI-002
//! - In-process or IPC is ADR-003; the UI never owns engine state.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
