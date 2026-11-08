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
    /// The correction the file asks for, in decibels, before the share is taken.
    pub correction_db: f32,
    /// Linear gain the channel opens with: the correction at the share in force.
    pub gain: f32,
}

/// Measure a file and work out the gain its channel should open with.
///
/// `level` is the share of the matched level the channel then sits at: the correction is applied whole,
/// so every file lands on the same level, and this factor takes that one level down (or, at 1,0, leaves
/// it alone). It comes from the settings (`dj-config`'s `AnalysisSettings`), because the matched level
/// itself was a decibel too hot for the meters.
pub fn analyse(path: &Path, level: f32) -> Result<Loudness, MediaError> {
    let rms_db = rms_db(path)?;
    let correction_db = rms_db.map_or(0.0, correction_for);
    Ok(Loudness {
        rms_db,
        correction_db,
        gain: gain_for(correction_db, level),
    })
}

/// The correction `rms_db` asks for, in decibels, clamped so a broken measurement cannot mute a track
/// or push it into the limiter.
pub fn correction_for(rms_db: f32) -> f32 {
    if !rms_db.is_finite() {
        return 0.0;
    }
    (TARGET_RMS_DB - rms_db).clamp(-MAX_CORRECTION_DB, MAX_CORRECTION_DB)
}

/// The linear gain of `correction_db`, then scaled by `level`.
///
/// The two are separate on purpose. The correction is what makes one file match another, and it is
/// applied whole: a share of it would leave loud files loud and quiet files quiet, which is the one
/// thing level matching exists to prevent. `level` then moves the now-common level — 0,9 is about a
/// decibel down for all of them, 1,0 leaves it alone, and 0,0 is silence.
pub fn gain_for(correction_db: f32, level: f32) -> f32 {
    if !correction_db.is_finite() {
        return 1.0;
    }
    let level = if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        1.0
    };
    10f32.powf(correction_db / 20.0) * level
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
        let whole = 1.0;
        assert!(
            (gain_for(correction_for(TARGET_RMS_DB), whole) - 1.0).abs() < 1e-6,
            "unity is the target"
        );
        let loud = gain_for(correction_for(-3.0), whole);
        assert!((loud - 0.355).abs() < 0.01, "a -3 dBFS track got {loud}");
        let quiet = gain_for(correction_for(-24.0), whole);
        assert!((quiet - 3.981).abs() < 0.05, "a -24 dBFS track got {quiet}");
        assert_eq!(
            gain_for(correction_for(f32::NAN), whole),
            1.0,
            "a broken measurement must not attenuate"
        );
    }

    #[test]
    fn the_correction_is_whole_and_the_level_is_scaled() {
        // A file eight decibels low asks for a +7,67 dB lift, and that lift is applied whole: the
        // matching is what makes two files sit at one level. What is scaled is that one level — nine
        // tenths of it is 0,92 dB down for every file, so they stay matched to each other.
        let correction = correction_for(-19.67);
        assert!((correction - 7.67).abs() < 0.01, "{correction}");
        let full = 20.0 * gain_for(correction, 1.0).log10();
        let shared = 20.0 * gain_for(correction, 0.9).log10();
        assert!((full - 7.67).abs() < 0.01, "{full}");
        assert!(
            (shared - (7.67 - 0.915)).abs() < 0.01,
            "the level factor should take 0,92 dB off the whole level, not a tenth of the correction:              {shared} dB"
        );
        // The point of applying the correction whole: a quiet and a loud file end up level with each
        // other, and both sit the same distance below the target — that is what a share of the
        // correction would break.
        let quiet = -24.0 + 20.0 * gain_for(correction_for(-24.0), 0.9).log10();
        let loud = -9.0 + 20.0 * gain_for(correction_for(-9.0), 0.9).log10();
        assert!(
            (quiet - loud).abs() < 0.01,
            "two files were left {:.2} dB apart after matching",
            quiet - loud
        );
        let below_target = TARGET_RMS_DB + 20.0 * 0.9f32.log10();
        assert!(
            (loud - below_target).abs() < 0.01,
            "the matched level sits at {loud} dB against the {below_target} dB the factor asks for"
        );
        assert_eq!(
            gain_for(correction, 0.0),
            0.0,
            "a level of zero is silence, not a silent correction"
        );
        // A factor outside the range is clamped, not obeyed: a settings file cannot invert a gain.
        assert!((gain_for(correction, 2.0) - gain_for(correction, 1.0)).abs() < 1e-6);
        assert_eq!(gain_for(correction, -1.0), 0.0);
    }

    #[test]
    fn the_correction_is_capped_in_both_directions() {
        // A silent file measured at -80 dBFS must not get a 68 dB lift, and a clipped one must not be
        // attenuated by 20: the auto-gain trims a track, it does not remaster it.
        assert!(
            gain_for(correction_for(-80.0), 1.0) <= 10f32.powf(MAX_CORRECTION_DB / 20.0) + 1e-4
        );
        assert!(gain_for(correction_for(0.0), 1.0) >= 10f32.powf(-MAX_CORRECTION_DB / 20.0) - 1e-4);
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
