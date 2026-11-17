//! How the scratch sounds: the clicks a hand must not make.
//!
//! The material is a sine, so any step the listener hears is a step the deck made: a groove read at a
//! steady speed has a largest sample-to-sample difference set by the signal and the speed, and a click is
//! a jump well past it. Every witness here compares against the same material played forwards at the same
//! speed, so the threshold is "not worse than ordinary resampling" rather than a number picked by hand.

#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use dj_deck::{Deck, DeckSource};
use dj_domain::DeckId;

const RATE: u32 = 48_000;
const HZ: f64 = 220.0;
const AMPLITUDE: f32 = 0.5;

/// A sine at a known frequency: smooth by construction, so a step is the deck's, not the material's.
struct Sine {
    frames: u64,
    position: u64,
}

impl DeckSource for Sine {
    fn sample_rate(&self) -> u32 {
        RATE
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = self.frames.saturating_sub(self.position);
        let take = want.min(usize::try_from(available).unwrap_or(want));
        for index in 0..take {
            let t = (self.position + index as u64) as f64 / f64::from(RATE);
            let value = (AMPLITUDE as f64 * (std::f64::consts::TAU * HZ * t).sin()) as f32;
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }

    fn seeks_are_immediate(&self) -> bool {
        true
    }
}

fn deck() -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(RATE);
    deck.attach(
        Box::new(Sine {
            frames: RATE as u64 * 30,
            position: 0,
        }),
        None,
    );
    deck
}

/// The largest jump between neighbouring samples: what a click sounds like on a scope.
fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max)
}

/// The largest step the same material makes when the hand moves forward at this speed: the bar every
/// other reading here is measured against.
///
/// Measured with the hand rather than the motor on purpose: the motor's own path steps at a chunk boundary
/// while its tempo glides, and that is a defect of its own — taking the bar from it would let a real click
/// through. A groove read forwards at a steady speed is what "not a click" means.
fn hand_step(rate: f64) -> f32 {
    let mut deck = deck();
    deck.seek(48_000);
    deck.play();
    deck.grab(true);
    deck.set_hand_rate(rate);
    let mut out = vec![0.0f32; 256 * 2];
    deck.render(&mut out);
    let mut heard = Vec::new();
    for _ in 0..3 {
        deck.render(&mut out);
        heard.extend(out.iter().step_by(2).copied());
    }
    largest_step(&heard)
}

/// Turning the record round under the hand is not a click: the needle stays in the groove and the material
/// simply runs the other way.
#[test]
fn turning_the_record_round_under_the_hand_does_not_click() {
    let speed = 1.05;
    let bar = hand_step(speed);
    let mut deck = deck();
    deck.seek(48_000);
    deck.play();
    deck.grab(true);
    deck.set_hand_rate(speed);
    let mut out = vec![0.0f32; 256 * 2];
    // Warm up: the edge out of silence belongs to the deck starting, not to the hand.
    deck.render(&mut out);
    let mut heard = Vec::new();
    for _ in 0..3 {
        deck.render(&mut out);
        heard.extend(out.iter().step_by(2).copied());
    }
    // The hand turns: the same material, the other way.
    deck.set_hand_rate(-speed);
    for _ in 0..3 {
        deck.render(&mut out);
        heard.extend(out.iter().step_by(2).copied());
    }
    let step = largest_step(&heard);
    assert!(
        step <= bar * 1.5,
        "turning the record round made a jump of {step:.4}, while a hand going forward makes {bar:.4}"
    );
}

/// Putting a hand on a playing record and taking it off again are not clicks either.
#[test]
fn the_hand_arrives_and_leaves_without_a_click() {
    let speed = 1.05;
    let bar = hand_step(speed);
    let mut deck = deck();
    deck.seek(48_000);
    deck.set_tempo(speed);
    deck.play();
    let mut out = vec![0.0f32; 256 * 2];
    // Warm up: the edge out of silence belongs to the deck starting, not to the hand.
    deck.render(&mut out);
    let mut heard = Vec::new();
    for _ in 0..3 {
        deck.render(&mut out);
        heard.extend(out.iter().step_by(2).copied());
    }
    deck.grab(true);
    deck.set_hand_rate(speed);
    for _ in 0..3 {
        deck.render(&mut out);
        heard.extend(out.iter().step_by(2).copied());
    }
    deck.grab(false);
    for _ in 0..3 {
        deck.render(&mut out);
        heard.extend(out.iter().step_by(2).copied());
    }
    let step = largest_step(&heard);
    // Two steps of the same material, not one: the two readings of the groove are half a frame apart, and
    // half a frame is a step and a bit. A click is tens of them — before the release crossfade this read
    // 0,4759, thirty times the forward step.
    assert!(
        step <= bar * 2.5,
        "the hand made a jump of {step:.4} arriving or leaving, while a hand going forward makes {bar:.4}"
    );
}
