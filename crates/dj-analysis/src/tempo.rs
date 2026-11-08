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

/// Share of the onset energy that must sit on the grid, measured on three signals: 0,93 for a click
/// track, 0,39 for a real drum loop (off-beat hats are exactly the energy a grid does not explain), and
/// 0,02 … 0,04 for an aperiodic line. 0,15 sits between the music and the noise with room on both
/// sides; anything tighter would refuse real tracks, which is the defect this gate was added for.
const MIN_EXPLAINED_SHARE: f64 = 0.15;

/// How much of the file the detector reads, in seconds. Four minutes covers every intro and breakdown
/// a track is likely to open with, and a DJ waits for the number.
pub const ANALYSED_SECONDS: f64 = 240.0;

/// Length of one window the tempo is looked for in, in seconds.
pub const WINDOW_SECONDS: f64 = 40.0;

/// Distance between those windows. They do not overlap: a tempo is either in a window or it is not, and
/// the point of having several is that an intro without a beat gets outvoted by the rest of the track.
pub const WINDOW_HOP_SECONDS: f64 = 40.0;

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
    detect_in_envelope(&envelope, rate)
}

/// The same, for an envelope someone already has: the analysis decodes the file once and works on it.
pub fn detect_in_envelope(envelope: &[f32], rate: f64) -> Option<Tempo> {
    let per_second = rate / HOP as f64;
    let (bpm, clear) = choose_tempo(envelope, per_second)?;
    // The spectrum is coarse by construction: it says "128-ish". The comb over the whole analysed span
    // says "this period and no other", which is what keeps a five-minute track from drifting away from
    // its own grid by a beat.
    let period = refine_period(envelope, per_second * 60.0 / bpm);
    if period <= 0.0 {
        return None;
    }
    let phase = best_phase(envelope, period);
    // How much of the onset energy actually sits on this grid: a real beat explains nearly all of it,
    // while a rubato line or a sparse noise explains a fraction. This is the confidence that matters —
    // the spectrum can be sharp about a tempo nothing is playing to.
    // Measured where the tempo is actually clear, not over the whole file: an intro that explains
    // nothing would otherwise veto a grid the rest of the track makes perfectly well.
    let inside = (phase - clear.start as f64).rem_euclid(period);
    let explained = explained_share(&envelope[clear.clone()], period, inside);
    if explained < MIN_EXPLAINED_SHARE {
        return None;
    }
    let bpm = per_second * 60.0 / period;
    // Half a block: the block that saw the onset starts at `phase · HOP`, and flux rises as soon as its
    // 21 ms window touches the beat, so the block's centre is the better estimate of where the beat is.
    // Measured on click tracks: 1 … 27 ms late over 72 … 174 BPM, against 5 … 38 ms early without it.
    let first_beat_frame = (phase * HOP as f64 + BLOCK as f64 / 2.0).round().max(0.0) as u64;
    Some(Tempo {
        bpm,
        first_beat_frame,
        confidence: explained as f32,
    })
}

/// The tempo of a track, read from several windows of its envelope.
///
/// A track can open with an intro that has no beat at all — a pad, a spoken word, a rubato line. Read as
/// one block of audio, that intro dilutes the beat and the detector refuses the file. Read window by
/// window and the rest of the track still answers: the tempo that most of the windows agree on wins, so
/// an intro can add a vote but not cast the deciding one.
fn choose_tempo(envelope: &[f32], per_second: f64) -> Option<(f64, std::ops::Range<usize>)> {
    let window = (WINDOW_SECONDS * per_second) as usize;
    let hop = (WINDOW_HOP_SECONDS * per_second).max(1.0) as usize;
    if window < 64 || envelope.len() <= window {
        let (bpm, _) = tempo_from_spectrum(envelope, per_second)?;
        return Some((bpm, 0..envelope.len()));
    }
    // Each answer remembers where it was found: the gate at the end asks "does this grid explain the
    // audio", and it has to ask a window that has a beat in it.
    let mut found: Vec<(f64, f32, std::ops::Range<usize>)> = Vec::new();
    let mut start = 0;
    while start + window <= envelope.len() {
        if let Some((bpm, confidence)) =
            tempo_from_spectrum(&envelope[start..start + window], per_second)
        {
            found.push((bpm, confidence, start..start + window));
        }
        start += hop;
    }
    // A tail shorter than a window is still worth a look when the rest of the file was silent or
    // beatless: half a window is enough for the spectrum to see a beat.
    if start < envelope.len()
        && envelope.len() - start >= window / 2
        && let Some((bpm, confidence)) = tempo_from_spectrum(&envelope[start..], per_second)
    {
        found.push((bpm, confidence, start..envelope.len()));
    }
    if found.is_empty() {
        return None;
    }
    if found.len() == 1 {
        let only = found.first()?;
        return Some((only.0, only.2.clone()));
    }

    // Cluster the answers by tempo: windows of the same track land within a fraction of a percent of
    // each other, and a window that saw a doubled or halved pattern lands an octave away, in its own
    // cluster. The cluster with the most confidence behind it is the answer.
    let mut ordered = found.clone();
    ordered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut best: Option<(f64, f64, usize, std::ops::Range<usize>, f32)> = None;
    let mut index = 0;
    while index < ordered.len() {
        let mut weight = 0.0f64;
        let mut moment = 0.0f64;
        let mut votes = 0usize;
        let mut clearest = (0.0f32, 0..0usize);
        let anchor = ordered[index].0;
        let mut inner = index;
        while inner < ordered.len() {
            let (bpm, confidence, range) = &ordered[inner];
            // Within 3 % is the same tempo: the spectral peak is coarse, the windows are not exact.
            if (bpm / anchor).log2().abs() > 0.03 {
                break;
            }
            let confidence_value = f64::from(*confidence);
            weight += confidence_value;
            moment += confidence_value * bpm;
            votes += 1;
            if *confidence > clearest.0 {
                clearest = (*confidence, range.clone());
            }
            inner += 1;
        }
        if best
            .as_ref()
            .is_none_or(|(best_weight, _, _, _, _)| weight > *best_weight)
        {
            best = Some((weight, moment / weight, votes, clearest.1, clearest.0));
        }
        index = inner.max(index + 1);
    }
    let (_, bpm, _, clear, _) = best?;
    Some((bpm, clear))
}

/// The period the comb likes best around a hint, searched over ±0.6 % in steps of 0.02 %.
fn refine_period(envelope: &[f32], hint: f64) -> f64 {
    if !hint.is_finite() || hint <= 1.0 {
        return hint;
    }
    let mut best = (hint, f64::NEG_INFINITY);
    for step in -30..=30 {
        let period = hint * (1.0 + f64::from(step) * 0.0002);
        if period <= 1.0 {
            continue;
        }
        let score = best_phase_and_score(envelope, period).1;
        if score > best.1 {
            best = (period, score);
        }
    }
    best.0
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
fn best_phase(envelope: &[f32], period: f64) -> f64 {
    best_phase_and_score(envelope, period).0
}

/// The phase of the grid and how well the comb fits there.
///
/// Searched in quarter-hop steps and then refined by a parabola through the best three, because the
/// guess at whole hops alone would be worth ten milliseconds at a fast tempo — audible as a grid that
/// sits behind the kick.
fn best_phase_and_score(envelope: &[f32], period: f64) -> (f64, f64) {
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
            return ((best.0 + offset.clamp(-0.5, 0.5) * STEP).max(0.0), best.1);
        }
    }
    best
}

/// Share of the envelope's energy that lands on the grid of `period` at `phase`, 0 … 1.
///
/// A metronome puts everything on the grid; a rubato line puts a fraction there by coincidence.
fn explained_share(envelope: &[f32], period: f64, phase: f64) -> f64 {
    let total: f64 = envelope.iter().map(|value| f64::from(*value)).sum();
    if total <= 0.0 {
        return 0.0;
    }
    // Half a hop either side of each grid point: the flux of one onset spans a couple of hops, and a
    // grid that hits the onset exactly would otherwise be punished for the smear it created.
    let mut on_grid = 0.0f64;
    let mut position = phase;
    let length = envelope.len() as f64;
    let mut last = f64::NEG_INFINITY;
    while position < length {
        let from = (position - 0.5).max(0.0) as usize;
        let to = ((position + 0.5) as usize + 1).min(envelope.len());
        if from as f64 > last {
            for value in &envelope[from..to] {
                on_grid += f64::from(*value);
            }
            last = to as f64;
        }
        position += period;
    }
    (on_grid / total).clamp(0.0, 1.0)
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
