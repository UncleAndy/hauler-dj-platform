//! Which of two files of the same recording is the better copy (DJP-MEDIA-005).
//!
//! The order is the owner's, and the first difference decides: lossless over lossy, then sample rate,
//! then bit depth, then bitrate. Bit depth and the measured spectral band come with the quality slice;
//! what is here is what the import can know without decoding twice.

use crate::identity::Identity;

/// What a file says about its own quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    /// FLAC, WAV, ALAC: every sample kept.
    pub lossless: bool,
    pub sample_rate: u32,
    /// Bits per sample, when the container says.
    pub bits: Option<u32>,
    /// Bits per second, averaged over the file: honest for VBR.
    pub bitrate: Option<u64>,
}

impl Quality {
    /// The quality an identity carries: what is known without opening the file again.
    pub fn of(identity: &Identity) -> Self {
        Self {
            lossless: identity.lossless,
            sample_rate: identity.sample_rate,
            bits: identity.bits,
            bitrate: identity.bitrate(),
        }
    }
}

/// True when `candidate` is strictly better than `existing`.
///
/// Strictly: two copies of the same recording at the same quality do not replace each other, because
/// replacing one with the other gains nothing and loses the metadata the first one may carry.
pub fn better(candidate: &Quality, existing: &Quality) -> bool {
    if candidate.lossless != existing.lossless {
        return candidate.lossless;
    }
    if candidate.sample_rate != existing.sample_rate {
        return candidate.sample_rate > existing.sample_rate;
    }
    if candidate.bits != existing.bits {
        return candidate.bits.unwrap_or(0) > existing.bits.unwrap_or(0);
    }
    candidate.bitrate.unwrap_or(0) > existing.bitrate.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quality(
        lossless: bool,
        sample_rate: u32,
        bits: Option<u32>,
        bitrate: Option<u64>,
    ) -> Quality {
        Quality {
            lossless,
            sample_rate,
            bits,
            bitrate,
        }
    }

    #[test]
    fn lossless_beats_lossy_whatever_the_numbers() {
        let flac = quality(true, 44_100, Some(16), Some(900_000));
        let mp3 = quality(false, 48_000, None, Some(320_000));
        assert!(better(&flac, &mp3));
        assert!(!better(&mp3, &flac));
    }

    #[test]
    fn a_higher_sample_rate_wins_between_two_lossless_copies() {
        let high = quality(true, 96_000, Some(24), Some(2_300_000));
        let low = quality(true, 44_100, Some(16), Some(1_000_000));
        assert!(better(&high, &low));
        assert!(!better(&low, &high));
    }

    #[test]
    fn bitrate_decides_only_between_equal_formats() {
        let fat = quality(false, 44_100, None, Some(320_000));
        let thin = quality(false, 44_100, None, Some(128_000));
        assert!(better(&fat, &thin));
        assert!(!better(&thin, &fat));
    }

    #[test]
    fn the_same_quality_replaces_nothing() {
        let one = quality(false, 44_100, None, Some(320_000));
        assert!(!better(&one, &one));
    }
}
