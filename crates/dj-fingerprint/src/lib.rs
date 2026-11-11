//! Audio fingerprints for the collection (DJP-DATA, duplicates).
//!
//! A file's path, size and hash answer "is this the same file". They do not answer "is this the same
//! recording": a track re-encoded to mp3, moved off a read-only disk or bought twice is a different file
//! and the same music. An acoustic fingerprint answers that, and Chromaprint's is the one with a decade of
//! tuning behind it.
//!
//! The implementation is vendored under `vendor/chromaprint-next` with its LGPL resampler replaced by a
//! polyphase Kaiser-windowed sinc written for this tree, so the whole crate is MIT like the rest of the
//! platform (see `THIRD_PARTY.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub use chromaprint::{Algorithm, Error, Fingerprinter, decode_fingerprint, encode_fingerprint};

/// The rate Chromaprint analyses at: everything is resampled to it first.
pub const ANALYSIS_RATE: u32 = 11_025;

/// How close two fingerprints stand, at their best relative position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agreement {
    /// Share of bits that differ, 0 is the same recording, about 0.5 is two strangers.
    pub ber: f64,
    /// The shift of the second fingerprint, in its own frames, that gave this number.
    pub lag: i32,
}

impl Agreement {
    /// Share of bits that agree, the friendlier way to say it.
    pub fn same(&self) -> f64 {
        1.0 - self.ber
    }
}

/// Fingerprint interleaved `i16` audio at its own rate and channel count.
///
/// Chromaprint mixes to mono and resamples to [`ANALYSIS_RATE`] itself; the caller hands over the file as
/// it decoded it.
pub fn of_samples(samples: &[i16], rate: u32, channels: u16) -> Result<Vec<u32>, Error> {
    let mut fingerprinter = Fingerprinter::new(Algorithm::Test2);
    fingerprinter.start(rate, channels)?;
    fingerprinter.feed(samples)?;
    fingerprinter.finish()?;
    Ok(fingerprinter.fingerprint().to_vec())
}

/// The best agreement between two fingerprints over small shifts of the second one.
///
/// Two copies of one recording do not start at the same frame: a different encoder adds or drops a few
/// milliseconds of silence, and a fingerprint compared frame-to-frame would read that as a stranger. The
/// shift is what makes the comparison about the music and not about the lead-in.
pub fn agreement(first: &[u32], second: &[u32], max_lag: i32) -> Agreement {
    let mut best = Agreement {
        ber: f64::INFINITY,
        lag: 0,
    };
    for lag in -max_lag..=max_lag {
        let mut differing = 0_u64;
        let mut compared = 0_u64;
        for (index, left) in first.iter().enumerate() {
            let other = index as i64 + i64::from(lag);
            if other < 0 || other as usize >= second.len() {
                continue;
            }
            differing += u64::from((left ^ second[other as usize]).count_ones());
            compared += 32;
        }
        // A shift that leaves a handful of frames compared says nothing; the middle of the overlap is
        // what the number is made of.
        if compared < 32 * 8 {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let ber = differing as f64 / compared as f64;
        if ber < best.ber {
            best = Agreement { ber, lag };
        }
    }
    if !best.ber.is_finite() {
        return Agreement { ber: 0.5, lag: 0 };
    }
    best
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// A sweep whose spectrogram is distinctive: every second moves the tone, so two different sweeps
    /// cannot look alike.
    fn sweep(from: f64, to: f64, seconds: f64, rate: u32) -> Vec<i16> {
        let count = (seconds * f64::from(rate)) as usize;
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let t = index as f64 / f64::from(rate);
            let progress = t / seconds;
            let frequency = from + (to - from) * progress;
            let phase = 2.0 * std::f64::consts::PI * frequency * t;
            out.push((phase.sin() * 12_000.0) as i16);
        }
        out
    }

    #[test]
    fn the_same_recording_is_recognised_after_a_small_shift_and_a_noisier_copy() {
        let rate = 48_000;
        let original = sweep(220.0, 1_760.0, 12.0, rate);
        let mut shifted = vec![0_i16; 5_000];
        shifted.extend_from_slice(&original);
        // A re-encode is not the same numbers: one part in a thousand of noise on top of the signal.
        let mut noisy = Vec::with_capacity(shifted.len());
        let mut seed = 0x1234_5678_u32;
        for sample in &shifted {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let noise = ((seed >> 16) as i32 - 32_768) / 64;
            noisy.push((i32::from(*sample) + noise).clamp(-32_768, 32_767) as i16);
        }

        let first = of_samples(&original, rate, 1).expect("the first fingerprint");
        let second = of_samples(&noisy, rate, 1).expect("the second fingerprint");
        let other = of_samples(&sweep(300.0, 3_000.0, 12.0, rate), rate, 1).expect("a stranger");

        let same = agreement(&first, &second, 16);
        let different = agreement(&first, &other, 16);
        println!(
            "same recording: ber {:.4} at lag {}, a stranger: ber {:.4} at lag {}",
            same.ber, same.lag, different.ber, different.lag
        );
        // Synthetic sweeps are close cousins: both are chroma-rich tones sliding through the same
        // spectrum, so the stranger is not a stranger in the way two songs are. What this test proves is
        // the pipeline and the direction of the numbers; the threshold itself is measured on real
        // records with `examples/duplicates.rs`, not assumed from a sine.
        assert!(
            same.ber < 0.1,
            "the same recording should agree closely, ber {:.4}",
            same.ber
        );
        assert!(
            same.ber * 4.0 < different.ber,
            "the copy must be far closer than the stranger: copy {:.4}, stranger {:.4}",
            same.ber,
            different.ber
        );
    }
}
