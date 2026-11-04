//! Channel gain, faders, crossfader, bus routing and metering.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A04
//! - Requirements: DJP-MIX-001, DJP-ARCH-006, DJP-ARCH-007
//! - Routing is configuration, not a hard-wired signal path.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
