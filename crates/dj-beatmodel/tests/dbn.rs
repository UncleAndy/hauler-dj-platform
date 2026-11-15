//! Декодер: ровный пульс даёт ровные доли, темп берётся из пульса.

#![allow(clippy::expect_used)]

use dj_beatmodel::{DbnConfig, decode_beats};

fn pulse(bpm: f64, seconds: f64, level: f32) -> Vec<f32> {
    let config = DbnConfig::default();
    let frames = (config.fps * seconds) as usize;
    let period = config.fps * 60.0 / bpm;
    let mut activation = vec![0.02f32; frames];
    let mut position = 0.0f64;
    while (position as usize) < frames {
        activation[position as usize] = level;
        position += period;
    }
    activation
}

fn median_interval(beats: &[f64]) -> f64 {
    let mut intervals: Vec<f64> = beats.windows(2).map(|pair| pair[1] - pair[0]).collect();
    intervals.sort_by(|a, b| a.partial_cmp(b).expect("интервал"));
    intervals[intervals.len() / 2]
}

#[test]
fn a_pulse_at_128_becomes_beats_at_128() {
    let activation = pulse(128.0, 12.0, 0.9);
    let beats = decode_beats(&activation, &DbnConfig::default());
    assert!(beats.len() > 20, "долей мало: {}", beats.len());
    let interval = median_interval(&beats);
    let bpm = 60.0 / interval;
    assert!((bpm - 128.0).abs() < 3.0, "декодер дал {bpm:.1} BPM");
}

#[test]
fn a_fast_pulse_is_not_halved() {
    // 174 BPM — драм-энд-бейс: граница диапазона, где ошибиться в половину легче всего.
    let activation = pulse(174.0, 12.0, 0.9);
    let beats = decode_beats(&activation, &DbnConfig::default());
    let bpm = 60.0 / median_interval(&beats);
    assert!((bpm - 174.0).abs() < 5.0, "декодер дал {bpm:.1} BPM");
}

#[test]
fn a_short_activation_decodes_to_nothing() {
    let activation = vec![0.5f32; 4];
    assert!(decode_beats(&activation, &DbnConfig::default()).is_empty());
}
