//! Three bands on every channel: a low shelf, a mid peak and a high shelf (A14, DJP-MIX-002).
//!
//! The numbers are the ones the acceptance asks for: each band works inside its own range, unity is
//! flat within 0.1 dB, and a channel whose knobs were never touched is not touched at all — the
//! section is bypassed rather than multiplied by one.

use crate::biquad::Biquad;

/// How much a band can cut or boost.
pub const RANGE_DB: f32 = 26.0;

/// A band of the channel EQ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// Low shelf.
    Low,
    /// Mid peak.
    Mid,
    /// High shelf.
    High,
}

/// Where the bands sit, in hertz, and how wide the peak is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossover {
    /// Corner of the low shelf.
    pub low_hz: f64,
    /// Centre of the mid peak.
    pub mid_hz: f64,
    /// Width of the mid peak.
    pub mid_q: f64,
    /// Corner of the high shelf.
    pub high_hz: f64,
}

impl Default for Crossover {
    fn default() -> Self {
        // Chosen so the three bands are audibly separate on a full-range track and do not overlap
        // into a tone control with one knob.
        Self {
            low_hz: 200.0,
            mid_hz: 1_000.0,
            mid_q: 0.7,
            high_hz: 4_000.0,
        }
    }
}

/// The three gains a channel carries, in decibels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqGains {
    /// Low shelf gain.
    pub low_db: f32,
    /// Mid peak gain.
    pub mid_db: f32,
    /// High shelf gain.
    pub high_db: f32,
}

impl Default for EqGains {
    fn default() -> Self {
        Self::FLAT
    }
}

impl EqGains {
    /// All bands at unity.
    pub const FLAT: Self = Self {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: 0.0,
    };

    /// True when nothing would change: the section is then skipped instead of computed.
    pub fn is_flat(&self) -> bool {
        self.low_db.abs() < 0.01 && self.mid_db.abs() < 0.01 && self.high_db.abs() < 0.01
    }

    /// Clamp every gain into the allowed range.
    pub fn clamped(self) -> Self {
        let clamp = |value: f32| {
            if value.is_finite() {
                value.clamp(-RANGE_DB, RANGE_DB)
            } else {
                0.0
            }
        };
        Self {
            low_db: clamp(self.low_db),
            mid_db: clamp(self.mid_db),
            high_db: clamp(self.high_db),
        }
    }
}

/// A channel's three-band equaliser, stereo.
#[derive(Debug, Clone)]
pub struct ChannelEq {
    sample_rate: f64,
    crossover: Crossover,
    gains: EqGains,
    low: [Biquad; 2],
    mid: [Biquad; 2],
    high: [Biquad; 2],
}

impl ChannelEq {
    /// Build a flat EQ for this sample rate.
    pub fn new(sample_rate: f64) -> Self {
        let mut eq = Self {
            sample_rate: if sample_rate > 0.0 {
                sample_rate
            } else {
                48_000.0
            },
            crossover: Crossover::default(),
            gains: EqGains::FLAT,
            low: [Biquad::bypass(); 2],
            mid: [Biquad::bypass(); 2],
            high: [Biquad::bypass(); 2],
        };
        eq.set_gains(EqGains::FLAT);
        eq
    }

    /// The gains in force.
    pub fn gains(&self) -> EqGains {
        self.gains
    }

    /// Set all three bands at once. Coefficients are recomputed here, off the audio callback.
    pub fn set_gains(&mut self, gains: EqGains) {
        let gains = gains.clamped();
        self.gains = gains;
        if gains.is_flat() {
            // Unity is a bypass, not a multiplication by one: a flat EQ must not cost a sample of
            // precision, and must not drift with every block.
            self.low = [Biquad::bypass(); 2];
            self.mid = [Biquad::bypass(); 2];
            self.high = [Biquad::bypass(); 2];
            return;
        }
        let rate = self.sample_rate;
        let low = shelf_coefficients(rate, self.crossover.low_hz, f64::from(gains.low_db), false);
        let mid = peak_coefficients(
            rate,
            self.crossover.mid_hz,
            self.crossover.mid_q,
            f64::from(gains.mid_db),
        );
        let high = shelf_coefficients(rate, self.crossover.high_hz, f64::from(gains.high_db), true);
        for channel in 0..2 {
            self.low[channel].set_coefficients(low.0, low.1, low.2, low.3, low.4, low.5);
            self.mid[channel].set_coefficients(mid.0, mid.1, mid.2, mid.3, mid.4, mid.5);
            self.high[channel].set_coefficients(high.0, high.1, high.2, high.3, high.4, high.5);
        }
    }

    /// Process interleaved stereo in place.
    #[inline]
    pub fn process(&mut self, frames: &mut [f32]) {
        if self.gains.is_flat() {
            return;
        }
        for frame in frames.chunks_exact_mut(2) {
            frame[0] = self.high[0].process(self.mid[0].process(self.low[0].process(frame[0])));
            frame[1] = self.high[1].process(self.mid[1].process(self.low[1].process(frame[1])));
        }
    }

    /// Magnitude response of one band at `frequency`, in decibels — the number a sweep would measure.
    pub fn band_magnitude_db(&self, band: Band, frequency: f64) -> f64 {
        match band {
            Band::Low => self.low[0].magnitude_db(frequency, self.sample_rate),
            Band::Mid => self.mid[0].magnitude_db(frequency, self.sample_rate),
            Band::High => self.high[0].magnitude_db(frequency, self.sample_rate),
        }
    }

    /// Magnitude response of the whole chain, in decibels.
    pub fn magnitude_db(&self, frequency: f64) -> f64 {
        self.band_magnitude_db(Band::Low, frequency)
            + self.band_magnitude_db(Band::Mid, frequency)
            + self.band_magnitude_db(Band::High, frequency)
    }

    /// Drop the filter history, for a channel that starts a new track.
    pub fn reset(&mut self) {
        for band in [&mut self.low, &mut self.mid, &mut self.high] {
            for section in band.iter_mut() {
                section.reset();
            }
        }
    }
}

/// RBJ low or high shelf; returns `(b0, b1, b2, a0, a1, a2)`.
fn shelf_coefficients(
    rate: f64,
    frequency: f64,
    gain_db: f64,
    high: bool,
) -> (f64, f64, f64, f64, f64, f64) {
    let a = 10f64.powf(gain_db / 40.0);
    let omega = std::f64::consts::TAU * frequency.min(rate * 0.49) / rate;
    let (sin, cos) = omega.sin_cos();
    let alpha = sin / 2.0 * 2.0_f64.sqrt();
    let two_sqa_alpha = 2.0 * a.sqrt() * alpha;
    if high {
        let b0 = a * ((a + 1.0) + (a - 1.0) * cos + two_sqa_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos - two_sqa_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos + two_sqa_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos);
        let a2 = (a + 1.0) - (a - 1.0) * cos - two_sqa_alpha;
        (b0, b1, b2, a0, a1, a2)
    } else {
        let b0 = a * ((a + 1.0) - (a - 1.0) * cos + two_sqa_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos - two_sqa_alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos + two_sqa_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos);
        let a2 = (a + 1.0) + (a - 1.0) * cos - two_sqa_alpha;
        (b0, b1, b2, a0, a1, a2)
    }
}

/// RBJ peaking EQ; returns `(b0, b1, b2, a0, a1, a2)`.
fn peak_coefficients(
    rate: f64,
    frequency: f64,
    q: f64,
    gain_db: f64,
) -> (f64, f64, f64, f64, f64, f64) {
    let a = 10f64.powf(gain_db / 40.0);
    let omega = std::f64::consts::TAU * frequency.min(rate * 0.49) / rate;
    let (sin, cos) = omega.sin_cos();
    let alpha = sin / (2.0 * q.max(0.1));
    (
        1.0 + alpha * a,
        -2.0 * cos,
        1.0 - alpha * a,
        1.0 + alpha / a,
        -2.0 * cos,
        1.0 - alpha / a,
    )
}
