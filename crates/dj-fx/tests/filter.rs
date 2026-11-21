//! The filter's law, measured: what the centre is, what the ends are, and that crossing the centre is a sweep
//! rather than a switch.

use dj_fx::{DjFilter, FxUnit};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;

/// A stereo sine at the given frequency and amplitude, `frames` frames long, phase continuous from the first frame.
fn sine(frequency: f32, amplitude: f32, frames: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let phase = 2.0 * std::f32::consts::PI * frequency * frame as f32 / RATE;
        let value = amplitude * phase.sin();
        samples.push(value);
        samples.push(value);
    }
    samples
}

/// Run a filter over a continuous signal, block by block, the way the mixer will.
fn run(filter: &mut DjFilter, input: &[f32]) -> Vec<f32> {
    let frames = input.len() / 2;
    let mut out = vec![0.0; input.len()];
    let mut at = 0;
    while at + BLOCK <= frames {
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        filter.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }
    out
}

/// The left channel's RMS: both channels carry the same tone, and one number reads better than two.
fn rms(samples: &[f32]) -> f64 {
    let count = samples.len() / 2;
    let sum: f64 = samples
        .iter()
        .step_by(2)
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum();
    (sum / count as f64).sqrt()
}

/// Level of the output against the input at one frequency, after the sweep and the sections have settled.
fn level_db(filter: &mut DjFilter, amount: f32, frequency: f32) -> f64 {
    filter.set_amount(amount);
    filter.reset();
    // One continuous tone: a tone restarted every block would put the measurement's own clicks into the number.
    let tone = sine(frequency, 0.5, 24_576);
    let out = run(filter, &tone);
    let tail = out.len() - 8192 * 2;
    let input_rms = rms(&tone[tail..]);
    let output_rms = rms(&out[tail..]);
    20.0 * (output_rms / input_rms).log10()
}

#[test]
fn the_centre_is_flat_and_the_ends_cut() {
    let mut filter = DjFilter::new();
    filter.prepare(RATE, BLOCK);

    let centre = level_db(&mut filter, 0.5, 1000.0);
    assert!(
        centre.abs() < 0.5,
        "the centre is not the signal itself: {centre:.2} dB"
    );

    // 1 kHz against a 150 Hz corner is about −33 dB of a second-order slope; 30 dB is the claim, not the theory.
    let low_pass = level_db(&mut filter, 0.0, 1000.0);
    assert!(
        low_pass < -30.0,
        "the low-pass end does not cut 1 kHz: {low_pass:.2} dB"
    );

    // 1 kHz against an 8 kHz high-pass corner is about −36 dB.
    let high_pass = level_db(&mut filter, 1.0, 1000.0);
    assert!(
        high_pass < -30.0,
        "the high-pass end does not cut 1 kHz: {high_pass:.2} dB"
    );
}

#[test]
fn crossing_the_centre_does_not_click() {
    // A 50 Hz tone at this amplitude steps by about 0.003 per sample, so anything above 0.02 is the filter, not the
    // signal.
    let mut filter = DjFilter::new();
    filter.prepare(RATE, BLOCK);

    let blocks = 100;
    let tone = sine(50.0, 0.5, blocks * BLOCK);
    let mut out = vec![0.0; tone.len()];
    for index in 0..blocks {
        // Centre, then hard left, then back: the two crossings that used to be a switch.
        let amount = if index < 20 {
            0.5
        } else if index < 80 {
            0.0
        } else {
            0.5
        };
        filter.set_amount(amount);
        let (from, to) = (index * BLOCK * 2, (index + 1) * BLOCK * 2);
        filter.process(&tone[from..to], &mut out[from..to], BLOCK);
    }

    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let mut worst = 0.0f32;
    for pair in left.windows(2) {
        worst = worst.max((pair[1] - pair[0]).abs());
    }
    assert!(
        worst <= 0.02,
        "the sweep stepped by {worst:.4} between two samples"
    );
}

#[test]
fn the_sweep_closes_the_highs_one_way() {
    // 8 kHz sits well above the low-pass corner for most of the travel, so its level documents the sweep itself.
    let mut filter = DjFilter::new();
    filter.prepare(RATE, BLOCK);
    let mut previous = level_db(&mut filter, 0.5, 8_000.0);
    for amount in [0.35, 0.25, 0.15, 0.0] {
        let level = level_db(&mut filter, amount, 8_000.0);
        assert!(
            level < previous - 1.0,
            "8 kHz is not being cut further at {amount}: {level:.2} dB after {previous:.2} dB"
        );
        previous = level;
    }
}

#[test]
fn the_steady_state_does_not_depend_on_the_block() {
    // The mixer's block size is the device's business, not the effect's: the same tone must settle to the same level
    // whether it arrives 64 frames at a time or 512.
    let measure = |block: usize| {
        let mut filter = DjFilter::new();
        filter.prepare(RATE, block);
        filter.set_amount(0.2);
        let frames = 16_384;
        let tone = sine(500.0, 0.5, frames);
        let mut out = vec![0.0; tone.len()];
        let mut at = 0;
        while at + block <= frames {
            let (from, to) = (at * 2, (at + block) * 2);
            filter.process(&tone[from..to], &mut out[from..to], block);
            at += block;
        }
        rms(&out[(out.len() / 2)..])
    };
    let small = measure(64);
    let large = measure(512);
    let difference = 20.0 * (large / small).log10();
    assert!(
        difference.abs() < 0.5,
        "the block size changes the level by {difference:.2} dB"
    );
}
