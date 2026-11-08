//! The isolator: what the knob's floor actually means.
//!
//! A shelf with −120 dB of gain silences its own band but takes −32 dB out of the neighbour an octave
//! and a half away — a gutted channel, not a killed band. The isolator is the real thing: a three-way
//! Linkwitz-Riley crossover (two Butterworth second-order sections per edge, from the library's
//! biquads), where removing a band is the exact complement of that band rather than a very deep shelf.
//!
//! A Linkwitz-Riley lowpass and highpass of the same order sum to one, in phase, so the complement is
//! arithmetic and not an approximation: the low band is removed by the highpass, the high band by the
//! lowpass, and the mid band is what is left when the low half and the high half are added.

use dj_signalsmith_sys::Isolator as Library;

use crate::eq::Crossover;

/// Gains at or below this are a kill, not a cut.
///
/// The ordinary law stops at −26 dB, which is a deep cut; the floor is −120 dB and only a crossover can
/// deliver it without gutting the neighbours.
pub const KILL_DB: f32 = -60.0;

/// Which of the three bands an isolator is removing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolatorBand {
    /// Removed by a highpass at the low crossover.
    Low,
    /// Removed by lowpass at the low crossover plus highpass at the high one.
    Mid,
    /// Removed by a lowpass at the high crossover.
    High,
}

impl IsolatorBand {
    /// All three, in the order the weights are passed.
    pub const ALL: [IsolatorBand; 3] = [IsolatorBand::Low, IsolatorBand::Mid, IsolatorBand::High];

    fn index(self) -> usize {
        match self {
            IsolatorBand::Low => 0,
            IsolatorBand::Mid => 1,
            IsolatorBand::High => 2,
        }
    }
}

/// A three-way crossover that removes bands.
#[derive(Debug)]
pub struct Isolator {
    inner: Library,
    crossover: Crossover,
    killed: [bool; 3],
}

impl Isolator {
    /// An isolator for `sample_rate`, splitting where the EQ's bands sit.
    pub fn new(sample_rate: f64, crossover: Crossover) -> Self {
        Self {
            inner: Library::new(sample_rate, crossover.low_hz, crossover.high_hz),
            crossover,
            killed: [false; 3],
        }
    }

    /// The crossover this isolator splits at.
    pub fn crossover(&self) -> Crossover {
        self.crossover
    }

    /// Which bands are being removed.
    pub fn killed(&self) -> [bool; 3] {
        self.killed
    }

    /// Kill `band`, or stop killing it.
    pub fn set_killed(&mut self, band: IsolatorBand, killed: bool) {
        self.killed[band.index()] = killed;
        let [low, mid, high] = self.killed;
        self.inner.set_weights(
            f32::from(u8::from(low)),
            f32::from(u8::from(mid)),
            f32::from(u8::from(high)),
        );
    }

    /// True when no band is being removed, which is a bypass.
    pub fn is_bypassed(&self) -> bool {
        !self.killed.iter().any(|killed| *killed)
    }

    /// Filter one interleaved stereo block in place. A bypass changes nothing, bit for bit.
    pub fn process(&mut self, frames: &mut [f32]) {
        if self.is_bypassed() {
            return;
        }
        self.inner.process(frames);
    }

    /// Drop the filter history, for a channel that starts a new track.
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    /// Magnitude of what the isolated bands leave behind at `frequency`, in decibels.
    ///
    /// The bands come from nested splits, so their magnitudes are a chain: the lowpass below the low
    /// crossover, then everything above it times a lowpass or a highpass at the high crossover. Each
    /// split is a Linkwitz-Riley pair (`w⁴/(1+w⁴)` highpass, `1/(1+w⁴)` lowpass, `w` the frequency over
    /// the crossover), which is in phase at every frequency — that is why the three add up to the dry
    /// signal and killing all of them leaves silence.
    pub fn magnitude_db(&self, hertz: f64) -> f64 {
        let low = lowpass_magnitude(hertz, self.crossover.low_hz);
        let upper = highpass_magnitude(hertz, self.crossover.low_hz);
        let magnitude_of = |band: IsolatorBand| match band {
            IsolatorBand::Low => low,
            IsolatorBand::Mid => lowpass_magnitude(hertz, self.crossover.high_hz) * upper,
            IsolatorBand::High => highpass_magnitude(hertz, self.crossover.high_hz) * upper,
        };
        let kept: f64 = IsolatorBand::ALL
            .into_iter()
            .map(|band| {
                if self.killed[band.index()] {
                    0.0
                } else {
                    magnitude_of(band)
                }
            })
            .sum();
        // Silence is a real answer here, not an error: three kills are −∞ dB.
        20.0 * kept.max(f64::MIN_POSITIVE).log10()
    }
}

impl Clone for Isolator {
    /// A fresh isolator with the same crossover and the same bands killed, and no filter history: the
    /// state belongs to the audio that was already played, and a copy has not played it.
    fn clone(&self) -> Self {
        let mut copy = Self::new(48_000.0, self.crossover);
        for band in IsolatorBand::ALL {
            copy.set_killed(band, self.killed[band.index()]);
        }
        copy
    }
}

/// Linkwitz-Riley fourth-order highpass magnitude, `w⁴/(1+w⁴)`.
fn highpass_magnitude(hertz: f64, crossover: f64) -> f64 {
    let w = hertz / crossover;
    let w4 = w * w * w * w;
    w4 / (1.0 + w4)
}

/// Linkwitz-Riley fourth-order lowpass magnitude, `1/(1+w⁴)`.
fn lowpass_magnitude(hertz: f64, crossover: f64) -> f64 {
    let w = hertz / crossover;
    let w4 = w * w * w * w;
    1.0 / (1.0 + w4)
}
