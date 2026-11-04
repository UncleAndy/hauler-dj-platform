//! Device abstraction, capability negotiation and mapping.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A09
//! - Requirements: DJP-CTRL-001, DJP-CTRL-002, DJP-CTRL-004
//! - Read-only probing until the hardware safety gate is passed.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
