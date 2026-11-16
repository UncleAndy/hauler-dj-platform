//! What a deck consumes per second: the number every pitch and sync check is really about.
//!
//! `--pitch-check` measures exactly this through the whole engine and reports a deck moving at a third of
//! its tempo; this witness asks the deck alone, in seconds instead of minutes, so the question is answered
//! where it belongs before the engine is blamed.

#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use dj_deck::{Deck, DeckSource};
use dj_domain::DeckId;

const RATE: u32 = 48_000;

/// A source whose every frame is its own number over this scale, so a position can be read back.
const SCALE: f64 = 4_000_000.0;

struct CodeSource {
    frames: u64,
    position: u64,
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
            let value = ((self.position + index as u64) as f32) / (SCALE as f32);
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

    fn seeks_are_immediate(&self) -> bool {
        true
    }
}

fn deck() -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(RATE);
    deck.attach(
        Box::new(CodeSource {
            frames: 4_800_000,
            position: 0,
        }),
        None,
    );
    deck
}

/// One second of wall clock at a tempo has to take the same second of the file, times the tempo.
fn consumed_per_second(tempo: f64, keylock: bool) -> f64 {
    let mut deck = deck();
    deck.set_keylock(keylock);
    deck.seek(1_000_000);
    deck.set_tempo(tempo);
    deck.play();
    let mut out = vec![0.0f32; 256 * 2];
    // The stretcher spends its delay before it has a sample to give.
    for _ in 0..80 {
        deck.render(&mut out);
    }
    let before = deck.position();
    let blocks = RATE as usize / 256;
    for _ in 0..blocks {
        deck.render(&mut out);
    }
    let consumed = deck.position().saturating_sub(before) as f64;
    consumed / f64::from(RATE)
}

#[test]
fn a_deck_consumes_the_file_at_its_tempo_with_keylock() {
    let consumed = consumed_per_second(1.0256, true);
    assert!(
        (consumed - 1.0256).abs() < 0.01,
        "a key-locked deck at 1,0256 consumed {consumed:.3} seconds of the file per second"
    );
}

#[test]
fn a_deck_consumes_the_file_at_its_tempo_without_keylock() {
    let consumed = consumed_per_second(1.0256, false);
    assert!(
        (consumed - 1.0256).abs() < 0.01,
        "a deck at 1,0256 consumed {consumed:.3} seconds of the file per second"
    );
}
