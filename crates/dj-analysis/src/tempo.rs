//! Tempo and the first beat of the grid (B02, DJP-MEDIA-006, DJP-SYNC-003).
//!
//! The detector is deliberately classical and has no model behind it: a spectral flux envelope, the
//! spectrum of that envelope (whose lowest strong peak is the beat rate), and a comb over the phase to
//! place the grid. Every number below was chosen by measurement on click tracks with a known tempo and
//! a known offset — see `tests/tempo.rs` and `docs/qa/evidence/A15/summary.md`.
//!
//! What it does not do: half/double correction by ear (that is a control in the UI, DJP-SYNC-003),
//! tempo maps (DJP-SYNC-004), or any decision about which beat is the downbeat.

use dj_codec::{MediaError, StreamDecoder};
use dj_signalsmith_sys::Spectrum;

/// Analysis block, in samples: 1024 at 48 kHz is 21 ms — long enough to see a kick, short enough that
/// two sixteenths never merge into one onset.
pub const BLOCK: usize = 1024;

/// Hop between blocks: 256 samples is 5,3 ms at 48 kHz, so a beat lands within a hundredth of a beat.
pub const HOP: usize = 256;

/// Tempo range the search covers, in beats per minute.
pub const MIN_BPM: f64 = 60.0;
/// Upper end of the search: above this the search starts finding eighths, not beats.
pub const MAX_BPM: f64 = 200.0;

/// A peak counts as the fundamental when it carries this share of the band's strongest peak. Below
/// this the peak is a rumble of the analysis, and the beat is one of the harmonics.
const FUNDAMENTAL_SHARE: f32 = 0.35;

/// Below this the band is noise rather than a beat, and the file gets no tempo instead of a guess.
const MIN_CONFIDENCE: f32 = 0.35;

/// How much of the file the detector reads, in seconds. Tempo does not change mid-track, and a DJ
/// waits for the number: the first two minutes carry the information, the rest is time the user spends
/// looking at a spinner.
pub const ANALYSED_SECONDS: f64 = 120.0;

/// What the analysis found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    /// Beats per minute, as measured (not corrected to a half or double).
    pub bpm: f64,
    /// Frame of the file's first beat: the grid is `first_beat_frame + k · 60 / bpm · rate`.
    pub first_beat_frame: u64,
    /// How sure the measurement is, 0 … 1: the winning autocorrelation peak against the runner-up.
    pub confidence: f32,
}

/// Spectral flux: the positive part of the change in magnitude between two blocks.
///
/// Level rises equally with every instrument; flux rises with *onsets*, which is what a beat is. The
/// envelope is one value per hop, in the frame scale of the file (`n · HOP`).
pub fn onset_envelope(mono: &[f32]) -> Option<Vec<f32>> {
    let mut spectrum = Spectrum::new(BLOCK)?;
    let bins = BLOCK / 2;
    let mut magnitudes = vec![0.0f32; bins];
    let mut previous = vec![0.0f32; bins];
    let mut block = vec![0.0f32; BLOCK];
    let mut flux = Vec::with_capacity(mono.len() / HOP + 1);

    let mut start = 0;
    while start + BLOCK <= mono.len() {
        block.copy_from_slice(&mono[start..start + BLOCK]);
        spectrum.magnitudes(&block, &mut magnitudes);
        let mut sum = 0.0f32;
        for (bin, before) in magnitudes.iter().zip(previous.iter()) {
            sum += (bin - before).max(0.0);
        }
        flux.push(sum);
        previous.copy_from_slice(&magnitudes);
        start += HOP;
    }
    // A handful of blocks cannot carry a tempo; refusing beats returning a number that means nothing.
    if flux.len() < 64 {
        return None;
    }
    // A slowly moving mean is subtracted so a loud section does not outweigh a quiet one: the tempo is
    // in the pattern, not in the level.
    Some(remove_the_running_mean(flux, 94))
}

/// Subtract a centred moving average of `window` envelope points, and clamp the rest at zero.
///
/// Centred, not trailing: a trailing mean delays the whole envelope by half its window, and the phase
/// of the first beat is read straight off this envelope.
fn remove_the_running_mean(flux: Vec<f32>, window: usize) -> Vec<f32> {
    let half = (window / 2).max(1);
    let mut out = Vec::with_capacity(flux.len());
    for index in 0..flux.len() {
        let from = index.saturating_sub(half);
        let to = (index + half + 1).min(flux.len());
        let mean: f64 = flux[from..to]
            .iter()
            .map(|value| f64::from(*value))
            .sum::<f64>()
            / (to - from) as f64;
        out.push((f64::from(flux[index]) - mean).max(0.0) as f32);
    }
    out
}

/// Detect the tempo and the first beat of a mono signal.
pub fn detect(mono: &[f32], rate: f64) -> Option<Tempo> {
    let envelope = onset_envelope(mono)?;
    let per_second = rate / HOP as f64;
    let (bpm, confidence) = tempo_from_spectrum(&envelope, per_second)?;
    let period = per_second * 60.0 / bpm;
    let phase = best_phase(&envelope, period);
    // The grid is periodic, so what matters is its phase, not which beat is "the first one": a track
    // with a silent intro has the same grid as one that starts on the beat, and beat lines over the
    // intro are normal in DJ software. The anchor is therefore the earliest beat at or after the file
    // start — never negative, because the metadata stores a frame, not an offset into silence.
    //
    // Half a block: the block that saw the onset starts at `phase · HOP`, and flux rises as soon as its
    // 21 ms window touches the beat, so the block's centre is the better estimate of where the beat is.
    // Measured on click tracks: 1 … 27 ms late over 72 … 174 BPM, against 5 … 38 ms early without it.
    let first_beat_frame = (phase * HOP as f64 + BLOCK as f64 / 2.0).round().max(0.0) as u64;
    Some(Tempo {
        bpm,
        first_beat_frame,
        confidence,
    })
}

/// The tempo, read off the spectrum of the onset envelope.
///
/// A beat pattern is periodic, so its envelope has energy at the beat frequency and at its harmonics.
/// The *lowest* strong peak is the beat rate: a click on every beat gives that peak at the tempo, while
/// the same track read at half speed would put it an octave lower, where there is nothing. Picking the
/// lowest strong peak is what keeps a track at 174 from being reported as 87.
fn tempo_from_spectrum(envelope: &[f32], per_second: f64) -> Option<(f64, f32)> {
    // A power of two, long enough to separate 1 Hz from its neighbours: 8192 points is 44 s at the hop
    // rate, and the envelope is usually shorter, which is only a little padding.
    let size = envelope.len().clamp(2_048, 16_384).next_power_of_two();
    let mut spectrum = Spectrum::new(size)?;
    let mut block = vec![0.0f32; size];
    let take = envelope.len().min(size);
    block[..take].copy_from_slice(&envelope[..take]);
    let mut magnitudes = vec![0.0f32; size / 2];
    spectrum.magnitudes(&block, &mut magnitudes);

    let bin_hz = per_second / size as f64;
    let lowest = (0.6 / bin_hz).ceil() as usize;
    let highest = (3.6 / bin_hz).floor() as usize;
    let band = &magnitudes[lowest.min(magnitudes.len())..highest.min(magnitudes.len())];
    if band.is_empty() {
        return None;
    }
    let strongest = band.iter().cloned().fold(0.0f32, f32::max);
    if strongest <= 0.0 {
        return None;
    }
    // The lowest peak that carries a real share of the band's strongest: the fundamental, unless the
    // fundamental is only a rumble and the beat is the harmonic.
    let mut chosen = None;
    for bin in lowest + 1..highest.saturating_sub(1) {
        let value = magnitudes[bin];
        if value < strongest * FUNDAMENTAL_SHARE {
            continue;
        }
        if value >= magnitudes[bin - 1] && value >= magnitudes[bin + 1] {
            chosen = Some(bin);
            break;
        }
    }
    let bin = chosen?;
    // Parabolic fit over the peak and its neighbours: 8192 points give a bin coarse enough that
    // rounding alone would cost a whole tenth of a BPM.
    let (left, centre, right) = (
        f64::from(magnitudes[bin - 1]),
        f64::from(magnitudes[bin]),
        f64::from(magnitudes[bin + 1]),
    );
    let denominator = left - 2.0 * centre + right;
    let offset = if denominator.abs() > f64::EPSILON {
        (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let hertz = (bin as f64 + offset) * bin_hz;
    let bpm = hertz * 60.0;
    if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
        return None;
    }
    // How much the band stands out: a held note has no beat, and its band is flat noise.
    let mut sorted = band.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = f64::from(sorted[sorted.len() / 2]);
    let confidence = if f64::from(strongest) > median {
        (1.0 - median / f64::from(strongest)).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    if confidence < MIN_CONFIDENCE {
        return None;
    }
    Some((bpm, confidence))
}

/// The phase of the grid, in hops: where inside one period the beats start.
///
/// Searched in quarter-hop steps and then refined by a parabola through the best three, because the
/// guess at whole hops alone would be worth ten milliseconds at a fast tempo — audible as a grid that
/// sits behind the kick.
fn best_phase(envelope: &[f32], period: f64) -> f64 {
    const STEP: f64 = 0.25;
    let steps = ((period / STEP).ceil() as usize).clamp(4, 8_192);
    let mut scores = Vec::with_capacity(steps);
    let mut best = (0.0f64, f64::NEG_INFINITY);
    let mut best_index = 0usize;
    for step in 0..steps {
        let phase = step as f64 * STEP;
        let score = comb_score(envelope, period, phase);
        scores.push(score);
        if score > best.1 {
            best = (phase, score);
            best_index = step;
        }
    }
    // Parabola through the neighbours of the winner: the true phase is between two quarter hops.
    if best_index > 0 && best_index + 1 < scores.len() {
        let (left, centre, right) = (
            scores[best_index - 1],
            scores[best_index],
            scores[best_index + 1],
        );
        let denominator = left - 2.0 * centre + right;
        if denominator.abs() > f64::EPSILON {
            let offset = 0.5 * (left - right) / denominator;
            return (best.0 + offset.clamp(-0.5, 0.5) * STEP).max(0.0);
        }
    }
    best.0
}

/// Mean onset strength on the grid of `period` hops starting at `phase`.
fn comb_score(envelope: &[f32], period: f64, phase: f64) -> f64 {
    let mut sum = 0.0f64;
    let mut counted = 0usize;
    let mut position = phase;
    let length = envelope.len() as f64;
    while position < length && counted < 1_024 {
        let index = position as usize;
        let fraction = position - index as f64;
        let value = if index + 1 < envelope.len() {
            f64::from(envelope[index]) * (1.0 - fraction)
                + f64::from(envelope[index + 1]) * fraction
        } else {
            f64::from(envelope[index])
        };
        sum += value;
        counted += 1;
        position += period;
    }
    if counted == 0 {
        0.0
    } else {
        sum / counted as f64
    }
}

/// Read the head of a file as mono and measure its tempo.
pub fn analyse(path: &std::path::Path) -> Result<Option<Tempo>, MediaError> {
    let mut decoder = StreamDecoder::open(path)?;
    let rate = decoder.sample_rate();
    let limit = (f64::from(rate) * ANALYSED_SECONDS) as usize;
    let mut buffer = vec![0.0f32; 4096 * 2];
    let mut mono = Vec::with_capacity(limit.min(1 << 22));
    while mono.len() < limit {
        let read = decoder.read_frames(&mut buffer)?;
        if read == 0 {
            break;
        }
        for frame in 0..read {
            mono.push((buffer[frame * 2] + buffer[frame * 2 + 1]) * 0.5);
        }
    }
    Ok(detect(&mono, f64::from(rate)))
}
