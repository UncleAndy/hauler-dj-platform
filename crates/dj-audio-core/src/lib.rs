//! Real-time audio graph, bounded command queue, block scheduler, offline render.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A01
//! - Requirements: DJP-ARCH-001, DJP-ARCH-003, DJP-ARCH-005, DJP-NFR-001
//! - Nothing here may allocate, lock or perform I/O inside the callback.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
