//! Test instrumentation for the audio core.
//!
//! The interesting part is [`rt::CountingAllocator`] + [`rt::RtScope`]: together they turn
//! "the callback does not allocate" from a review comment into an assertion (DJP-NFR-001).
//!
//! Ownership: qa (`docs/architecture/crate-map.md`).
#![deny(clippy::unwrap_used)]

pub mod rt;
