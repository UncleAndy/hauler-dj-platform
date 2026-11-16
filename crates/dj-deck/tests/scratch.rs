//! Scratch: the hand holds the record, and the sound is what the hand does (DJP-DECK-009).
//!
//! The model is one equation: while a hand is on the record, the record's speed is the hand's speed —
//! absolute, not a correction to the motor — and the head moves by that speed per output frame, in
//! either direction.
//!
//! Reading a position back out of a sample needs care. The interpolation kernel has a passband ripple of
//! about 0,08 % at a half-frame phase, and a sample that carries its own frame number therefore looks
//! shifted by a fraction of a percent of the *level*, which on a monotonically rising code reads as
//! hundreds of frames. The material here is a sawtooth around zero (200 frames either side), so a level
//! error and a position error are different sizes: one frame of material is one unit of amplitude.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use dj_deck::{Deck, DeckSource};
use dj_domain::DeckId;

const RATE: u32 = 44_100;
/// Frames per unit of the code: a sample says its own frame number divided by this.
const CODE_SCALE: f64 = 4_000_000.0;

/// A source whose every sample says its own frame number, wrapped around zero.
struct CodeSource {
    frames: u64,
    position: u64,
}

fn code(frame: u64) -> f32 {
    (frame as f32) / (CODE_SCALE as f32)
}

impl DeckSource for CodeSource {
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
            let value = code(self.position + index as u64);
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames().unwrap_or(0).saturating_sub(1));
        Ok(self.position)
    }

    /// The cache answers a seek with an index assignment; the hand relies on that.
    fn seeks_are_immediate(&self) -> bool {
        true
    }
}

fn code_deck(frames: u64) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(RATE);
    deck.attach(
        Box::new(CodeSource {
            frames,
            position: 0,
        }),
        None,
    );
    deck
}

/// Put the deck in the state a DJ has it in: the motor running, the record under a hand.
fn start(deck: &mut Deck) {
    if !deck.is_playing() {
        deck.play();
    }
}

/// Frames the deck damps after a seek: the sound comes back over a couple of milliseconds rather than
/// standing against silence, so the first frames of a resumed block are not at the material's level.
const RESUME_FADE: usize = 64;

/// The frame a sample came from.
///
/// The kernel's passband gain ripples by about 0,08 % at a half-frame phase: on a ramp that carries its
/// own frame number, a level error of that size reads as a shift of hundreds of frames at high frame
/// numbers. So an absolute position is only read where the phase is whole (the kernel is then a plain
/// tap, gain exactly one), and half-frame phases are checked through the slope between two whole ones.
fn frame_of(sample: f32) -> f64 {
    f64::from(sample) * CODE_SCALE
}

/// A hand that holds the record still makes no sound, and the needle stays where it is.
#[test]
fn a_still_hand_is_silence() {
    let mut deck = code_deck(480_000);
    deck.seek(100_000);
    start(&mut deck);
    deck.grab(true);
    deck.set_hand_rate(0.0);

    let mut out = vec![0.0f32; 512 * 2];
    deck.render(&mut out);

    assert!(
        out.iter().all(|sample| *sample == 0.0),
        "a record that is not moving has nothing to say"
    );
    assert!(
        (deck.position() as i64 - 100_000).abs() <= 1,
        "the needle stayed in the groove, but the head is at {}",
        deck.position()
    );
}

/// The head moves by the hand's own speed, and comes home when the hand goes back as far as it went on.
#[test]
fn a_hand_moves_the_head_by_its_own_integral() {
    let mut deck = code_deck(480_000);
    deck.seek(200_000);
    start(&mut deck);
    deck.grab(true);

    let block = 441; // ten milliseconds at 44,1 kHz
    let mut out = vec![0.0f32; block * 2];
    deck.render(&mut out);
    let start_at = deck.position() as i64;

    deck.set_hand_rate(-1.0);
    for _ in 0..10 {
        deck.render(&mut out);
    }
    let back = deck.position() as i64;
    assert!(
        (back - (start_at - 4410)).abs() <= 2,
        "ten blocks of a hand at -1,0 went to {back}, not {}",
        start_at - 4410
    );

    deck.set_hand_rate(1.0);
    for _ in 0..10 {
        deck.render(&mut out);
    }
    let home = deck.position() as i64;
    assert!(
        (home - start_at).abs() <= 2,
        "the hand came back to {home} instead of {start_at}"
    );
}

/// The sound is the material the hand moves over, at the speed it moves — forwards and backwards.
#[test]
fn the_hand_plays_the_material_it_moves_over() {
    let mut deck = code_deck(480_000);
    deck.seek(300_000);
    start(&mut deck);
    deck.grab(true);
    deck.set_hand_rate(1.0);

    let block = 256;
    let mut out = vec![0.0f32; block * 2];
    deck.render(&mut out); // the ramp back from silence
    let before = deck.position() as f64;
    deck.render(&mut out);
    for index in RESUME_FADE..block {
        let heard = frame_of(out[index * 2]);
        let want = before + index as f64;
        assert!(
            (heard - want).abs() <= 1.0,
            "output frame {index} came from {heard:.1}, not {want:.1}"
        );
    }

    // The other way: the same material, read backwards.
    deck.set_hand_rate(-1.0);
    deck.render(&mut out);
    let before = deck.position() as f64;
    deck.render(&mut out);
    for index in RESUME_FADE..block {
        let heard = frame_of(out[index * 2]);
        let want = before - index as f64;
        assert!(
            (heard - want).abs() <= 1.0,
            "backwards output frame {index} came from {heard:.1}, not {want:.1}"
        );
    }
}

/// Half speed is half speed — through the whole block, tail included, where a window that ran out of
/// material would show up as silence or as a jump.
#[test]
fn a_slow_hand_moves_the_head_at_its_own_speed() {
    let mut deck = code_deck(480_000);
    deck.seek(120_000);
    start(&mut deck);
    deck.grab(true);
    deck.set_hand_rate(0.5);

    let block = 400;
    let mut out = vec![0.0f32; block * 2];
    deck.render(&mut out);
    let before = deck.position() as f64;
    deck.render(&mut out);

    assert!(
        (deck.position() as i64 - (before as i64 + 200)).abs() <= 2,
        "a hand at 0,5 moved the head to {}, not {}",
        deck.position(),
        before as i64 + 200
    );
    // Even frames sit on a whole phase: the kernel is a plain tap there, so the level names the frame.
    for index in (RESUME_FADE..block).step_by(2) {
        let want = before + 0.5 * index as f64;
        let heard = frame_of(out[index * 2]);
        assert!(
            (heard - want).abs() <= 1.0,
            "output frame {index} came from {heard:.1}, not {want:.1}"
        );
    }
    // Between two whole phases the head moves half a frame: the slope is what a half-frame phase can
    // state without being confused with the kernel's ripple.
    for index in (RESUME_FADE..block - 2).step_by(2) {
        let from = frame_of(out[index * 2]);
        let to = frame_of(out[(index + 2) * 2]);
        assert!(
            ((to - from) - 1.0).abs() <= 0.05,
            "two frames between output {index} and {} moved the head by {:.3}, not 1,000",
            index + 2,
            to - from
        );
    }
}

/// Letting go hands the record back to the motor, and the motor carries on from where the hand left it.
#[test]
fn releasing_the_record_gives_it_back_to_the_motor() {
    let mut deck = code_deck(480_000);
    deck.seek(200_000);
    start(&mut deck);
    deck.grab(true);
    deck.set_hand_rate(-1.0);

    let mut out = vec![0.0f32; 256 * 2];
    for _ in 0..4 {
        deck.render(&mut out);
    }
    let left_at = deck.position();
    assert!(left_at < 200_000, "the hand went backwards to {left_at}");

    deck.grab(false);
    deck.render(&mut out);
    let carried = deck.position();
    assert!(
        carried > left_at && carried <= left_at + 257,
        "the motor carried the record from {left_at} to {carried}"
    );
}
