//! The stretcher runs inside the audio callback, so its cost belongs to the RT budget.

use dj_stretch_sys::Stretch;
use std::time::Instant;

#[test]
fn a_block_of_two_hundred_fifty_six_frames_stays_inside_the_budget() {
    // DJP-NFR-003: at 48 kHz and 256 frames the period is 5,33 ms; the RT contract allows 60 % of it
    // for the whole graph, so one deck's stretch has to be far below that.
    let mut stretch = Stretch::with_config(48_000.0, 0.06, 0.015);
    let input: Vec<f32> = (0..48_000 * 6)
        .map(|i| {
            let phase = std::f64::consts::TAU * 220.0 * i as f64 / 48_000.0;
            (phase.sin() * 0.4) as f32
        })
        .collect();
    // Interleaved stereo: duplicate each sample.
    let mut stereo = Vec::with_capacity(input.len() * 2);
    for value in &input {
        stereo.push(*value);
        stereo.push(*value);
    }

    let mut out = vec![0.0f32; 256 * 2];
    let mut worst = 0.0f64;
    let mut consumed = 0usize;
    for _ in 0..400 {
        let in_frames = (256.0f64 * 1.08).round() as usize;
        let from = consumed * 2;
        let start = Instant::now();
        stretch.process(&stereo[from..from + in_frames * 2], &mut out);
        let elapsed = start.elapsed().as_secs_f64() * 1e6;
        worst = worst.max(elapsed);
        consumed += in_frames;
    }
    let period_us = 256.0 / 48_000.0 * 1e6;
    println!("worst block {worst:.1} us of {period_us:.0} us period");
    assert!(
        worst < period_us * 0.3,
        "the stretcher took {worst:.0} us of a {period_us:.0} us block"
    );
}
