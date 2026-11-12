//! What a long beat-less passage does to the tempo, and what the fallback is allowed to publish.
//!
//! The envelopes are built by hand: `detect_in_envelope` takes an onset envelope and a sample rate, so a
//! test can say exactly how loud the beat is against the wash under it and how long the beat-less part is.

#![allow(clippy::expect_used)]

use dj_analysis::tempo;

const RATE: f64 = 44_100.0;

fn per_second() -> f64 {
    RATE / tempo::HOP as f64
}

fn frames_for(seconds: f64) -> usize {
    (per_second() * seconds) as usize
}

/// Onsets everywhere, none of them periodic: a pad, a wash of noise, a rubato line.
fn wash(seconds: f64, level: f32) -> Vec<f32> {
    let mut envelope = vec![0.0f32; frames_for(seconds)];
    let mut noise: u64 = 0x2545_f491_4f6c_dd1d;
    for value in &mut envelope {
        noise ^= noise << 13;
        noise ^= noise >> 7;
        noise ^= noise << 17;
        *value = level * (noise % 1000) as f32 / 1000.0;
    }
    envelope
}

/// The same wash with a hit on every beat: the beat carries the music by as much as `hit` stands above
/// the floor.
fn beat_over_wash(seconds: f64, bpm: f64, floor: f32, hit: f32) -> Vec<f32> {
    let mut envelope = wash(seconds, floor);
    let period = per_second() * 60.0 / bpm;
    let mut at = period * 0.5;
    while (at as usize) < envelope.len() {
        envelope[at as usize] = hit;
        at += period;
    }
    envelope
}

fn join(parts: &[Vec<f32>]) -> Vec<f32> {
    let mut envelope = Vec::new();
    for part in parts {
        envelope.extend_from_slice(part);
    }
    envelope
}

#[test]
fn a_long_breakdown_does_not_hide_the_beat() {
    // Forty seconds of beat at 120, then eighty seconds of wash as loud as the beat. The whole file
    // averages 2,3 against the 1,2 floor — the walk keeps the answer, and the track is mixable from the
    // part that has a beat.
    let envelope = join(&[beat_over_wash(40.0, 120.0, 1.0, 5.0), wash(80.0, 1.0)]);
    let tempo = tempo::detect_in_envelope(&envelope, RATE).expect("доля слышна в первой трети");
    assert!(
        (tempo.bpm() / 120.0 - 1.0).abs() < 0.02,
        "ожидалось 120 BPM, получено {}",
        tempo.bpm()
    );
    assert!(tempo.is_single());
}

#[test]
fn a_track_with_no_beat_anywhere_stays_without_a_tempo() {
    let envelope = wash(180.0, 1.0);
    assert!(
        tempo::detect_in_envelope(&envelope, RATE).is_none(),
        "у трека без доли темпа быть не должно"
    );
}

#[test]
fn the_walk_keeps_the_answer_it_gave() {
    // A clean beat: the walk answers it, and the fallback never runs. The number is the one the walk has
    // always given, which is the point of putting the fallback after it.
    let envelope = beat_over_wash(120.0, 128.0, 0.05, 1.0);
    let tempo = tempo::detect_in_envelope(&envelope, RATE).expect("чистая доля");
    assert!(
        (tempo.bpm() / 128.0 - 1.0).abs() < 0.02,
        "ожидалось 128 BPM, получено {}",
        tempo.bpm()
    );
}

#[test]
fn the_fallback_does_not_publish_from_a_handful_of_bars() {
    // Eight seconds of beat in two minutes of wash: the windows that carry it are a sixth of the track,
    // under the share the fallback asks for. The walk refuses, and the fallback must not rescue it.
    let envelope = join(&[beat_over_wash(8.0, 120.0, 1.0, 5.0), wash(112.0, 1.0)]);
    assert!(
        tempo::detect_in_envelope(&envelope, RATE).is_none(),
        "восьми секунд доли мало, чтобы публиковать темп"
    );
}
