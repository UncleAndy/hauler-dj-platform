//! How fast a deck renders a second of material: the throughput the null-sink checks lean on.

#![allow(clippy::expect_used, clippy::cast_precision_loss)]

use dj_deck::{Deck, DeckSource};
use dj_domain::DeckId;

struct Tone(u32);
impl DeckSource for Tone {
    fn sample_rate(&self) -> u32 {
        self.0
    }
    fn frames(&self) -> Option<u64> {
        Some(48_000 * 60)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        for (i, s) in out.iter_mut().enumerate() {
            *s = ((i % 97) as f32) / 97.0 - 0.5;
        }
        Ok(out.len() / 2)
    }
    fn seek(&mut self, _frame: u64) -> Result<u64, String> {
        Ok(0)
    }
    fn seeks_are_immediate(&self) -> bool {
        true
    }
}

#[test]
fn a_deck_renders_a_second_of_material_in_far_less_than_a_second() {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(48_000);
    deck.attach(Box::new(Tone(48_000)), None);
    deck.set_tempo(1.0256);
    deck.play();
    let mut out = vec![0.0f32; 256 * 2];
    for _ in 0..80 {
        deck.render(&mut out);
    }
    let before = deck.position();
    let start = std::time::Instant::now();
    for _ in 0..187 {
        deck.render(&mut out);
    }
    let elapsed = start.elapsed().as_secs_f64();
    let consumed = deck.position().saturating_sub(before) as f64 / 48_000.0;
    eprintln!("THROUGHPUT consumed {consumed:.3} s in {elapsed:.4} s of wall clock");
    assert!(
        elapsed < 0.5,
        "a second of material took {elapsed:.3} s to render"
    );
}
