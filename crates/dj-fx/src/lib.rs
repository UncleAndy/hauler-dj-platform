//! Deck effects (DJP-FX-001, DJP-FX-004): one slot per deck, driven by a knob and a switch.
//!
//! Every effect here is written from the standard literature, not tuned by ear, and each module's doc
//! comment names the formula it implements and the source it came from:
//!
//! - biquad coefficients (LPF, HPF, band-pass, all-pass) and the equal-power curves used at every seam —
//!   Robert Bristow-Johnson, *Audio EQ Cookbook*;
//! - parameter smoothing and delay lines — Julius O. Smith III, CCRMA (*Introduction to Digital Filters*,
//!   *Physical Audio Signal Processing*);
//! - the shapes of echo, flanger, phaser and crusher — Udo Zölzer (ed.), *DAFX: Digital Audio Effects*;
//! - denormal and NaN hygiene on the render path — the RT practice the mixer already follows
//!   (DJP-NFR-001, `docs/architecture/rt-contract.md`).
//!
//! The contract is the same as the mixer's: everything is preallocated by [`FxUnit::prepare`], and
//! [`FxUnit::process`] allocates nothing, takes no locks and cannot panic on the audio path.
//!
//! Ownership: dsp (`docs/architecture/crate-map.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod filter;

pub use dj_domain::{FxKind, FxPatch};
pub use filter::DjFilter;

/// One effect in the deck's slot: it takes the channel's block and writes what the room should hear.
///
/// The block is interleaved stereo at the mixer's sample rate, the same shape the mixer already hands
/// around, so a slot can sit in the strip without a conversion step.
pub trait FxUnit: Send {
    /// Take the sample rate and the largest block that will ever arrive. Buffers are allocated here and
    /// only here: after this call the effect must not allocate (DJP-NFR-001).
    fn prepare(&mut self, sample_rate: f32, max_frames: usize);

    /// Forget everything that outlives a block: delay lines, filter state, oscillator phase.
    ///
    /// Called when the effect is switched away from, so the next switch-on starts clean instead of
    /// replaying a tail from a minute ago.
    fn reset(&mut self);

    /// Process `frames` interleaved stereo frames from `input` into `out`.
    ///
    /// `input` and `out` may be the same length or longer; only `frames * 2` samples are read and
    /// written. The mixer keeps them apart so that the dry signal is still available for the crossfade
    /// when an effect is switched on or off.
    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize);

    /// How much later than the input this effect's output arrives, in frames.
    ///
    /// The lookahead limiter already delays the master bus; an effect that delays as well has to say so,
    /// or the monitoring bus and the room drift apart (DJP-FX-004 asks for a latency report).
    fn latency_frames(&self) -> usize {
        0
    }
}

/// The slot with nothing in it: the signal passes through untouched.
///
/// It exists so that the mixer's slot has a defined occupant before any effect is written, and so that
/// "the effect is off" has exactly one meaning on the render path — the same samples in and out.
#[derive(Debug, Default, Clone, Copy)]
pub struct Dry;

impl FxUnit for Dry {
    fn prepare(&mut self, _sample_rate: f32, _max_frames: usize) {}

    fn reset(&mut self) {}

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        out[..samples].copy_from_slice(&input[..samples]);
    }
}
