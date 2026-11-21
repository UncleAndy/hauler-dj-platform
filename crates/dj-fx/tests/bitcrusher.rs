//! The crusher's law: the amplitude lands on a grid, and every value is held for whole frames.

mod support;

use dj_fx::{Bitcrusher, FxUnit};
use support::{BLOCK, RATE};

fn ramp(frames: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let value = -0.9 + 1.8 * frame as f32 / frames as f32;
        samples.push(value);
        samples.push(value);
    }
    samples
}

fn run(amount: f32, input: &[f32]) -> Vec<f32> {
    let mut crusher = Bitcrusher::new();
    // The knob first, then the buffers: the amount is smoothed against clicks, so a value set after `prepare`
    // would take 10 ms to arrive and the first samples of the ramp would be crusched with the wrong grid.
    crusher.set_amount(amount);
    crusher.prepare(RATE, BLOCK);
    let frames = input.len() / 2;
    let mut out = vec![0.0; input.len()];
    let mut at = 0;
    while at + BLOCK <= frames {
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        crusher.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }
    out
}

#[test]
fn the_step_is_the_quantum() {
    // Four bits is a step of 2^-3 = 0.125, and a ramp through the whole scale must land on that grid.
    let input = ramp(4_096);
    let out = run(1.0, &input);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    for value in &left[..left.len() - 1] {
        let steps = value / 0.125;
        assert!(
            (steps - steps.round()).abs() < 1e-3,
            "{value} is not on the four-bit grid"
        );
    }

    // A knob at zero keeps sixteen bits: the same ramp lands on a much finer grid, so there are many more distinct
    // levels in it than in the four-bit one.
    let fine = run(0.0, &input);
    let distinct = |samples: &[f32]| {
        let mut values: Vec<i32> = samples
            .iter()
            .map(|v| (v * 1_000_000.0).round() as i32)
            .collect();
        values.sort_unstable();
        values.dedup();
        values.len()
    };
    let coarse_levels = distinct(&left);
    let fine_levels = distinct(&fine.iter().step_by(2).copied().collect::<Vec<f32>>());
    assert!(
        fine_levels > coarse_levels * 4,
        "sixteen bits are not finer than four: {fine_levels} levels against {coarse_levels}"
    );
}

#[test]
fn the_hold_repeats() {
    // At the far end the hold is 32 frames, so the output can only change once every 32 frames.
    let input = ramp(4_096);
    let out = run(1.0, &input);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let mut changes = 0;
    for index in 1..left.len() {
        if (left[index] - left[index - 1]).abs() > 1e-6 {
            changes += 1;
        }
    }
    let expected = left.len() / 32;
    assert!(
        changes <= expected + 40,
        "{changes} changes for a 32-frame hold over {} frames",
        left.len()
    );
}
