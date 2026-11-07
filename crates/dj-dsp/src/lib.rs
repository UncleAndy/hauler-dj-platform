//! DSP primitives: the three-band channel EQ (A14) and the biquad underneath it.
//!
//! The rule the whole crate follows: coefficients are computed off the audio callback, and the
//! callback only runs a direct-form biquad. They are computed once per knob position, when the
//! equaliser is built, so a move costs a copy of five numbers. A flat EQ is a bypass, not a
//! multiplication by one.
//!
//! - Backlog task: A14
//! - Requirements: DJP-MIX-002 (the three-band part), DJP-ARCH-006, DJP-NFR-008
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod biquad;
pub mod eq;

pub use biquad::Biquad;
pub use dj_domain::{EQ_RANGE_DB, EqBand, EqGains};
pub use eq::{ChannelEq, Crossover, EqTable, STEP_DB, STEPS, Section};

/// How much a band can cut or boost, in decibels.
///
/// The value is the protocol's (`EQ_RANGE_DB`); the name is kept here because this is where a caller
/// looks for it.
pub const RANGE_DB: f32 = EQ_RANGE_DB;
