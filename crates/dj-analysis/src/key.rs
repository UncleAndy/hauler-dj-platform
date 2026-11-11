//! Musical key: which of the twelve pitch classes a track is built on, and whether it is major or
//! minor.
//!
//! The measurement is the textbook one: a short-time spectrum is folded onto the twelve pitch classes
//! (a *chroma*), frames that carry an onset count for more than frames that do not, and the result is
//! correlated with the twenty-four Krumhansl–Kessler profiles — one per tonic in each mode. The pair
//! that correlates best is the answer, and the distance to the runner-up is how much the answer is
//! worth.
//!
//! What this is not: a transcription. A track whose bass walks and whose chords never settle will
//! report the profile of its most common triad, and a track in a mode the profiles do not describe
//! (Dorian, Phrygian) will be reported as its nearest major or minor neighbour. The margin is printed
//! with the key so a caller can tell a firm answer from a guess.
//!
//! **Measured state** (`docs/qa/evidence/B07`): exact on triads — a C major triad, an A minor triad and
//! an F# major triad come out right, and a synthetic triad transposed by two semitones moves the answer
//! by exactly two. On real mixes it is not yet reliable: one of three transposed excerpts kept its
//! original tonic, and the relative major/minor pair is confused often enough that the same 90-second
//! excerpt read as `A# major` at its own rate and `G minor` after a resample. The module is therefore a
//! measurement, not a published field: nothing writes it to `.djmeta` and nothing shows it in the shell
//! until the answer is checked against keys the owner knows.
//!
//! - Requirements: DJP-MEDIA-007
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::path::Path;

use dj_codec::{MediaError, StreamDecoder};

/// Which of the two modes the profiles describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Major: the third above the tonic is four semitones up.
    Major,
    /// Minor: the third above the tonic is three semitones up.
    Minor,
}

impl Mode {
    /// The mode as a word, for a label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
        }
    }
}

/// The measured key of a track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// Tonic as a pitch class: `0` is C, `1` is C♯/D♭, … `11` is B.
    pub tonic: u8,
    /// Major or minor.
    pub mode: Mode,
    /// Correlation with the winning profile, −1 … 1. Above about 0.5 the answer is firm.
    pub score: f64,
    /// How far the winner is ahead of the best profile of the *other* mode: a small margin means the
    /// track is as well described by its relative key as by its own.
    pub margin: f64,
}

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Camelot wheel numbers for the major keys, tonic `0` … `11`.
const CAMELOT_MAJOR: [u8; 12] = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];
/// Camelot wheel numbers for the minor keys, tonic `0` … `11`.
const CAMELOT_MINOR: [u8; 12] = [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10];

/// Krumhansl–Kessler major profile, tonic first.
const PROFILE_MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
/// Krumhansl–Kessler minor profile, tonic first.
const PROFILE_MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

impl Key {
    /// The key as `C`, `F#`, `A#` and the mode: `A minor`.
    pub fn name(&self) -> String {
        format!(
            "{} {}",
            NAMES[usize::from(self.tonic % 12)],
            self.mode.as_str()
        )
    }

    /// The key in Camelot notation: `8B` is C major, `8A` is A minor, and neighbours on the wheel mix.
    pub fn camelot(&self) -> String {
        let number = match self.mode {
            Mode::Major => CAMELOT_MAJOR[usize::from(self.tonic % 12)],
            Mode::Minor => CAMELOT_MINOR[usize::from(self.tonic % 12)],
        };
        let side = match self.mode {
            Mode::Major => 'B',
            Mode::Minor => 'A',
        };
        format!("{number}{side}")
    }
}

/// Lowest frequency that counts as a note: below it is rumble, not pitch.
const LOWEST_HZ: f64 = 55.0;
/// Highest frequency that counts as a note: above it a cymbal's noise says nothing about the key.
const HIGHEST_HZ: f64 = 5_000.0;
/// The analysis window, in frames. At 48 kHz this is 341 ms — 2,9 Hz between bins, which separates
/// neighbouring semitones down to the bottom of a bass line. A 170 ms window (5,9 Hz) did not: a C3
/// triad's leakage spread over nine pitch classes and the answer came out as its relative minor.
const WINDOW: usize = 16_384;
/// How far the window moves between spectra.
const HOP: usize = 4_096;
/// How much an onset counts against a sustained frame: an onset carries five times the weight.
const ONSET_WEIGHT: f64 = 4.0;

/// The twelve pitch classes of a slice of audio, each in `0.0 … 1.0`.
///
/// Frames that carry an onset are weighted more than frames that do not: a chord played on a beat says
/// what the key is, a pad held under it says less.
pub fn chroma(mono: &[f32], rate: f64) -> Option<[f64; 12]> {
    if !rate.is_finite() || rate <= 0.0 || mono.len() < WINDOW {
        return None;
    }
    let mut chroma = [0.0_f64; 12];
    let mut real = vec![0.0_f64; WINDOW];
    let mut imaginary = vec![0.0_f64; WINDOW];
    let mut previous = vec![0.0_f64; WINDOW / 2];
    let mut flux_of_frame = Vec::with_capacity(mono.len() / HOP + 1);
    let mut spectra: Vec<Vec<f64>> = Vec::with_capacity(mono.len() / HOP + 1);

    let mut at = 0;
    while at + WINDOW <= mono.len() {
        for index in 0..WINDOW {
            // A Hann window: without it the ends of the window ring across the whole spectrum.
            let phase = std::f64::consts::TAU * index as f64 / WINDOW as f64;
            real[index] = f64::from(mono[at + index]) * (0.5 - 0.5 * phase.cos());
            imaginary[index] = 0.0;
        }
        fft(&mut real, &mut imaginary);
        let mut half = vec![0.0_f64; WINDOW / 2];
        let mut flux = 0.0_f64;
        for (index, slot) in half.iter_mut().enumerate() {
            let magnitude = real[index].hypot(imaginary[index]);
            *slot = magnitude;
            let rise = magnitude - previous[index];
            if rise > 0.0 {
                flux += rise;
            }
        }
        previous.clone_from_slice(&half);
        flux_of_frame.push(flux);
        spectra.push(half);
        at += HOP;
    }
    if spectra.is_empty() {
        return None;
    }
    let loudest_flux = flux_of_frame.iter().cloned().fold(0.0_f64, f64::max);
    let mut total_weight = 0.0_f64;
    for (index, half) in spectra.iter().enumerate() {
        let onset = if loudest_flux > 0.0 {
            flux_of_frame[index] / loudest_flux
        } else {
            0.0
        };
        let weight = 1.0 + ONSET_WEIGHT * onset;
        let mut frame_chroma = [0.0_f64; 12];
        for (bin, magnitude) in half.iter().enumerate() {
            if *magnitude <= 0.0 {
                continue;
            }
            let frequency = bin as f64 * rate / WINDOW as f64;
            if !(LOWEST_HZ..=HIGHEST_HZ).contains(&frequency) {
                continue;
            }
            // The bin's pitch, as a fractional pitch class, so energy between two semitones is split
            // between them instead of being thrown at the nearer one.
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
            let class = midi.rem_euclid(12.0);
            let lower = class.floor();
            let fraction = class - lower;
            let first = lower as usize % 12;
            let second = (first + 1) % 12;
            // Energy, not amplitude: squaring pushes the window's sidelobes down while a fundamental
            // keeps its weight, which is what separates a triad from the smear around it.
            let energy = magnitude * magnitude;
            frame_chroma[first] += energy * (1.0 - fraction);
            frame_chroma[second] += energy * fraction;
        }
        // Each frame says what *it* sounds like, whatever its level: a quiet bridge and a loud drop
        // weigh the same, and the bass of one bar cannot outvote the chords of the next.
        let frame_total: f64 = frame_chroma.iter().sum();
        if frame_total > 0.0 {
            for (class, value) in frame_chroma.iter().enumerate() {
                chroma[class] += weight * value / frame_total;
            }
            total_weight += weight;
        }
    }
    if total_weight <= 0.0 {
        return None;
    }
    for value in &mut chroma {
        *value /= total_weight;
    }
    let total: f64 = chroma.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    for value in &mut chroma {
        *value /= total;
    }
    Some(chroma)
}

/// Pearson correlation of two twelve-point series.
fn correlate(left: &[f64; 12], right: &[f64; 12]) -> f64 {
    let mean_left = left.iter().sum::<f64>() / 12.0;
    let mean_right = right.iter().sum::<f64>() / 12.0;
    let mut covariance = 0.0;
    let mut spread_left = 0.0;
    let mut spread_right = 0.0;
    for index in 0..12 {
        let a = left[index] - mean_left;
        let b = right[index] - mean_right;
        covariance += a * b;
        spread_left += a * a;
        spread_right += b * b;
    }
    let denominator = (spread_left * spread_right).sqrt();
    if denominator <= f64::EPSILON {
        return 0.0;
    }
    covariance / denominator
}

/// The key of a slice of audio, or `None` when there is not enough of it to tell.
pub fn detect(mono: &[f32], rate: f64) -> Option<Key> {
    let chroma = chroma(mono, rate)?;
    let mut best: Option<Key> = None;
    for mode in [Mode::Major, Mode::Minor] {
        let profile = match mode {
            Mode::Major => PROFILE_MAJOR,
            Mode::Minor => PROFILE_MINOR,
        };
        for tonic in 0..12_u8 {
            let mut rotated = [0.0_f64; 12];
            for index in 0..12 {
                rotated[index] = profile[(index + 12 - usize::from(tonic)) % 12];
            }
            let score = correlate(&chroma, &rotated);
            if best.is_none_or(|current| score > current.score) {
                best = Some(Key {
                    tonic,
                    mode,
                    score,
                    margin: 0.0,
                });
            }
        }
    }
    let mut key = best?;
    // The margin is against the same tonic in the *other* mode: the relative major/minor pair is the
    // confusion a listener would make, and it is the one worth reporting.
    let other = match key.mode {
        Mode::Major => Mode::Minor,
        Mode::Minor => Mode::Major,
    };
    let profile = match other {
        Mode::Major => PROFILE_MAJOR,
        Mode::Minor => PROFILE_MINOR,
    };
    let mut rotated = [0.0_f64; 12];
    for index in 0..12 {
        rotated[index] = profile[(index + 12 - usize::from(key.tonic)) % 12];
    }
    key.margin = key.score - correlate(&chroma, &rotated);
    Some(key)
}

/// The key of a file, decoded in one pass.
pub fn analyse(path: &Path) -> Result<Option<Key>, MediaError> {
    let mut decoder = StreamDecoder::open(path)?;
    let rate = f64::from(decoder.sample_rate());
    let mut buffer = vec![0.0_f32; 4096 * 2];
    let mut mono = Vec::with_capacity(1 << 20);
    loop {
        let read = decoder.read_frames(&mut buffer)?;
        if read == 0 {
            break;
        }
        for frame in 0..read {
            mono.push((buffer[frame * 2] + buffer[frame * 2 + 1]) * 0.5);
        }
    }
    Ok(detect(&mono, rate))
}

/// In-place iterative radix-2 Cooley–Tukey.
fn fft(real: &mut [f64], imaginary: &mut [f64]) {
    let count = real.len();
    debug_assert_eq!(count, imaginary.len());
    debug_assert!(count.is_power_of_two());
    let mut target = 0;
    for index in 1..count {
        let mut bit = count >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target ^= bit;
        if index < target {
            real.swap(index, target);
            imaginary.swap(index, target);
        }
    }
    let mut width = 2;
    while width <= count {
        let step = std::f64::consts::TAU / width as f64;
        for start in (0..count).step_by(width) {
            for offset in 0..width / 2 {
                let (sin, cos) = (step * offset as f64).sin_cos();
                let even = start + offset;
                let odd = even + width / 2;
                let odd_real = real[odd] * cos + imaginary[odd] * sin;
                let odd_imaginary = imaginary[odd] * cos - real[odd] * sin;
                let even_real = real[even];
                let even_imaginary = imaginary[even];
                real[even] = even_real + odd_real;
                imaginary[even] = even_imaginary + odd_imaginary;
                real[odd] = even_real - odd_real;
                imaginary[odd] = even_imaginary - odd_imaginary;
            }
        }
        width <<= 1;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    fn tone(frequency: f64, seconds: f64) -> Vec<f32> {
        let count = (RATE * seconds) as usize;
        (0..count)
            .map(|index| {
                let phase = std::f64::consts::TAU * frequency * index as f64 / RATE;
                (0.25 * phase.sin()) as f32
            })
            .collect()
    }

    fn chord(frequencies: &[f64], seconds: f64) -> Vec<f32> {
        let count = (RATE * seconds) as usize;
        (0..count)
            .map(|index| {
                let sum: f64 = frequencies
                    .iter()
                    .map(|frequency| {
                        (std::f64::consts::TAU * frequency * index as f64 / RATE).sin()
                    })
                    .sum();
                (0.2 * sum) as f32
            })
            .collect()
    }

    #[test]
    fn a_c_major_triad_is_c_major() {
        let audio = chord(&[261.63, 329.63, 392.00], 3.0);
        let key = detect(&audio, RATE).expect("a key");
        assert_eq!(key.mode, Mode::Major, "{:?}", key);
        assert_eq!(key.tonic, 0, "{}", key.name());
        assert_eq!(key.name(), "C major");
        assert_eq!(key.camelot(), "8B");
    }

    #[test]
    fn an_a_minor_triad_is_a_minor() {
        let audio = chord(&[220.00, 261.63, 329.63], 3.0);
        let key = detect(&audio, RATE).expect("a key");
        assert_eq!(key.mode, Mode::Minor, "{:?}", key);
        assert_eq!(key.tonic, 9, "{}", key.name());
        assert_eq!(key.name(), "A minor");
        assert_eq!(key.camelot(), "8A");
    }

    #[test]
    fn an_f_sharp_major_triad_is_f_sharp_major() {
        let audio = chord(&[369.99, 466.16, 554.37], 3.0);
        let key = detect(&audio, RATE).expect("a key");
        assert_eq!(key.mode, Mode::Major, "{:?}", key);
        assert_eq!(key.tonic, 6, "{}", key.name());
        assert_eq!(key.name(), "F# major");
        assert_eq!(key.camelot(), "2B");
    }

    #[test]
    fn the_octave_of_the_notes_does_not_change_the_key() {
        let low = chord(&[130.81, 164.81, 196.00], 3.0);
        let high = chord(&[523.25, 659.26, 783.99], 3.0);
        assert_eq!(
            detect(&low, RATE).map(|key| key.name()),
            Some("C major".into())
        );
        assert_eq!(
            detect(&high, RATE).map(|key| key.name()),
            Some("C major".into())
        );
    }

    #[test]
    fn a_single_tone_is_a_guess_and_says_so_by_its_margin() {
        // One pitch class is not a key: the profiles cannot be told apart, and the margin is the field
        // that says the answer is not worth much.
        let audio = tone(440.0, 3.0);
        let key = detect(&audio, RATE).expect("a key");
        assert!(key.score.is_finite());
        assert!(
            key.margin.abs() < 0.5,
            "one tone should not look like a firm key: {} at {:.3} (margin {:.3})",
            key.name(),
            key.score,
            key.margin
        );
    }

    #[test]
    fn silence_has_no_key() {
        let audio = vec![0.0_f32; (RATE * 2.0) as usize];
        assert!(detect(&audio, RATE).is_none());
    }

    #[test]
    fn a_clip_too_short_to_hold_a_window_has_no_key() {
        let audio = vec![0.1_f32; 1_000];
        assert!(detect(&audio, RATE).is_none());
    }

    #[test]
    fn the_camelot_wheel_is_the_one_djs_use() {
        for (tonic, expected) in [(0_u8, "8B"), (7, "9B"), (9, "11B"), (2, "10B")] {
            let key = Key {
                tonic,
                mode: Mode::Major,
                score: 1.0,
                margin: 1.0,
            };
            assert_eq!(key.camelot(), expected, "{}", key.name());
        }
        for (tonic, expected) in [(9_u8, "8A"), (4, "9A"), (11, "10A"), (2, "7A")] {
            let key = Key {
                tonic,
                mode: Mode::Minor,
                score: 1.0,
                margin: 1.0,
            };
            assert_eq!(key.camelot(), expected, "{}", key.name());
        }
    }
}
