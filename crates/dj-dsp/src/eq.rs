//! Three bands on every channel: a low shelf, a mid peak and a high shelf (A14, DJP-MIX-002).
//!
//! Two rules shape this file.
//!
//! The first: a knob move must cost a copy, not arithmetic. The coefficients of every position the
//! protocol can express are computed once, when the equaliser is built; a move then picks a row out
//! of that table. `powf` and `sin_cos` inside the audio callback is exactly what the RT contract
//! forbids (DJP-NFR-001), and a knob is the kind of event that arrives in the middle of it.
//!
//! The second: unity is a bypass, not a multiplication by one, so a channel whose knobs were never
//! touched keeps its samples bit for bit — and three knobs at zero cost nothing at all.

use crate::biquad::Biquad;
pub use dj_domain::{EQ_RANGE_DB, EqBand, EqGains};

/// Distance between two positions of a knob, in decibels.
///
/// An eighth of a decibel: finer than anyone hears, coarser than the noise of the shelf, and — the
/// reason for this exact number — representable without error in binary. A decimal step would leave
/// unity reading as `1.19e-7` dB and turn "flat is a bypass" into a comparison with a tolerance.
pub const STEP_DB: f32 = 0.125;

/// Rows in the table of one band, both ends of the range included.
///
/// `2 × 26 dB / 0.125 dB + 1`. The test `the_table_is_as_long_as_the_range_says` holds this number
/// to the arithmetic, because a table one row short turns the top of a knob into a clamped value
/// nobody notices.
pub const STEPS: usize = 417;

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

/// One row of the table: `(b0, b1, b2, a1, a2)`, normalised so `a0 = 1`.
pub type Section = [f64; 5];

/// Every knob position of one sample rate, already turned into coefficients.
#[derive(Clone)]
pub struct EqTable {
    sample_rate: f64,
    crossover: Crossover,
    // Band-major: `band.index() * STEPS + step`.
    rows: Vec<Section>,
}

impl std::fmt::Debug for EqTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EqTable")
            .field("sample_rate", &self.sample_rate)
            .field("crossover", &self.crossover)
            .field("rows", &self.rows.len())
            .finish()
    }
}

impl EqTable {
    /// Build the whole table: `3 × 417` rows, a few thousand transcendental calls, once.
    pub fn new(sample_rate: f64) -> Self {
        let rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let crossover = Crossover::default();
        let mut rows = vec![[1.0, 0.0, 0.0, 0.0, 0.0]; EqBand::COUNT * STEPS];
        for band in EqBand::ALL {
            for step in 0..STEPS {
                let gain_db = f64::from(Self::gain_of(step));
                let raw = match band {
                    EqBand::Low => shelf_coefficients(rate, crossover.low_hz, gain_db, false),
                    EqBand::Mid => {
                        peak_coefficients(rate, crossover.mid_hz, crossover.mid_q, gain_db)
                    }
                    EqBand::High => shelf_coefficients(rate, crossover.high_hz, gain_db, true),
                };
                rows[band.index() * STEPS + step] = normalise(raw);
            }
        }
        Self {
            sample_rate: rate,
            crossover,
            rows,
        }
    }

    /// Sample rate the table was built for.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Where the bands sit.
    pub fn crossover(&self) -> Crossover {
        self.crossover
    }

    /// Which row a requested gain lands on: the nearest eighth of a decibel inside the range.
    ///
    /// A value that is not a number lands on unity, so a broken client cannot mute a channel.
    pub fn step_of(gain_db: f32) -> usize {
        if !gain_db.is_finite() {
            return Self::UNITY_STEP;
        }
        let clamped = gain_db.clamp(-EQ_RANGE_DB, EQ_RANGE_DB);
        let step = f64::from(clamped + EQ_RANGE_DB) / f64::from(STEP_DB);
        (step.round() as usize).min(STEPS - 1)
    }

    /// The gain a row means, in decibels.
    pub fn gain_of(step: usize) -> f32 {
        -EQ_RANGE_DB + (step.min(STEPS - 1) as f32) * STEP_DB
    }

    /// Coefficients of one band at one row.
    pub fn row(&self, band: EqBand, step: usize) -> Section {
        self.rows[band.index() * STEPS + step.min(STEPS - 1)]
    }

    /// Where unity sits in the table.
    pub const UNITY_STEP: usize = (STEPS - 1) / 2;
}

/// A channel's three-band equaliser, stereo.
#[derive(Debug, Clone)]
pub struct ChannelEq {
    table: EqTable,
    gains: EqGains,
    low: [Biquad; 2],
    mid: [Biquad; 2],
    high: [Biquad; 2],
}

impl ChannelEq {
    /// Build a flat equaliser for this sample rate. The table is computed here, off the audio path.
    pub fn new(sample_rate: f64) -> Self {
        let mut eq = Self {
            table: EqTable::new(sample_rate),
            gains: EqGains::FLAT,
            low: [Biquad::bypass(); 2],
            mid: [Biquad::bypass(); 2],
            high: [Biquad::bypass(); 2],
        };
        eq.set_gains(EqGains::FLAT);
        eq
    }

    /// The gains in force, as applied — already on the knob's grid, which is what the strip shows.
    pub fn gains(&self) -> EqGains {
        self.gains
    }

    /// The table this equaliser selects from.
    pub fn table(&self) -> &EqTable {
        &self.table
    }

    /// Set all three bands.
    ///
    /// Cheap by construction: the arithmetic happened when the table was built, so this is a handful
    /// of copies. That is what makes it callable from the audio callback when a knob moves.
    pub fn set_gains(&mut self, gains: EqGains) {
        let mut applied = EqGains::FLAT;
        for band in EqBand::ALL {
            applied.set_band(band, EqTable::gain_of(EqTable::step_of(gains.band(band))));
        }
        self.gains = applied;
        if applied.is_flat() {
            // Unity is a bypass, not a multiplication by one: a flat EQ must not cost a sample of
            // precision, and must not drift with every block.
            self.low = [Biquad::bypass(); 2];
            self.mid = [Biquad::bypass(); 2];
            self.high = [Biquad::bypass(); 2];
            return;
        }
        let lows = self
            .table
            .row(EqBand::Low, EqTable::step_of(applied.low_db));
        let mids = self
            .table
            .row(EqBand::Mid, EqTable::step_of(applied.mid_db));
        let highs = self
            .table
            .row(EqBand::High, EqTable::step_of(applied.high_db));
        for (sections, row) in [
            (&mut self.low, lows),
            (&mut self.mid, mids),
            (&mut self.high, highs),
        ] {
            for section in sections.iter_mut() {
                section.set_coefficients(row[0], row[1], row[2], 1.0, row[3], row[4]);
            }
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
    pub fn band_magnitude_db(&self, band: EqBand, frequency: f64) -> f64 {
        let row = self.table.row(band, self.band_step(band));
        section_magnitude_db(&row, frequency, self.table.sample_rate())
    }

    /// Magnitude response of the whole chain, in decibels.
    pub fn magnitude_db(&self, frequency: f64) -> f64 {
        EqBand::ALL
            .into_iter()
            .map(|band| self.band_magnitude_db(band, frequency))
            .sum()
    }

    /// Where this band sits in the table right now.
    fn band_step(&self, band: EqBand) -> usize {
        if self.gains.is_flat() {
            return EqTable::UNITY_STEP;
        }
        EqTable::step_of(self.gains.band(band))
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

/// Divide by `a0`, so the row is what a direct-form section wants.
fn normalise(raw: (f64, f64, f64, f64, f64, f64)) -> Section {
    let (b0, b1, b2, a0, a1, a2) = raw;
    if a0 == 0.0 || !a0.is_finite() {
        return [1.0, 0.0, 0.0, 0.0, 0.0];
    }
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// Magnitude of one normalised section at `frequency`, in decibels.
fn section_magnitude_db(row: &Section, frequency: f64, sample_rate: f64) -> f64 {
    let omega = std::f64::consts::TAU * frequency / sample_rate;
    let (sin, cos) = omega.sin_cos();
    let (sin2, cos2) = (2.0 * omega).sin_cos();
    // z^-1 = e^{-jω}, z^-2 = e^{-2jω}: the sine of z^-2 is sin(2ω), not 2·sin(ω).
    let (s1, c1) = (-sin, cos);
    let (s2, c2) = (-sin2, cos2);
    let num_re = row[0] + row[1] * c1 + row[2] * c2;
    let num_im = row[1] * s1 + row[2] * s2;
    let den_re = 1.0 + row[3] * c1 + row[4] * c2;
    let den_im = row[3] * s1 + row[4] * s2;
    let magnitude =
        ((num_re * num_re + num_im * num_im).sqrt()) / ((den_re * den_re + den_im * den_im).sqrt());
    20.0 * magnitude.max(1e-12).log10()
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
