//! Pieces the effect tests share: a tone, a block runner and the two measurements they argue with.
//!
//! Deliberately small — a test that needs more machinery usually needs a narrower claim instead.

#![allow(dead_code)]

pub const RATE: f32 = 48_000.0;
pub const BLOCK: usize = 256;

/// A stereo tone with continuous phase from the first frame.
pub fn tone(frequency: f32, amplitude: f32, frames: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let phase = 2.0 * std::f32::consts::PI * frequency * frame as f32 / RATE;
        let value = amplitude * phase.sin();
        samples.push(value);
        samples.push(value);
    }
    samples
}

/// A stereo impulse of `frames` frames, the first frame at `amplitude`.
pub fn impulse(amplitude: f32, frames: usize) -> Vec<f32> {
    let mut samples = vec![0.0; frames * 2];
    samples[0] = amplitude;
    samples[1] = amplitude;
    samples
}

/// The left channel's RMS.
pub fn rms(samples: &[f32]) -> f64 {
    let count = (samples.len() / 2).max(1);
    let sum: f64 = samples
        .iter()
        .step_by(2)
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum();
    (sum / count as f64).sqrt()
}

/// The largest step between two neighbouring samples of the left channel.
pub fn worst_step(samples: &[f32]) -> f32 {
    let mut worst = 0.0f32;
    let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
    for pair in left.windows(2) {
        worst = worst.max((pair[1] - pair[0]).abs());
    }
    worst
}

/// The spread of the envelope, in decibels: how much a signal's level moves over time. A steady tone gives about
/// zero; a modulated one does not.
pub fn envelope_spread_db(samples: &[f32], window_frames: usize) -> f64 {
    let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
    let mut levels = Vec::new();
    let mut at = 0;
    while at + window_frames <= left.len() {
        let window = &left[at..at + window_frames];
        let rms = (window
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            / window_frames as f64)
            .sqrt();
        if rms > 1e-6 {
            levels.push(20.0 * rms.log10());
        }
        at += window_frames;
    }
    if levels.len() < 2 {
        return 0.0;
    }
    let high = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let low = levels.iter().copied().fold(f64::INFINITY, f64::min);
    high - low
}

/// How often the left channel crosses zero: a cheap way to say "this noise sits higher than that one".
pub fn zero_crossings(samples: &[f32]) -> usize {
    let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
    left.windows(2)
        .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
        .count()
}
