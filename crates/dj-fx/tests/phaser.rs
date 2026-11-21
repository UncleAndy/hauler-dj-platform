//! The phaser's law: the notches move, the level holds, and the knob is audible along its travel.

mod support;

use dj_fx::{FxUnit, Phaser};
use support::{BLOCK, RATE, envelope_spread_db, rms, tone};

fn run(amount: f32, frequency: f32) -> Vec<f32> {
    let mut phaser = Phaser::new();
    phaser.set_amount(amount);
    phaser.prepare(RATE, BLOCK);
    let input = tone(frequency, 0.5, 96_000);
    let frames = input.len() / 2;
    let mut out = vec![0.0; input.len()];
    let mut at = 0;
    while at + BLOCK <= frames {
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        phaser.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }
    out
}

#[test]
fn a_knob_at_zero_is_the_dry_signal() {
    let input = tone(800.0, 0.5, (96_000 / BLOCK) * BLOCK);
    let out = run(0.0, 800.0);
    assert_eq!(out, input, "a phaser at zero changed the signal");
}

#[test]
fn the_notches_move_and_the_level_holds() {
    let still = envelope_spread_db(&run(0.0, 800.0), 2_400);
    let moving = envelope_spread_db(&run(1.0, 800.0), 2_400);
    assert!(still < 1.0, "a closed knob already sweeps: {still:.2} dB");
    assert!(
        moving > 1.5,
        "the sweep is not audible: {moving:.2} dB of spread"
    );

    // Half and half with the dry signal: the level stays where the deck put it.
    let input = tone(800.0, 0.5, 96_000);
    let out = run(1.0, 800.0);
    let difference = 20.0 * (rms(&out) / rms(&input)).log10();
    assert!(
        difference.abs() < 6.0,
        "the level moved by {difference:.2} dB"
    );
}

#[test]
fn every_step_of_the_knob_is_audible() {
    // The same ladder for the phaser (owner's report, 21.11: the left half of the knob changed nothing). Measured:
    // 0,05 → 1,8 · 0,1 → 2,6 · 0,25 → 5,8 · 0,5 → 10,4 · 0,75 → 13,5 · 1,0 → 13,7. The top flattens out — the share
    // of the effect is capped at half, which is what a phaser is — and that is allowed; falling back is not.
    let mut previous = 0.0;
    for (amount, floor) in [
        (0.1, 2.0),
        (0.25, 5.0),
        (0.5, 9.0),
        (0.75, 12.5),
        (1.0, 12.5),
    ] {
        let spread = envelope_spread_db(&run(amount, 800.0), 2_400);
        assert!(
            spread >= floor,
            "at {amount} the effect is only {spread:.2} dB, wanted {floor}"
        );
        assert!(
            spread >= previous - 0.3,
            "the effect fell back at {amount}: {spread:.2} dB after {previous:.2}"
        );
        previous = spread;
    }
}
