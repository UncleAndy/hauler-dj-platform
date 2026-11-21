//! The roll's law: the seam is silent, the length follows the knob, releasing returns to the live signal.

mod support;

/// Where the worst steps are, printed when a test fails: a click has an address, and the address says why.
fn report_steps(samples: &[f32]) {
    let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
    let mut worst: Vec<(usize, f32)> = Vec::new();
    for index in 1..left.len() {
        let step = (left[index] - left[index - 1]).abs();
        if step > 0.05 {
            worst.push((index, step));
        }
    }
    worst.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (index, step) in worst.iter().take(5) {
        println!("step {step:.4} at {index} (block {} of 256)", index / 256);
    }
}

use dj_fx::{FxUnit, Roll};
use support::{BLOCK, RATE, tone, worst_step};

fn run(unit: &mut Roll, input: &[f32]) -> Vec<f32> {
    let frames = input.len() / 2;
    let mut out = vec![0.0; input.len()];
    let mut at = 0;
    while at + BLOCK <= frames {
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        unit.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }
    out
}

#[test]
fn the_seam_does_not_click() {
    // The deck has been playing before the hand reaches the switch: that is what a roll captures, so the test warms
    // the buffer up first. Pressing on a silent deck would capture silence and prove nothing.
    //
    // 0.47 s per beat, so a quarter of it is 5640 frames. The tone is 20 Hz on purpose: its period is 2400 frames, so
    // 5640 frames is two periods and a third — a join that jumps almost a whole period, which is what a seam without
    // a crossfade sounds like. A 200 Hz tone lands near a zero crossing there and would prove nothing.
    let mut roll = Roll::new();
    roll.prepare(RATE, BLOCK);
    roll.set_beat(0.47);
    roll.set_amount(0.5);

    let warm = (24_000 / BLOCK) * BLOCK;
    let input = tone(20.0, 0.5, ((warm + 48_000) / BLOCK) * BLOCK);
    let _ = run(&mut roll, &input[..warm * 2]);
    roll.engage(true);
    let out = run(&mut roll, &input[warm * 2..]);

    report_steps(&out);

    // The tone steps by 0.0013 per sample at this rate and amplitude; the loop's join must not beat that by much.
    let step = worst_step(&out[4_000..]);
    assert!(
        step <= 0.02,
        "the seam stepped by {step:.4} between two samples"
    );
}

#[test]
fn the_loop_length_follows_the_knob() {
    let mut roll = Roll::new();
    roll.prepare(RATE, BLOCK);
    roll.set_beat(0.5);
    let cases = [(0.0, 1_500), (0.5, 6_000), (1.0, 12_000)];
    for (amount, expected) in cases {
        roll.set_amount(amount);
        assert_eq!(
            roll.length_frames(),
            expected,
            "at {amount} the loop is {} frames, not {expected}",
            roll.length_frames()
        );
    }
}

#[test]
fn releasing_returns_to_live() {
    let mut roll = Roll::new();
    roll.prepare(RATE, BLOCK);
    roll.set_beat(0.5);
    roll.set_amount(0.5);
    roll.engage(true);
    let input = tone(300.0, 0.4, 24_576);
    let _ = run(&mut roll, &input);
    roll.engage(false);
    let out = run(&mut roll, &input);
    assert_eq!(out, input, "the released roll is not the live signal");
}
