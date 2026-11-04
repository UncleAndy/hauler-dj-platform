//! Audio device adapter: enumeration, stream configuration, xrun telemetry.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A05
//! - Requirements: DJP-NFR-002, DJP-NFR-003, DJP-ARCH-007
//! - Backend choice (CPAL vs native PipeWire/JACK) is ADR-002 and stays open.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
