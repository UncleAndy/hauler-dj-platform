//! The flanger's law: the knob is audible along its whole travel, not only at the end.

mod support;

use dj_fx::{Flanger, FxUnit};
use support::{BLOCK, RATE, envelope_spread_db, tone};

fn run(amount: f32, frequency: f32) -> Vec<f32> {
    let mut flanger = Flanger::new();
    // The knob first, then the buffers: the amount is smoothed against clicks, and a value set after `prepare` would
    // spend the first milliseconds arriving.
    flanger.set_amount(amount);
    flanger.prepare(RATE, BLOCK);
    let input = tone(frequency, 0.5, 96_000);
    let frames = input.len() / 2;
    let mut out = vec![0.0; input.len()];
    let mut at = 0;
    while at + BLOCK <= frames {
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        flanger.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }
    out
}

#[test]
fn a_knob_at_zero_is_the_dry_signal() {
    let input = tone(2_000.0, 0.5, (96_000 / BLOCK) * BLOCK);
    let out = run(0.0, 2_000.0);
    assert_eq!(out, input, "a flanger at zero changed the signal");
}

#[test]
fn the_comb_moves() {
    // A static comb leaves a tone's level alone; a moving one makes it breathe.
    let still = envelope_spread_db(&run(0.0, 2_000.0), 2_400);
    let moving = envelope_spread_db(&run(1.0, 2_000.0), 2_400);
    assert!(
        still < 0.5,
        "a knob at zero still moves the comb: {still:.2} dB of spread"
    );
    assert!(
        moving > 3.0 && moving > still + 3.0,
        "the sweep is not audible: {moving:.2} dB of spread against {still:.2}"
    );
}

#[test]
fn every_step_of_the_knob_is_audible() {
    // The owner's report of 21.11: turning the knob barely changed the sound. Each quarter of the travel has to be
    // measurably stronger than the one before it, not only the last one.
    let mut previous = envelope_spread_db(&run(0.0, 2_000.0), 2_400);
    for amount in [0.25, 0.5, 0.75, 1.0] {
        let spread = envelope_spread_db(&run(amount, 2_000.0), 2_400);
        assert!(
            spread > previous + 0.5,
            "at {amount} the effect is {spread:.2} dB of spread, barely more than {previous:.2} at the step before"
        );
        previous = spread;
    }
    assert!(
        previous > 4.0,
        "the far end of the travel is only {previous:.2} dB"
    );
}
