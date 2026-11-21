//! The echo's law: the tap lands on half a beat, the tail decays, and retiming is a crossfade.

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

use dj_fx::{Echo, FxUnit};
use support::{BLOCK, RATE, impulse, tone, worst_step};

fn run_in_blocks<E: FxUnit>(unit: &mut E, input: &[f32]) -> Vec<f32> {
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

/// Local peaks of the left channel above a floor, as (index, value).
fn peaks(samples: &[f32], floor: f32) -> Vec<(usize, f32)> {
    let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
    let mut found = Vec::new();
    for index in 1..left.len().saturating_sub(1) {
        let value = left[index];
        if value.abs() > floor
            && value.abs() >= left[index - 1].abs()
            && value.abs() >= left[index + 1].abs()
        {
            found.push((index, value.abs()));
        }
    }
    found
}

#[test]
fn the_taps_land_on_half_beats() {
    // 120 BPM: half a beat is 0.25 s, which is 12 000 frames at 48 kHz.
    let mut echo = Echo::new();
    echo.prepare(RATE, BLOCK);
    echo.set_beat(0.5);
    echo.set_amount(0.8);

    let out = run_in_blocks(&mut echo, &impulse(1.0, 48_000));
    let found = peaks(&out, 0.02);

    let first = found.first().copied().unwrap_or((0, 0.0));
    assert!(
        (first.0 as i64 - 12_000).abs() <= 3,
        "the first repeat is at frame {} instead of 12000: {:?}",
        first.0,
        found.iter().take(4).collect::<Vec<_>>()
    );
    assert!(
        (first.1 - 0.8).abs() < 0.05,
        "the first repeat is {:.3} instead of the knob's 0.8",
        first.1
    );

    let second = found.get(1).copied().unwrap_or((0, 0.0));
    assert!(
        (second.0 as i64 - 24_000).abs() <= 4,
        "the second repeat is at frame {} instead of 24000: {:?}",
        second.0,
        found.iter().take(4).collect::<Vec<_>>()
    );
    assert!(
        second.1 < first.1 && second.1 > 0.0,
        "the tail does not decay: {:.3} after {:.3}",
        second.1,
        first.1
    );
}

#[test]
fn the_beat_change_retimes_the_tail() {
    let mut echo = Echo::new();
    echo.prepare(RATE, BLOCK);
    echo.set_beat(0.5);
    echo.set_amount(0.5);

    // Block boundaries on the split and on the end: a truncated tail is the test's own click.
    let blocks = 100;
    let split = 56;
    let mut input = tone(200.0, 0.4, blocks * BLOCK);
    let first = run_in_blocks(&mut echo, &input[..split * BLOCK * 2]);
    // The deck's tempo moved: the tap must follow without a step in the sound.
    echo.set_beat(0.4);
    let rest = run_in_blocks(&mut echo, &input[split * BLOCK * 2..]);

    let mut all = first;
    all.extend_from_slice(&rest);
    let tail = &all[all.len() / 2..];
    report_steps(&all);
    let step = worst_step(tail);
    assert!(
        step <= 0.06,
        "retiming stepped by {step:.4} between two samples"
    );
    input.clear();
}

#[test]
fn a_quiet_knob_leaves_the_signal_alone() {
    let mut echo = Echo::new();
    echo.prepare(RATE, BLOCK);
    echo.set_beat(0.5);
    echo.set_amount(0.0);
    let input = tone(440.0, 0.3, 8_192);
    let out = run_in_blocks(&mut echo, &input);
    assert_eq!(out, input, "a knob at zero is not the dry signal");
}
