//! Resampler, EQ and filters implementing the DSP node contract.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A04
//! - Requirements: DJP-ARCH-006, DJP-ARCH-009, DJP-NFR-008
//! - prepare/process/reset/latency_frames/tail_frames; no NaN or Inf on valid input.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
