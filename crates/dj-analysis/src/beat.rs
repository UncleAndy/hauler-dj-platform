//! Beat placement by dynamic programming (Ellis, JNMR 2007).
//!
//! The question this answers is not "what is the tempo" but "where are the beats". A grid drawn from a
//! period and one phase is a straight line through the whole track, and a tenth of a percent of tempo
//! error walks it off the music by the end; a syncopation in the low band can capture its phase outright.
//! The beat tracker takes the same onset envelope and finds the sequence of instants that is best by two
//! measures at once: strong onsets, and a spacing close to the period. The balance is a penalty
//! `−(ln(Δt/τ))²` on the log-ratio of the actual gap to the ideal one, searched over `τ/2 … 2τ`, and the
//! whole thing is one pass plus a backtrace, so it costs about `envelope × period` operations.
//!
//! Two differences from the paper, both deliberate:
//!
//! - the envelope is scaled to a mean of one inside [`track`], so `weight` means the same thing on a quiet
//!   track and a loud one (the paper's onset function is normalised by the caller);
//! - when no predecessor scores positively the sequence *starts again* (the trick from Müller's FMP
//!   notebook). Without it the tracker drags a grid through a breakdown, because a negative cumulative
//!   score still beats no score at all.

/// The beats found, and how well the sequence scores.
#[derive(Debug, Clone, PartialEq)]
pub struct Beats {
    /// Beat positions in envelope frames, in order.
    pub positions: Vec<f64>,
    /// The score of the chosen sequence: onset strength minus the penalties it paid.
    pub score: f64,
}

/// Find the beats: the strongest onsets that keep the period.
///
/// `period` is in envelope frames, `weight` balances the transition penalty against the onset strength
/// (a larger weight keeps the grid straighter). Returns `None` when there is nothing to track.
pub fn track(envelope: &[f32], period: f64, weight: f64) -> Option<Beats> {
    if envelope.len() < 2 || !period.is_finite() || period < 2.0 {
        return None;
    }
    // The envelope has to be *signed* and *locally* standardised, both for the trade-off to exist.
    //
    // Signed: an onset envelope is non-negative — a flux with the running mean taken out and the negative
    // half thrown away. Left like that, a beat between two onsets costs nothing to skip and saves a
    // penalty, so the tracker spaces beats at twice the period and lands only on the strongest hits (90
    // beats 207 frames apart on a 103-frame period, measured). Removing the mean puts the between-beats
    // part below zero, and then a skipped beat costs evidence, as in Ellis's formulation.
    //
    // Local: a global mean makes one loud isolated onset worth tens of ordinary ones — a stray hit half a
    // beat off the grid then pays for breaking it. Ellis normalises the onset strength over a moving
    // window of about a second, and that is what is done here, over two periods.
    let count = envelope.len();
    let mut sum = vec![0.0f64; count + 1];
    let mut square = vec![0.0f64; count + 1];
    for (index, value) in envelope.iter().enumerate() {
        let value = f64::from(*value);
        sum[index + 1] = sum[index] + value;
        square[index + 1] = square[index] + value * value;
    }
    let reach = (period * 2.0).round().max(2.0) as usize;
    let mut onset = vec![0.0f64; count];
    let mut spread = 0.0f64;
    for (index, value) in envelope.iter().enumerate() {
        let from = index.saturating_sub(reach);
        let to = (index + reach + 1).min(count);
        let width = (to - from) as f64;
        let mean = (sum[to] - sum[from]) / width;
        let variance = ((square[to] - square[from]) / width - mean * mean).max(0.0);
        let deviation = variance.sqrt();
        onset[index] = if deviation > 1e-9 {
            (f64::from(*value) - mean) / deviation
        } else {
            0.0
        };
        spread = spread.max(deviation);
    }
    if spread <= 1e-9 {
        return None;
    }

    let from = (period * 0.5).max(1.0);
    let to = period * 2.0;
    let mut cumulative = vec![0.0f64; onset.len()];
    let mut backlink = vec![usize::MAX; onset.len()];
    for index in 0..onset.len() {
        let mut best = (0.0f64, usize::MAX);
        let window_start = (index as f64 - to).max(0.0) as usize;
        let window_end = index as f64 - from;
        if window_end >= 0.0 {
            let window_end = (window_end as usize).min(index.saturating_sub(1));
            for (offset, earlier) in cumulative[window_start..=window_end].iter().enumerate() {
                let previous = window_start + offset;
                let gap = (index - previous) as f64;
                let penalty = -(gap / period).ln().powi(2) * weight;
                let score = earlier + penalty;
                if score > best.0 {
                    best = (score, previous);
                }
            }
        }
        cumulative[index] = onset[index] + best.0;
        backlink[index] = best.1;
    }

    let mut at =
        (0..onset.len()).max_by(|left, right| cumulative[*left].total_cmp(&cumulative[*right]))?;
    let score = cumulative[at];
    let mut positions = vec![at as f64];
    while backlink[at] != usize::MAX {
        at = backlink[at];
        positions.push(at as f64);
    }
    positions.reverse();
    Some(Beats { positions, score })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// A click every `period` frames: the envelope of a metronome.
    fn clicks(count: usize, period: usize, length: usize) -> Vec<f32> {
        let mut envelope = vec![0.0f32; length];
        for index in 0..count {
            let at = 20 + index * period;
            if at < length {
                envelope[at] = 1.0;
            }
        }
        envelope
    }

    #[test]
    fn a_click_train_is_tracked_beat_by_beat() {
        let envelope = clicks(40, 20, 820);
        let beats = track(&envelope, 20.0, 100.0).expect("beats");
        assert!(
            beats.positions.len() >= 38,
            "{} beats",
            beats.positions.len()
        );
        for position in &beats.positions {
            let offset = (position - 20.0).rem_euclid(20.0);
            assert!(
                offset.min(20.0 - offset) <= 1.0,
                "a beat at {position} is off the clicks"
            );
        }
    }

    #[test]
    fn a_break_does_not_drag_the_grid() {
        // Clicks, then four seconds of nothing, then clicks again. The grid has to come back where the
        // clicks are, not where a line drawn through the silence would put it.
        let mut envelope = clicks(40, 20, 820);
        envelope[400..600].fill(0.0);
        let beats = track(&envelope, 20.0, 100.0).expect("beats");
        let after: Vec<f64> = beats
            .positions
            .iter()
            .copied()
            .filter(|position| *position > 600.0)
            .collect();
        assert!(!after.is_empty(), "nothing after the break");
        for position in after {
            let offset = (position - 20.0).rem_euclid(20.0);
            assert!(
                offset.min(20.0 - offset) <= 1.0,
                "a beat at {position} is off the clicks"
            );
        }
    }

    #[test]
    fn a_jump_of_a_beat_and_a_half_is_punished() {
        // One stray onset a beat and a half in: the sequence keeps the period and takes the click.
        let mut envelope = clicks(40, 20, 820);
        envelope[20 + 17 * 20 + 10] = 6.0;
        let beats = track(&envelope, 20.0, 100.0).expect("beats");
        for position in &beats.positions {
            let offset = (position - 20.0).rem_euclid(20.0);
            assert!(
                offset.min(20.0 - offset) <= 1.0,
                "the stray onset at {} pulled a beat to {position}",
                20 + 17 * 20 + 10
            );
        }
    }

    #[test]
    fn silence_has_no_beats() {
        assert!(track(&vec![0.0f32; 500], 20.0, 100.0).is_none());
        assert!(track(&[], 20.0, 100.0).is_none());
        assert!(track(&clicks(4, 20, 100), 0.0, 100.0).is_none());
    }
}
