//! Integrated loudness of a file, for the gain a channel opens with.
//!
//! A mixer channel expects a track somewhere near its nominal level: pro software analyses every track
//! and trims it so the fader and the EQ start from the same place. Without that, one file is 6 dB
//! hotter than the next and the DJ spends the set correcting gain instead of mixing — and an EQ boost
//! on the hot one lands in the limiter.
//!
//! The measurement is a gated RMS over 0.4 s blocks: block energies 20 dB below the loudest block are
//! left out, so a long intro of silence does not drag the number down. No K-weighting, no LUFS: this
//! is a level match, not a compliance meter, and the target is chosen with headroom for the EQ.

use std::path::Path;

use dj_codec::{MediaError, StreamDecoder};

/// Level a track is trimmed to, in dBFS RMS.
///
/// -12 dBFS leaves room for a +6 dB EQ boost and for the peaks above the RMS: a normal master sits
/// around -9 dBFS RMS, so the correction is usually a few decibels down, not up.
pub const TARGET_RMS_DB: f32 = -12.0;

/// Largest correction the auto-gain may apply, in either direction.
pub const MAX_CORRECTION_DB: f32 = 12.0;

/// Length of one gating block, in seconds.
const BLOCK_SECONDS: f64 = 0.4;

/// How far below the loudest block a block is ignored, in decibels.
const GATE_BELOW_LOUDEST_DB: f32 = 20.0;

/// What the analysis found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Gated RMS of the whole file, in dBFS. `None` when the file is silent.
    pub rms_db: Option<f32>,
    /// Linear gain that brings the file to [`TARGET_RMS_DB`], clamped to the correction limit.
    pub gain: f32,
}

/// Measure a file and work out the gain its channel should open with.
pub fn analyse(path: &Path) -> Result<Loudness, MediaError> {
    let rms_db = rms_db(path)?;
    Ok(Loudness {
        rms_db,
        gain: rms_db.map_or(1.0, gain_for),
    })
}

/// The gain that brings `rms_db` to the target, clamped so a broken measurement cannot mute a track
/// or push it into the limiter.
pub fn gain_for(rms_db: f32) -> f32 {
    if !rms_db.is_finite() {
        return 1.0;
    }
    let correction = (TARGET_RMS_DB - rms_db).clamp(-MAX_CORRECTION_DB, MAX_CORRECTION_DB);
    10f32.powf(correction / 20.0)
}

/// Gated RMS of a file, in dBFS.
pub fn rms_db(path: &Path) -> Result<Option<f32>, MediaError> {
    let mut decoder = StreamDecoder::open(path)?;
    let rate = decoder.sample_rate();
    let block_frames = ((f64::from(rate) * BLOCK_SECONDS) as usize).max(1);
    let mut buffer = vec![0.0f32; block_frames * 2];
    let mut blocks: Vec<f64> = Vec::new();
    let mut filled = 0usize;
    let mut energy = 0.0f64;

    loop {
        let read = decoder.read_frames(&mut buffer[filled * 2..])?;
        if read == 0 {
            break;
        }
        for frame in 0..read {
            let left = f64::from(buffer[frame * 2]);
            let right = f64::from(buffer[frame * 2 + 1]);
            energy += (left * left + right * right) / 2.0;
        }
        filled += read;
        if filled == block_frames {
            blocks.push(energy / (filled as f64));
            filled = 0;
            energy = 0.0;
        }
    }
    if filled > 0 {
        blocks.push(energy / (filled as f64));
    }
    if blocks.is_empty() {
        return Ok(None);
    }

    Ok(gated_rms_db(&blocks))
}

/// Gated mean power of block energies, in dBFS.
///
/// The gate: silence between tracks, long intros, a fade from nothing — none of it is the level of the
/// material, and averaging it in would make a quiet track out of a loud one.
fn gated_rms_db(blocks: &[f64]) -> Option<f32> {
    let loudest = blocks.iter().copied().fold(0.0f64, f64::max);
    if loudest <= 0.0 {
        return None;
    }
    let floor = loudest * 10f64.powf(-f64::from(GATE_BELOW_LOUDEST_DB) / 10.0);
    let kept: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|block| *block >= floor)
        .collect();
    let mean = kept.iter().sum::<f64>() / kept.len() as f64;
    if mean <= 0.0 {
        return None;
    }
    Some(10.0 * mean.log10() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loud_file_is_turned_down_and_a_quiet_one_up() {
        assert!(
            (gain_for(TARGET_RMS_DB) - 1.0).abs() < 1e-6,
            "unity is the target"
        );
        let loud = gain_for(-3.0);
        assert!((loud - 0.355).abs() < 0.01, "a -3 dBFS track got {loud}");
        let quiet = gain_for(-24.0);
        assert!((quiet - 3.981).abs() < 0.05, "a -24 dBFS track got {quiet}");
        assert_eq!(
            gain_for(f32::NAN),
            1.0,
            "a broken measurement must not attenuate"
        );
    }

    #[test]
    fn the_correction_is_capped_in_both_directions() {
        // A silent file measured at -80 dBFS must not get a 68 dB lift, and a clipped one must not be
        // attenuated by 20: the auto-gain trims a track, it does not remaster it.
        assert!(gain_for(-80.0) <= 10f32.powf(MAX_CORRECTION_DB / 20.0) + 1e-4);
        assert!(gain_for(0.0) >= 10f32.powf(-MAX_CORRECTION_DB / 20.0) - 1e-4);
    }

    #[test]
    fn silence_is_gated_out_of_the_level() {
        // Ten loud blocks and thirty near-silent ones: the level is the loud material, not the average
        // of the material and the gaps between tracks.
        let mut blocks = vec![1e-2f64; 10];
        blocks.extend(std::iter::repeat_n(1e-10f64, 30));
        let measured = gated_rms_db(&blocks).unwrap_or_else(|| panic!("not silent"));
        assert!(
            (measured + 20.0).abs() < 0.1,
            "a -20 dBFS passage with gaps measured {measured:.2} dB"
        );
        assert_eq!(gated_rms_db(&[0.0; 8]), None);
    }
}
