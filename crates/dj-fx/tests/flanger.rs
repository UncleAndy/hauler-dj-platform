//! The flanger's law: the comb moves, and a knob at zero stops moving it.

mod support;

use dj_fx::{Flanger, FxUnit};
use support::{BLOCK, RATE, envelope_spread_db, tone};

fn run(amount: f32, frequency: f32) -> Vec<f32> {
    let mut flanger = Flanger::new();
    flanger.prepare(RATE, BLOCK);
    flanger.set_amount(amount);
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
