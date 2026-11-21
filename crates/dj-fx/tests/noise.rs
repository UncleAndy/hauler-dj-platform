//! The noise's law: the band follows the knob, and the same seed gives the same sweep.

mod support;

use dj_fx::{FxUnit, Noise};
use support::{BLOCK, RATE, zero_crossings};

fn run(amount: f32) -> Vec<f32> {
    let mut noise = Noise::new();
    noise.prepare(RATE, BLOCK);
    noise.set_amount(amount);
    let input = vec![0.0; 24_576 * 2];
    let frames = input.len() / 2;
    let mut out = vec![0.0; input.len()];
    let mut at = 0;
    while at + BLOCK <= frames {
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        noise.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }
    out
}

#[test]
fn the_band_follows_the_knob() {
    // The same generator, two centres: the higher one crosses zero far more often.
    let low = zero_crossings(&run(0.2));
    let high = zero_crossings(&run(1.0));
    assert!(low > 0, "the sweep is silent at the low end");
    assert!(
        high > low * 3,
        "the centre did not follow the knob: {low} crossings low against {high} high"
    );
}

#[test]
fn the_same_seed_gives_the_same_noise() {
    let first = run(0.7);
    let second = run(0.7);
    assert_eq!(first, second, "the generator is not deterministic");
}
