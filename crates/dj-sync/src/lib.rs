//! Synchronisation: the beat, the tempo, and where both come from (DJP-SYNC-001/002).
//!
//! The planner here is deliberately blind to the origin of the beat. A deck, a MIDI clock, a network
//! clock or something that does not exist yet answer the same three questions through [`BeatClock`] —
//! how fast, where inside the beat, and how fresh the answer is. That is what makes an external clock
//! a plug-in rather than a rewrite: nothing in `dj-sync` asks which one it is talking to.
//!
//! Two rules shape the numbers:
//!
//! * Phase is measured in beats, not in frames, so two decks with different source rates and different
//!   tempos are compared in the same units.
//! * A tempo that cannot be matched inside the pitch range is refused, not clamped: a deck that
//!   silently stops short of the master is a deck that lies about being in sync.
//!
//! Nothing here touches audio. The planner takes readings and returns a decision; the engine turns
//! that decision into a command outside the real-time thread.
//!
//! Ownership: audio-core (`docs/architecture/crate-map.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod clock;
pub mod master;
pub mod meter;
pub mod phase;
pub mod plan;
pub mod trim;

pub use clock::{BeatClock, COAST_MS, ClockStatus, DeckClock, LOCK_MS, LockState};
pub use master::{DeckCandidate, MasterPolicy, choose_master};
pub use meter::DriftMeter;
pub use phase::{Phase, error_beats};
pub use plan::{Jump, PitchLimits, Refusal, SyncMode, SyncPlan, matching_ratio, plan};
pub use trim::RateTrim;
