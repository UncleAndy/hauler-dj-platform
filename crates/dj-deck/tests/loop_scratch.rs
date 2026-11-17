//! The loop holds the hand: a scratch inside a captured loop stays inside it.
//!
//! A DJ scratching a loop expects the record to run out at the loop's edge and come back in from the other
//! side — that is what makes a loop a loop under the hand. The witnesses here ask for exactly that, and for
//! the two things that follow from it: the hand must not be counted as the motor going round (the wrap
//! counter is the motor's), and the sound must not fall into silence at the seam.

#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use dj_deck::{Deck, DeckSource};
use dj_domain::{DeckId, FrameRange};

const RATE: u32 = 48_000;

/// A sine, so a hole in the sound at the seam is a hole and not a phase.
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
            let value = (0.5 * (std::f64::consts::TAU * 220.0 * t).sin()) as f32;
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

fn deck_with_loop() -> (Deck, u64, u64) {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(RATE);
    deck.attach(
        Box::new(Sine {
            frames: RATE as u64 * 30,
            position: 0,
        }),
        None,
    );
    // A loop of 400 ms, well inside the file.
    let start = 96_000u64;
    let end = start + 19_200;
    let region = FrameRange {
        start: dj_domain::SourceFrame(start),
        end: dj_domain::SourceFrame(end),
    };
    assert!(
        deck.set_loop(Some(region)),
        "the deck refused a loop inside the file"
    );
    deck.set_loop_mode(true);
    deck.seek(start + 9_600);
    deck.play();
    (deck, start, end)
}

/// A hand that drags the record past the loop's end comes back in at its start, and never leaves.
#[test]
fn the_hand_wraps_at_the_loop_and_does_not_leave_it() {
    let (mut deck, start, end) = deck_with_loop();
    let mut out = vec![0.0f32; 256 * 2];
    deck.grab(true);
    // Backwards, past the loop's beginning: the hand is asking for the end of the loop.
    deck.set_hand_rate(-1.0);
    for _ in 0..40 {
        deck.render(&mut out);
        let position = deck.position();
        assert!(
            position >= start && position < end,
            "the hand took the record out of the loop: {position} is outside {start}..{end}"
        );
    }
}

/// The wrap counter belongs to the motor: a hand going round is not the loop going round.
#[test]
fn scratch_does_not_count_loop_wraps() {
    let (mut deck, _, _) = deck_with_loop();
    let mut out = vec![0.0f32; 256 * 2];
    for _ in 0..4 {
        deck.render(&mut out);
    }
    let wraps = deck.stats().loop_wraps;
    deck.grab(true);
    deck.set_hand_rate(1.5);
    for _ in 0..40 {
        deck.render(&mut out);
    }
    assert_eq!(
        deck.stats().loop_wraps,
        wraps,
        "the hand counted as the motor going round the loop"
    );
}

/// Crossing the loop's edge under the hand is not a hole in the sound.
///
/// The window the hand reads is the loop's own content repeated, so the material on the far side of the edge
/// is already there: a gap would mean the deck read past the loop and into the file, or into nothing.
#[test]
fn the_hand_wraps_without_a_hole_in_the_sound() {
    let (mut deck, _, _) = deck_with_loop();
    let mut out = vec![0.0f32; 256 * 2];
    deck.grab(true);
    deck.set_hand_rate(-1.0);
    let mut heard = Vec::new();
    let mut silent_runs = 0usize;
    for _ in 0..40 {
        deck.render(&mut out);
        let mut run = 0usize;
        for sample in out.iter().step_by(2) {
            if sample.abs() < 1e-6 {
                run += 1;
                silent_runs = silent_runs.max(run);
            } else {
                run = 0;
            }
        }
        heard.extend(out.iter().step_by(2).copied());
    }
    assert!(
        silent_runs < 8,
        "the sound fell into silence for {silent_runs} frames while the hand crossed the loop's edge"
    );
    // And it is the loop's material, not a held sample: the sine keeps moving.
    let largest = heard
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        largest > 0.01,
        "the hand produced a flat line at the loop's edge (largest step {largest:.4})"
    );
}

/// A loop shorter than the window still runs: the hand cycles it instead of falling off its end.
///
/// Fifty milliseconds is shorter than the kernel's own window, so the deck cannot simply read around the
/// head — it has to page the loop, several turns per window, and the sound must still be the loop.
#[test]
fn a_short_loop_pages_under_the_hand() {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(RATE);
    deck.attach(
        Box::new(Sine {
            frames: RATE as u64 * 30,
            position: 0,
        }),
        None,
    );
    let start = 96_000u64;
    let end = start + 2_400;
    assert!(
        deck.set_loop(Some(FrameRange {
            start: dj_domain::SourceFrame(start),
            end: dj_domain::SourceFrame(end),
        })),
        "the deck refused a short loop"
    );
    deck.set_loop_mode(true);
    deck.seek(start + 1_200);
    deck.play();
    let mut out = vec![0.0f32; 256 * 2];
    deck.grab(true);
    // Backwards at speed: many turns of the loop inside one window.
    deck.set_hand_rate(-2.0);
    let mut silent = 0usize;
    for _ in 0..40 {
        deck.render(&mut out);
        let position = deck.position();
        assert!(
            position >= start && position < end,
            "the hand left a short loop: {position} is outside {start}..{end}"
        );
        let mut run = 0usize;
        for sample in out.iter().step_by(2) {
            if sample.abs() < 1e-6 {
                run += 1;
                silent = silent.max(run);
            } else {
                run = 0;
            }
        }
    }
    assert!(
        silent < 8,
        "a short loop fell silent for {silent} frames under the hand"
    );
}
