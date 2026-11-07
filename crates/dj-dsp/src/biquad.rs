//! One second-order section, in the form every cookbook agrees on.
//!
//! Coefficients are computed on the caller's thread (the UI thread or a worker), never in the audio
//! callback: a `set` allocates nothing and touches no shared state, because the channel owns its
//! filter and the callback is the only reader.

/// A direct-form-I biquad with `f64` coefficients and `f32` state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// A section that passes the signal through unchanged.
    pub const fn bypass() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Build from unnormalised coefficients; `a0` must not be zero.
    pub fn from_coefficients(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        let mut section = Self::bypass();
        section.set_coefficients(b0, b1, b2, a0, a1, a2);
        section
    }

    /// Replace the coefficients, keeping the running state: an EQ that resets on every knob turn
    /// clicks, and a click on the master is the one thing a DJ never forgives.
    pub fn set_coefficients(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        if a0 == 0.0 || !a0.is_finite() {
            *self = Self::bypass();
            return;
        }
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Drop the history, for a deck that starts a new track.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    /// Process one sample.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let output = (self.b0 * f64::from(input)
            + self.b1 * f64::from(self.x1)
            + self.b2 * f64::from(self.x2)
            - self.a1 * f64::from(self.y1)
            - self.a2 * f64::from(self.y2)) as f32;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }

    /// Magnitude response at `frequency`, in decibels: the same numbers a sweep would measure.
    pub fn magnitude_db(&self, frequency: f64, sample_rate: f64) -> f64 {
        let omega = std::f64::consts::TAU * frequency / sample_rate;
        // z^-1 = e^{-jω}
        let (sin, cos) = omega.sin_cos();
        let (sin2, cos2) = (2.0 * omega).sin_cos();
        let (s1, c1) = (-sin, cos);
        // z^-2 is e^{-2jω}, so its sine is sin(2ω) — not twice sin(ω). Reading it as `2 sin ω`
        // made the reported response disagree with the filter by up to 6 dB at the top of the band,
        // while the filter itself was right; a measurement that lies is worse than no measurement.
        let (s2, c2) = (-sin2, cos2);

        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = self.b1 * s1 + self.b2 * s2;
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = self.a1 * s1 + self.a2 * s2;
        let magnitude = ((num_re * num_re + num_im * num_im).sqrt())
            / ((den_re * den_re + den_im * den_im).sqrt());
        20.0 * magnitude.max(1e-12).log10()
    }
}
