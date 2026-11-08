//! Three bands on every channel, and a gain on each (A14, DJP-MIX-002).
//!
//! The equaliser is a crossover, not three shelves. A shelf shapes the spectrum around a corner: a
//! "−26 dB" shelf measures −26 dB at its centre and −13 dB an octave away, so the number on the knob
//! and the sound disagree and the middle band only ever moved by half of what it promised. A crossover
//! gives each band its own gain, which is what a mixer's three-band EQ has always been: the whole band
//! goes down by the number on the knob, and the neighbouring bands do not move at all.
//!
//! Two rules survive from the shelf days.
//!
//! The first: a knob move must cost a copy, not arithmetic. The filter coefficients are designed once,
//! when the equaliser is built; a knob move only sets three gains. `powf` and `sin_cos` inside the
//! audio callback is what the RT contract forbids (DJP-NFR-001).
//!
//! The second: unity is a bypass, not a multiplication by one, so a channel whose knobs were never
//! touched keeps its samples bit for bit — and three knobs at zero cost nothing at all.

use crate::isolator::{Isolator, IsolatorBand};
pub use dj_domain::{EQ_BOOST_DB, EQ_CUT_DB, EQ_FLOOR_DB, EqBand, EqGains};

/// Distance between two positions of a knob, in decibels.
///
/// A quarter of a decibel: finer than anyone hears, coarser than the noise of the crossover, and — the
/// reason for this exact number — representable without error in binary. A decimal step would leave
/// unity reading as `1.19e-7` dB and turn "flat is a bypass" into a comparison with a tolerance.
pub const STEP_DB: f32 = 0.25;

/// Gains at or below this are silence for a band, not a very quiet band.
///
/// The knob's floor is −120 dB, and the audio floor of the mixer is −60 dB (`db_to_gain`), so anything
/// under −60 is a removed band. Keeping one number for both the knob and the gain means the kill does
/// not depend on which path asked for it.
pub const SILENT_DB: f32 = -60.0;

/// Where the bands meet, in hertz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossover {
    /// Where the low band ends and the middle begins.
    pub low_hz: f64,
    /// Where the middle band ends and the high band begins.
    pub high_hz: f64,
}

impl Default for Crossover {
    fn default() -> Self {
        // Chosen so the three bands are audibly separate on a full-range track and do not overlap
        // into a tone control with one knob.
        Self {
            low_hz: 200.0,
            high_hz: 4_000.0,
        }
    }
}

/// A channel's three-band equaliser, stereo.
#[derive(Debug, Clone)]
pub struct ChannelEq {
    gains: EqGains,
    /// The crossover the equaliser is made of: three bands and a gain on each.
    bands: Isolator,
}

impl ChannelEq {
    /// Build a flat equaliser for this sample rate. The coefficients are designed here, off the audio
    /// path.
    pub fn new(sample_rate: f64) -> Self {
        let mut eq = Self {
            gains: EqGains::FLAT,
            bands: Isolator::new(sample_rate, Crossover::default()),
        };
        eq.set_gains(EqGains::FLAT);
        eq
    }

    /// The gains in force, as applied — already on the knob's grid, which is what the strip shows.
    pub fn gains(&self) -> EqGains {
        self.gains
    }

    /// The crossover this equaliser is built on.
    pub fn crossover(&self) -> Crossover {
        self.bands.crossover()
    }

    /// Set all three bands.
    ///
    /// Cheap by construction: the coefficients were designed when the equaliser was built, so this is
    /// three gains. That is what makes it callable from the audio callback when a knob moves.
    pub fn set_gains(&mut self, gains: EqGains) {
        let mut applied = EqGains::FLAT;
        for band in EqBand::ALL {
            applied.set_band(band, Self::on_the_grid(gains.band(band)));
        }
        self.gains = applied;
        self.bands.set_gains(
            gain_of_db(applied.low_db),
            gain_of_db(applied.mid_db),
            gain_of_db(applied.high_db),
        );
    }

    /// Where a requested gain lands on the knob's grid: the nearest quarter decibel inside the range.
    ///
    /// A value that is not a number lands on unity, so a broken client cannot mute a channel.
    pub fn on_the_grid(gain_db: f32) -> f32 {
        if !gain_db.is_finite() {
            return 0.0;
        }
        let clamped = gain_db.clamp(EQ_FLOOR_DB, EQ_BOOST_DB);
        let step = ((f64::from(clamped) - f64::from(EQ_FLOOR_DB)) / f64::from(STEP_DB)).round();
        (EQ_FLOOR_DB as f64 + step * f64::from(STEP_DB)) as f32
    }

    /// Process interleaved stereo in place.
    #[inline]
    pub fn process(&mut self, frames: &mut [f32]) {
        self.bands.process(frames);
    }

    /// Magnitude response of one band at `frequency`, in decibels — that band alone, with its gain.
    pub fn band_magnitude_db(&self, band: EqBand, frequency: f64) -> f64 {
        let which = match band {
            EqBand::Low => IsolatorBand::Low,
            EqBand::Mid => IsolatorBand::Mid,
            EqBand::High => IsolatorBand::High,
        };
        self.bands.band_magnitude_db(which, frequency)
    }

    /// Magnitude response of the whole equaliser, in decibels — the curve a strip draws.
    pub fn magnitude_db(&self, frequency: f64) -> f64 {
        self.bands.magnitude_db(frequency)
    }

    /// The crossover behind this equaliser, which is what its floor means.
    pub fn bands(&self) -> &Isolator {
        &self.bands
    }

    /// Drop the filter history, for a channel that starts a new track.
    pub fn reset(&mut self) {
        self.bands.reset();
    }
}

/// A band gain in decibels as a linear multiplier; anything at or under the mixer's floor is silence.
pub fn gain_of_db(db: f32) -> f32 {
    if !db.is_finite() || db <= SILENT_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}
