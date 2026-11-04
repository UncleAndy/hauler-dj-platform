//! Golden fixtures, simulated clock and RT instrumentation for tests.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A11
//! - Requirements: DJP-NFR-001
//! - Evidence produced here feeds docs/qa/evidence/.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
