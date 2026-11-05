//! Real-time audio core: the fixed-block render loop, the bounded command queue and the
//! sample-accurate scheduler.
//!
//! Status: **A01**. One sine source, transport, gain, tempo and crossfader — enough to
//! prove the contract, the scheduler and the offline render path. Track loading (A02),
//! decks B–D (A03) and the mixer (A04) build on these pieces without changing them.
//!
//! Ownership: audio-core (`docs/architecture/crate-map.md`).
//!
//! - Requirements: DJP-ARCH-001, DJP-ARCH-003, DJP-ARCH-005, DJP-NFR-001
//! - `Engine::process` must not allocate, lock, log or wait; `dj_testkit::rt` proves it.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod bench;
pub mod engine;
pub mod queue;
pub mod sine;
pub mod wav;

pub use bench::{BlockTimings, TimingSummary, render_measured};
pub use engine::{ACK_CAPACITY, Engine, render_offline, scheduled};
pub use queue::{CRITICAL_CAPACITY, CommandQueue, QueueStats};
pub use sine::SineSource;
pub use wav::{BITS_PER_SAMPLE, RenderInfo, write_pcm16_mono};
