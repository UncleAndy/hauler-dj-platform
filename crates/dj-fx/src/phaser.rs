//! Phaser: four all-pass stages swept together, mixed half and half with the dry signal.
//!
//! All-pass stages change phase and nothing else; put four in series and add the dry signal back, and the places
//! where the two are in antiphase become notches that move as the stages are swept (Zölzer, *DAFX*). The cookbook's
//! all-pass coefficients are what `dj-dsp`'s biquad takes directly, so this module is four of those and an
//! oscillator — no second filter design in the tree.

use dj_dsp::Biquad;

use crate::FxUnit;
use crate::smooth::OnePole;

/// The stages: four is the classic number — fewer is a gentle sweep, more is a whoosh without notches.
const STAGES: usize = 4;
/// Where the sweep starts and how far it goes, in hertz.
const LOW_HZ: f32 = 200.0;
const HIGH_HZ: f32 = 3_000.0;
/// The sweep on the other side of the centre: lower and wider, so the notches walk through the body of the sound rather
/// than across its top.
const LOW_HZ_LEFT: f32 = 120.0;
const HIGH_HZ_LEFT: f32 = 2_000.0;
/// The sweep's rate: a phaser is a slow movement, and a rate that grew with the knob turned it into a wobble —
/// measured as a *smaller* level spread at the top of the travel than at three quarters of it (21.11). It is fast
/// enough (0.45 Hz) that the lower half of the knob is visibly moving rather than standing still.
const LFO_HZ: f32 = 0.45;
/// The rate on that side: a phaser this wide has to be slow, or the notches smear into a wobble.
const LFO_HZ_LEFT: f32 = 0.12;
/// Half and half at the far end: a phaser that is 100 % wet is a low-pass wobble, not a phaser.
const MIX: f32 = 0.5;

/// A swept all-pass chain.
#[derive(Debug, Clone)]
pub struct Phaser {
    amount: f32,
    rate: f32,
    phase: f32,
    phase_step: f32,
    depth: OnePole,
    stages: [[Biquad; STAGES]; 2],
    designed: f32,
}

impl Default for Phaser {
    fn default() -> Self {
        Self::new()
    }
}

impl Phaser {
    pub fn new() -> Self {
        Self {
            amount: 0.0,
            rate: 48_000.0,
            phase: 0.0,
            phase_step: 0.0,
            depth: OnePole::new(),
            stages: [[Biquad::bypass(); STAGES]; 2],
            designed: f32::NAN,
        }
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        // The side of the centre decides both the sweep's speed and its span, so the step is set here as well.
        self.phase_step = self.lfo_hz() / self.rate;
    }

    /// How much of the effect, 0 … 1: the distance from the centre, whichever way the knob is turned.
    fn travel(&self) -> f32 {
        self.amount.abs()
    }

    fn lfo_hz(&self) -> f32 {
        if self.amount < 0.0 {
            LFO_HZ_LEFT
        } else {
            LFO_HZ
        }
    }

    /// The two ends of the sweep the knob's side asks for.
    fn sweep_range(&self) -> (f32, f32) {
        if self.amount < 0.0 {
            (LOW_HZ_LEFT, HIGH_HZ_LEFT)
        } else {
            (LOW_HZ, HIGH_HZ)
        }
    }

    /// The cookbook's all-pass coefficients for a corner frequency, written straight into the four stages of both
    /// channels. `Q = 0.5` is the gentle all-pass the classic four-stage phaser uses: higher makes the notches
    /// narrow and the sweep whistle.
    fn design(&mut self, corner: f32) {
        let w0 = 2.0 * std::f64::consts::PI * f64::from(corner.min(self.rate * 0.45))
            / f64::from(self.rate);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * 0.5);
        // RBJ all-pass: b0 = 1 - alpha, b1 = -2 cos, b2 = 1 + alpha, a0 = 1 + alpha, a1 = -2 cos, a2 = 1 - alpha.
        let (b0, b1, b2) = (1.0 - alpha, -2.0 * cos, 1.0 + alpha);
        let (a0, a1, a2) = (1.0 + alpha, -2.0 * cos, 1.0 - alpha);
        for channel in 0..2 {
            for stage in 0..STAGES {
                self.stages[channel][stage].set_coefficients(b0, b1, b2, a0, a1, a2);
            }
        }
    }
}

impl FxUnit for Phaser {
    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.phase_step = self.lfo_hz() / self.rate;
        self.depth.set_time(self.rate, 0.020);
        self.depth.reset(self.travel());
        self.phase = 0.0;
        self.designed = f32::NAN;
    }

    fn reset(&mut self) {
        for channel in &mut self.stages {
            for stage in channel {
                stage.reset();
            }
        }
        self.phase = 0.0;
        self.designed = f32::NAN;
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        // `sqrt`, as in the flanger: half a turn is seventy per cent of the effect, so the left half of the travel is
        // audible at all (owner's report, 21.11).
        let strength = self.travel().sqrt();
        self.depth.advance(strength, frames);
        let depth = self.depth.value();
        // Nothing of the effect at the bottom of the travel: the deck has to come back untouched.
        if depth <= 0.0005 {
            out[..samples].copy_from_slice(&input[..samples]);
            return;
        }
        let mix = MIX * depth;
        // The corner is re-designed once a block: `sin`/`cos` on a block boundary, and never in the sample loop.
        let sweep = (self.phase * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        let (low, high) = self.sweep_range();
        let corner = low * (high / low).powf(sweep * depth);
        if !self.designed.is_finite() || (corner - self.designed).abs() > 1.0 {
            self.design(corner);
            self.designed = corner;
        }

        for frame in 0..samples / 2 {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];
            let mut wet_left = left;
            let mut wet_right = right;
            for stage in &mut self.stages[0] {
                wet_left = stage.process(wet_left);
            }
            for stage in &mut self.stages[1] {
                wet_right = stage.process(wet_right);
            }
            self.phase += self.phase_step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            out[frame * 2] = left * (1.0 - mix) + wet_left * mix;
            out[frame * 2 + 1] = right * (1.0 - mix) + wet_right * mix;
        }
    }
}
