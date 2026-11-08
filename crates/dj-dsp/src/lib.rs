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
pub mod isolator;

pub use biquad::Biquad;
pub use dj_domain::{EQ_BOOST_DB, EQ_CUT_DB, EQ_FLOOR_DB, EqBand, EqGains};
pub use eq::{ChannelEq, Crossover, SILENT_DB, STEP_DB, gain_of_db};
pub use isolator::{Isolator, IsolatorBand};

/// The knob's ends, re-exported here because this is where a caller looks for them.
///
/// `BOOST` is the DJ cap (+6 dB), `CUT` the ordinary cut (−26 dB) and `FLOOR` the isolator kill
/// (−120 dB). The filter accepts all three; only the knob law decides which end the hand reaches.
pub use dj_domain::{EQ_BOOST_DB as BOOST_DB, EQ_CUT_DB as CUT_DB, EQ_FLOOR_DB as FLOOR_DB};
