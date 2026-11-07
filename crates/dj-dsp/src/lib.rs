//! DSP primitives: the three-band channel EQ (A14) and the biquad underneath it.
//!
//! The rule the whole crate follows: coefficients are computed off the audio callback, and the
//! callback only runs a direct-form biquad. A flat EQ is a bypass, not a multiplication by one.
//!
//! - Backlog task: A14
//! - Requirements: DJP-MIX-002 (the three-band part), DJP-ARCH-006, DJP-NFR-008
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod biquad;
pub mod eq;

pub use biquad::Biquad;
pub use eq::{Band, ChannelEq, Crossover, EqGains, RANGE_DB};
