//! The bipolar DJ filter (DJP-FX-004): the centre is the signal itself, one way cuts the highs, the other the
//! lows.
//!
//! Coefficients are the cookbook's, not fitted by ear — Robert Bristow-Johnson, *Audio EQ Cookbook*, the low-pass
//! and high-pass forms with `alpha = sin(w0) / (2 Q)`:
//!
//! ```text
//! w0 = 2 pi f0 / Fs,  alpha = sin(w0) / (2 Q)
//! LPF: b0 = (1 - cos)/2, b1 = 1 - cos,     b2 = (1 - cos)/2,  a0 = 1 + alpha, a1 = -2 cos, a2 = 1 - alpha
//! HPF: b0 = (1 + cos)/2, b1 = -(1 + cos),  b2 = (1 + cos)/2,  a0 = 1 + alpha, a1 = -2 cos, a2 = 1 - alpha
//! ```
//!
//! The sweep is the knob's law:
//!
//! - the centre (±2 % of the travel) is the dry signal **bit for bit**; the sections keep running, so coming back
//!   does not start from silence;
//! - to the left the cut-off falls from 20 kHz to 150 Hz while the resonance rises (`Q = 0.707 … 3.0`; 3.0 is about
//!   +9.5 dB at the corner, which is what the cookbook's `Q = 10^(dB/40)` gives and what a DJ filter sounds like
//!   rather than a self-oscillating one);
//! - to the right the same travel is a high-pass, 20 Hz … 8 kHz;
//! - the cut-off follows the knob through a 20 ms one-pole and the dry/wet mix through 10 ms (Julius O. Smith III,
//!   *Introduction to Digital Filters*: coefficients that step are a click, not a sweep).
//!
//! Measured by `crates/dj-fx/tests/filter.rs`: 1 kHz through the centre is within ±0.5 dB of the input, 1 kHz at
//! either end is at least 30 dB down, and crossing the centre with a 50 Hz tone leaves no step above 0.02.

use dj_dsp::Biquad;

use crate::FxUnit;

/// Where the low-pass starts and where the high-pass ends, in hertz.
const LPF_OPEN_HZ: f32 = 20_000.0;
/// How far the low-pass closes on the left.
const LPF_CLOSED_HZ: f32 = 150.0;
/// Where the high-pass starts on the right and how far it opens.
const HPF_CLOSED_HZ: f32 = 20.0;
const HPF_OPEN_HZ: f32 = 8_000.0;
/// Resonance at the ends of the travel.
const Q_END: f32 = 3.0;
/// The cookbook's Butterworth value: the filter is flat in the middle of the travel.
const Q_CENTRE: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// The dead zone around the centre where the output is the input.
const CENTRE_ZONE: f32 = 0.02;
/// Where the mix reaches fully wet: without a few per cent of travel the crossfade would be a switch.
const WET_ZONE: f32 = 0.05;
/// How fast the cut-off and the mix follow the knob, in seconds.
const SWEEP_SECONDS: f32 = 0.020;
const MIX_SECONDS: f32 = 0.010;

/// A bipolar DJ filter.
#[derive(Debug, Clone)]
pub struct DjFilter {
    sample_rate: f32,
    /// What the knob asked for, 0.0 … 1.0.
    requested: f32,
    /// The bipolar position the filter is actually at, −1.0 … +1.0, smoothed.
    position: f32,
    /// How much of the filtered signal reaches the output, 0.0 … 1.0, smoothed.
    wet: f32,
    sweep_coeff: f32,
    mix_coeff: f32,
    left: Biquad,
    right: Biquad,
    /// The position the sections were designed for: `sin`/`cos` run on a block boundary, and only when the knob has
    /// actually moved.
    designed: f32,
}

impl Default for DjFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl DjFilter {
    /// A filter at its centre: the signal passes through.
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            requested: 0.5,
            position: 0.0,
            wet: 0.0,
            sweep_coeff: 1.0,
            mix_coeff: 1.0,
            left: Biquad::bypass(),
            right: Biquad::bypass(),
            designed: f32::NAN,
        }
    }

    /// The knob: 0.0 is the low-pass fully closed, 0.5 the centre, 1.0 the high-pass fully open.
    pub fn set_amount(&mut self, amount: f32) {
        self.requested = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.5
        };
    }

    /// The cut-off, the kind and the resonance a smoothed position asks for; `None` inside the centre zone.
    fn design(position: f32) -> Option<(f32, bool, f32)> {
        let travel = position.abs();
        if travel <= CENTRE_ZONE {
            return None;
        }
        let span = (travel - CENTRE_ZONE) / (1.0 - CENTRE_ZONE);
        let q = Q_CENTRE + (Q_END - Q_CENTRE) * span;
        if position < 0.0 {
            Some((
                LPF_OPEN_HZ * (LPF_CLOSED_HZ / LPF_OPEN_HZ).powf(span),
                false,
                q,
            ))
        } else {
            Some((
                HPF_CLOSED_HZ * (HPF_OPEN_HZ / HPF_CLOSED_HZ).powf(span),
                true,
                q,
            ))
        }
    }

    /// The cookbook's coefficients for a cut-off, a kind and a resonance.
    fn coefficients(
        sample_rate: f32,
        cutoff: f32,
        high_pass: bool,
        q: f32,
    ) -> (f64, f64, f64, f64, f64, f64) {
        // A cut-off at or above Nyquist would fold back: the design stops at 45 % of the rate, which is above
        // anything a filter on this path needs.
        let guard = sample_rate * 0.45;
        let w0 = 2.0 * std::f64::consts::PI * f64::from(cutoff.min(guard)) / f64::from(sample_rate);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * f64::from(q));
        let (b0, b1, b2) = if high_pass {
            ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0)
        } else {
            ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0)
        };
        (b0, b1, b2, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// Redesign both sections for a position, keeping their running state: a section that resets on every knob
    /// step clicks, and a click on the master is the one thing a DJ never forgives.
    fn set_sections(&mut self, position: f32) {
        match Self::design(position) {
            Some((cutoff, high_pass, q)) => {
                let (b0, b1, b2, a0, a1, a2) =
                    Self::coefficients(self.sample_rate, cutoff, high_pass, q);
                self.left.set_coefficients(b0, b1, b2, a0, a1, a2);
                self.right.set_coefficients(b0, b1, b2, a0, a1, a2);
            }
            None => {
                self.left.set_coefficients(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
                self.right.set_coefficients(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            }
        }
    }

    /// Move the smoothed position one block forward and design the section it asks for.
    fn advance(&mut self, frames: usize) {
        let target = self.requested * 2.0 - 1.0;
        let steps = frames as f32;
        self.position += (target - self.position) * (1.0 - (1.0 - self.sweep_coeff).powf(steps));
        let travel = self.position.abs();
        let wanted_wet = ((travel - CENTRE_ZONE) / (WET_ZONE - CENTRE_ZONE)).clamp(0.0, 1.0);
        self.wet += (wanted_wet - self.wet) * (1.0 - (1.0 - self.mix_coeff).powf(steps));

        let moved = if self.designed.is_finite() {
            (self.position - self.designed).abs()
        } else {
            f32::INFINITY
        };
        if moved > 0.0005 {
            self.set_sections(self.position);
            self.designed = self.position;
        }
    }
}

impl FxUnit for DjFilter {
    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.sweep_coeff = 1.0 - (-1.0 / (SWEEP_SECONDS * self.sample_rate)).exp();
        self.mix_coeff = 1.0 - (-1.0 / (MIX_SECONDS * self.sample_rate)).exp();
        self.position = self.requested * 2.0 - 1.0;
        self.wet = 0.0;
        self.designed = f32::NAN;
    }

    fn reset(&mut self) {
        self.position = self.requested * 2.0 - 1.0;
        self.wet = 0.0;
        self.left.reset();
        self.right.reset();
        self.designed = f32::NAN;
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        self.advance(frames);
        let wet = self.wet;
        for frame in 0..samples / 2 {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];
            let filtered_left = self.left.process(left);
            let filtered_right = self.right.process(right);
            // A mix of exactly 0.0 gives the input back unchanged: `x + (f - x) * 0.0` is `x` for finite values,
            // and non-finite input is the mixer's business, not this effect's.
            out[frame * 2] = left + (filtered_left - left) * wet;
            out[frame * 2 + 1] = right + (filtered_right - right) * wet;
        }
    }
}
