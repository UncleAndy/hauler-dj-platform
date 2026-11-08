//! The three bands of a channel, and what the knobs do to them.
//!
//! A mixer's three-band EQ is a crossover with a gain on each band, not three shelves: a shelf shapes
//! the spectrum around a corner, so a "−26 dB" shelf is 26 dB at its centre and 13 dB at an octave
//! away — the number on the knob and the sound disagree, and the detent then has all the missing
//! decibels to make up in one step (measured: 46 dB at 1 kHz).
//!
//! This is the real thing: two Linkwitz-Riley fourth-order splits (two Butterworth second-order
//! sections per edge, from the library's biquads), and a linear gain per band. A Linkwitz-Riley
//! lowpass and highpass of one crossover sum to one in phase, so the three bands add up to the dry
//! signal exactly: the bands tile the spectrum, a gain of zero removes its band and nothing else, and
//! the floor of a knob is silence for that band.

use dj_signalsmith_sys::Isolator as Library;

use crate::eq::Crossover;

/// Which of the three bands a knob moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolatorBand {
    /// Everything below the low crossover.
    Low,
    /// What is left between the two crossovers.
    Mid,
    /// Everything above the high crossover.
    High,
}

impl IsolatorBand {
    /// All three, in the order the gains are passed.
    pub const ALL: [IsolatorBand; 3] = [IsolatorBand::Low, IsolatorBand::Mid, IsolatorBand::High];

    /// Index into a three-element array of gains.
    pub fn index(self) -> usize {
        match self {
            IsolatorBand::Low => 0,
            IsolatorBand::Mid => 1,
            IsolatorBand::High => 2,
        }
    }
}

/// A three-way crossover with a gain on each band.
#[derive(Debug)]
pub struct Isolator {
    inner: Library,
    crossover: Crossover,
    sample_rate: f64,
    gains: [f32; 3],
}

impl Isolator {
    /// An isolator for `sample_rate`, splitting where the bands meet.
    pub fn new(sample_rate: f64, crossover: Crossover) -> Self {
        let rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            inner: Library::new(rate, crossover.low_hz, crossover.high_hz),
            crossover,
            sample_rate: rate,
            gains: [1.0; 3],
        }
    }

    /// The crossover this isolator splits at.
    pub fn crossover(&self) -> Crossover {
        self.crossover
    }

    /// Sample rate its coefficients were designed for.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Linear gain of every band: `(low, mid, high)`.
    pub fn gains(&self) -> [f32; 3] {
        self.gains
    }

    /// Set all three band gains, linearly: `1.0` is unity, `0.0` takes the band out.
    pub fn set_gains(&mut self, low: f32, mid: f32, high: f32) {
        self.gains = [low, mid, high];
        self.inner.set_gains(low, mid, high);
    }

    /// True when every band stands at unity, which is a bypass.
    pub fn is_bypassed(&self) -> bool {
        self.gains.iter().all(|gain| *gain == 1.0)
    }

    /// Filter one interleaved stereo block in place. At unity the signal comes back bit for bit.
    ///
    /// The block always goes through the split, even when every band stands at unity: the splitter is
    /// what writes the dry sample back in that case, and skipping the call instead would leave its
    /// state from whenever it last ran. The first block after a knob moved then starts from a stale
    /// state, which measures as a step of fifty times the material's own — the click a hand hears when
    /// it turns a knob off zero.
    pub fn process(&mut self, frames: &mut [f32]) {
        self.inner.process(frames);
    }

    /// Drop the filter history, for a channel that starts a new track.
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    /// Response of one band at `frequency`, in decibels, with that band's gain — read from the
    /// coefficients the filter is actually running, so the drawn curve and the sound cannot drift.
    pub fn band_magnitude_db(&self, band: IsolatorBand, hertz: f64) -> f64 {
        self.inner
            .response_db(band.index() as i32, hertz, self.sample_rate)
    }

    /// Response of the whole split at `frequency`, in decibels: the three bands summed.
    pub fn magnitude_db(&self, hertz: f64) -> f64 {
        self.inner.response_db(3, hertz, self.sample_rate)
    }
}

impl Clone for Isolator {
    /// A fresh splitter with the same gains and no filter history: the state belongs to the audio that
    /// was already played, and a copy has not played it.
    fn clone(&self) -> Self {
        let mut copy = Self::new(self.sample_rate, self.crossover);
        copy.set_gains(self.gains[0], self.gains[1], self.gains[2]);
        copy
    }
}
