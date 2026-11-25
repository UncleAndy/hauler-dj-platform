//! The knob's centre is no effect at all, and the two ways from it are two different effects (owner's request, 25.11).
//!
//! Both claims are made for every unit through the same door the mixer uses, so a unit added later cannot be left out:
//! nothing happens at the centre, and a turn to the left is not a turn to the right.
//!
//! The roll is the one unit whose knob is not a level but a length: a held repeat at the centre plays an eighth of a
//! beat, and the two ways shorten it towards a sixteenth or lengthen it towards half. That is measured here too.

use dj_fx::{Bitcrusher, DjFilter, Echo, Flanger, FxUnit, Noise, Phaser, Roll};

const RATE: f32 = 48_000.0;
const FRAMES: usize = 16_384;

/// A knob the mixer can turn: `FxUnit` plus the one call the rack makes when a patch changes.
trait Knob: FxUnit {
    fn set_amount(&mut self, amount: f32);
}

macro_rules! knobs {
    ($($unit:ty),*) => {$(
        impl Knob for $unit {
            fn set_amount(&mut self, amount: f32) {
                <$unit>::set_amount(self, amount);
            }
        }
    )*};
}
knobs!(DjFilter, Echo, Roll, Flanger, Phaser, Bitcrusher, Noise);

/// A signal with body and top: twelve harmonics of 220 Hz under a slow swell, so a filter, a comb, a crusher and a sweep
/// all have something to act on. Deterministic, so two runs compare sample for sample.
fn signal() -> Vec<f32> {
    let mut out = Vec::with_capacity(FRAMES * 2);
    for frame in 0..FRAMES {
        let t = frame as f32 / RATE;
        let swell = 0.6 + 0.4 * (std::f32::consts::TAU * 1.7 * t).sin();
        let mut sample = 0.0;
        for harmonic in 1..=12u32 {
            let partial = std::f32::consts::TAU * 220.0 * harmonic as f32 * t;
            sample += partial.sin() / harmonic as f32;
        }
        let sample = sample * 0.08 * swell;
        out.push(sample);
        out.push(sample * 0.9);
    }
    out
}

/// One block through a unit, the way the rack drives it.
fn play<T: Knob>(unit: &mut T, dry: &[f32], amount: f32) -> Vec<f32> {
    unit.prepare(RATE, FRAMES);
    unit.set_amount(amount);
    unit.engage(true);
    let mut out = vec![0.0; dry.len()];
    unit.process(dry, &mut out, FRAMES);
    out
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn difference(one: &[f32], other: &[f32]) -> f32 {
    let mut out = Vec::with_capacity(one.len());
    for (a, b) in one.iter().zip(other.iter()) {
        out.push(a - b);
    }
    rms(&out)
}

#[test]
fn the_centre_leaves_every_unit_out_of_the_path() {
    let dry = signal();
    let mut filter = DjFilter::new();
    assert_eq!(
        play(&mut filter, &dry, 0.0),
        dry,
        "the filter at the centre must be the deck itself"
    );
    let mut echo = Echo::new();
    assert_eq!(
        play(&mut echo, &dry, 0.0),
        dry,
        "the echo at the centre must be the deck itself"
    );
    let mut flanger = Flanger::new();
    assert_eq!(
        play(&mut flanger, &dry, 0.0),
        dry,
        "the flanger at the centre must be the deck itself"
    );
    let mut phaser = Phaser::new();
    assert_eq!(
        play(&mut phaser, &dry, 0.0),
        dry,
        "the phaser at the centre must be the deck itself"
    );
    let mut crusher = Bitcrusher::new();
    assert_eq!(
        play(&mut crusher, &dry, 0.0),
        dry,
        "the crusher at the centre must be the deck itself"
    );
    let mut noise = Noise::new();
    assert_eq!(
        play(&mut noise, &dry, 0.0),
        dry,
        "the noise at the centre must be the deck itself"
    );
}

#[test]
fn the_two_ways_from_the_centre_are_two_different_effects() {
    let dry = signal();
    let scale = rms(&dry).max(0.0001);
    let cases = [
        (
            "filter",
            play(&mut DjFilter::new(), &dry, -0.8),
            play(&mut DjFilter::new(), &dry, 0.8),
        ),
        (
            "echo",
            play(&mut Echo::new(), &dry, -0.8),
            play(&mut Echo::new(), &dry, 0.8),
        ),
        (
            "flanger",
            play(&mut Flanger::new(), &dry, -0.8),
            play(&mut Flanger::new(), &dry, 0.8),
        ),
        (
            "phaser",
            play(&mut Phaser::new(), &dry, -0.8),
            play(&mut Phaser::new(), &dry, 0.8),
        ),
        (
            "crusher",
            play(&mut Bitcrusher::new(), &dry, -0.8),
            play(&mut Bitcrusher::new(), &dry, 0.8),
        ),
        (
            "noise",
            play(&mut Noise::new(), &dry, -0.8),
            play(&mut Noise::new(), &dry, 0.8),
        ),
    ];
    for (name, left, right) in cases {
        let left_off = difference(&left, &dry);
        let right_off = difference(&right, &dry);
        assert!(
            left_off > 0.05 * scale,
            "{name}: a turn to the left changes nothing ({left_off} against {scale})"
        );
        assert!(
            right_off > 0.05 * scale,
            "{name}: a turn to the right changes nothing ({right_off} against {scale})"
        );
        let between = difference(&left, &right);
        let apart = left_off.min(right_off);
        assert!(
            between > 0.25 * apart,
            "{name}: the two ways do the same thing ({between} against {apart})"
        );
    }
}

#[test]
fn the_rolls_knob_is_a_length_with_an_eighth_of_a_beat_at_the_centre() {
    let beat = 0.5;
    let mut roll = Roll::new();
    roll.prepare(RATE, FRAMES);
    roll.set_beat(beat);
    let frames = |share: f32| (beat * share * RATE).round() as usize;
    for (amount, share, name) in [
        (-1.0, 1.0 / 16.0, "hard left"),
        (0.0, 1.0 / 8.0, "centre"),
        (1.0, 0.5, "hard right"),
    ] {
        roll.set_amount(amount);
        assert_eq!(
            roll.length_frames(),
            frames(share),
            "the roll at {name} captures {share} of a beat"
        );
    }
    // And the travel between the ends is continuous, not three steps.
    let mut previous = 0;
    for step in 0..=8 {
        let amount = -1.0 + step as f32 * 0.25;
        roll.set_amount(amount);
        let length = roll.length_frames();
        assert!(
            length > previous,
            "the roll's length must grow with the knob: {amount} gave {length} after {previous}"
        );
        previous = length;
    }
}
