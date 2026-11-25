//! Noise sweep: white noise through a resonant band-pass whose centre is the knob.
//!
//! The generator is a xorshift: it is deterministic, it needs no allocation and it gives the same samples from the
//! same seed, which is what lets the test compare runs. The band-pass is the cookbook's, with `Q = 4` so the sweep
//! has a whistle to it (Zölzer, *DAFX*, on noise as a source and the band-pass as its filter).

use dj_dsp::Biquad;

use crate::FxUnit;
use crate::smooth::OnePole;

/// Where the band starts and where it ends, in hertz.
const LOW_HZ: f32 = 200.0;
const HIGH_HZ: f32 = 8_000.0;
/// The resonance of the sweeping band.
const Q: f32 = 4.0;
/// The level at the far end of the knob (it rises as the square, so the knob feels even).
const LEVEL: f32 = 0.35;
/// The generator's seed: any non-zero number will do, and this one is written down so runs are repeatable.
const SEED: u32 = 0x9E37_79B9;

/// Band-passed noise.
#[derive(Debug, Clone)]
pub struct Noise {
    amount: f32,
    rate: f32,
    phase: u32,
    band: [Biquad; 2],
    designed: f32,
    level: OnePole,
}

impl Default for Noise {
    fn default() -> Self {
        Self::new()
    }
}

impl Noise {
    pub fn new() -> Self {
        Self {
            amount: 0.0,
            rate: 48_000.0,
            phase: SEED,
            band: [Biquad::bypass(); 2],
            designed: f32::NAN,
            level: OnePole::new(),
        }
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }

    /// How much of the effect, 0 … 1: the distance from the centre, whichever way the knob is turned.
    fn travel(&self) -> f32 {
        self.amount.abs()
    }

    /// Whether the noise goes through the band: the right side sweeps a whistle, the left side is the noise itself — a
    /// bed to raise the floor with, which is what a noise knob is used for at a quiet moment in a set.
    fn banded(&self) -> bool {
        self.amount >= 0.0
    }

    /// One white sample in −1 … 1 (xorshift32, the smallest generator that is not obviously wrong).
    #[inline]
    fn next_sample(&mut self) -> f32 {
        let mut x = self.phase;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.phase = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// The cookbook's band-pass coefficients for the centre the knob asks for.
    fn design(&mut self, centre: f32) {
        let w0 = 2.0 * std::f64::consts::PI * f64::from(centre.min(self.rate * 0.45))
            / f64::from(self.rate);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * f64::from(Q));
        // RBJ band-pass (constant skirt gain, peak gain = Q): b0 = sin/2 = Q alpha, b1 = 0, b2 = -Q alpha.
        let (b0, b1, b2) = (alpha * f64::from(Q), 0.0, -(alpha * f64::from(Q)));
        let (a0, a1, a2) = (1.0 + alpha, -2.0 * cos, 1.0 - alpha);
        for channel in &mut self.band {
            channel.set_coefficients(b0, b1, b2, a0, a1, a2);
        }
    }
}

impl FxUnit for Noise {
    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.phase = SEED;
        self.designed = f32::NAN;
        self.level.set_time(self.rate, 0.020);
        self.level.reset(self.amount);
        self.band = [Biquad::bypass(); 2];
    }

    fn reset(&mut self) {
        self.phase = SEED;
        self.designed = f32::NAN;
        self.band = [Biquad::bypass(); 2];
        self.level.reset(self.amount);
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        self.level.advance(self.travel(), frames);
        let travel = self.level.value();
        let banded = self.banded();
        let centre = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(travel);
        if banded && (!self.designed.is_finite() || (centre - self.designed).abs() > 5.0) {
            self.design(centre);
            self.designed = centre;
        }
        let gain = LEVEL * travel * travel;
        for frame in 0..samples / 2 {
            // The sweep is added to the dry signal: it is a layer, not a replacement, so a deck in silence sweeps
            // through silence the way a hardware unit does.
            let white = self.next_sample();
            for channel in 0..2 {
                let colour = if banded {
                    self.band[channel].process(white)
                } else {
                    white
                };
                out[frame * 2 + channel] = input[frame * 2 + channel] + colour * gain;
            }
        }
    }
}
