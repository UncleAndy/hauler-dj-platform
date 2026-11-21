//! One-pole smoothing and equal-power fades: the two pieces every effect in this crate uses to keep a hand's
//! movement from becoming a click.
//!
//! The one-pole is the textbook form (Julius O. Smith III, *Introduction to Digital Filters*): `y += a (x - y)`
//! with `a = 1 - exp(-1 / (tau Fs))`, so the time constant is in seconds and does not depend on the block size.
//! The fade curves are the cookbook's constant-power pair, `cos` and `sin` of a quarter turn, which is what the
//! mixer's crossfader already uses.

/// A one-pole smoother.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OnePole {
    coeff: f32,
    state: f32,
}

impl OnePole {
    pub(crate) fn new() -> Self {
        Self {
            coeff: 1.0,
            state: 0.0,
        }
    }

    /// Set the time constant; `coefficient` is derived from the rate, so the smoother is the same speed whatever
    /// the block size.
    pub(crate) fn set_time(&mut self, sample_rate: f32, seconds: f32) {
        self.coeff = if seconds <= 0.0 {
            1.0
        } else {
            1.0 - (-1.0 / (seconds * sample_rate)).exp()
        };
    }

    pub(crate) fn reset(&mut self, value: f32) {
        self.state = value;
    }

    pub(crate) fn value(&self) -> f32 {
        self.state
    }

    pub(crate) fn process(&mut self, input: f32) -> f32 {
        self.state += (input - self.state) * self.coeff;
        self.state
    }

    /// Advance the state one block towards `target` at once, which is how a smoother that only feeds coefficients
    /// is driven: the result is the same as processing every frame.
    pub(crate) fn advance(&mut self, target: f32, frames: usize) {
        let steps = frames as f32;
        self.state += (target - self.state) * (1.0 - (1.0 - self.coeff).powf(steps));
    }
}

/// Equal-power crossfade: `t` runs 0.0 … 1.0, the two gains always sum in power to one.
pub(crate) fn equal_power(t: f32) -> (f32, f32) {
    let angle = t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}
