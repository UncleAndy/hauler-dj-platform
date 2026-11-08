//! The resampler runs inside the audio callback, so its cost belongs to the RT budget.

use dj_signalsmith_sys::{Interpolation, Resampler};
use std::time::Instant;

#[test]
fn a_block_of_two_hundred_fifty_six_frames_stays_inside_the_budget() {
    // DJP-NFR-003: at 48 kHz and 256 frames the period is 5,33 ms; the RT contract allows 60 % of it
    // for the whole graph. A deck resamples twice on the key-locked path (rate, then tempo), so one
    // pass has to be a small part of that.
    let (taps, lookback) = Interpolation::KaiserSinc16.geometry();
    let frames = 2048;
    let mut input = vec![0.0f32; (frames + 512) * 2];
    for frame in 0..frames {
        let value = (std::f64::consts::TAU * 440.0 * frame as f64 / 48_000.0).sin() as f32 * 0.4;
        input[frame * 2] = value;
        input[frame * 2 + 1] = value;
    }
    let mut resampler = Resampler::new(Interpolation::KaiserSinc16);
    let mut out = vec![0.0f32; 256 * 2];

    let step = 44_100.0 / 48_000.0;
    let mut worst = 0.0f64;
    let mut total = 0.0f64;
    let mut base = lookback;
    for _ in 0..400 {
        let start = Instant::now();
        let reached = resampler.process(&input, &mut out, base, 0.0, step);
        let cost = start.elapsed().as_secs_f64() * 1e6;
        worst = worst.max(cost);
        total += cost;
        assert!(reached > 0.0);
        // Walk through the buffer and wrap: the real deck keeps a window of that size.
        base += 256;
        if base + taps + 256 > frames {
            base = lookback;
        }
    }
    let average = total / 400.0;
    println!(
        "dj-signalsmith-sys: resampler {taps} taps, average {average:.1} us, worst {worst:.1} us"
    );
    assert!(
        worst < 2000.0,
        "a 256-frame resample took {worst:.1} us, which is 37 % of the 5333 us period"
    );
}
