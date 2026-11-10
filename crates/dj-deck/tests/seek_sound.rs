//! A deck that was seeked while playing must come back with sound (DJP-SYNC-002).
//!
//! The engine's sync loop moves a follower onto its master's beat with a seek from the audio callback.
//! A seek there is a *request*: the decoder answers later, and the deck is silent until it does. What is
//! measured here is whether the sound comes back at all.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::Deck;
use dj_domain::DeckId;

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dj-codec/tests/fixtures/audio/drums-8s-48000.wav")
}

fn deck(window_seconds: f64, blocking: bool) -> Deck {
    let path = fixture();
    let prefetcher = Prefetcher::open(&path, PrefetchConfig::for_sample_rate(RATE, window_seconds))
        .unwrap_or_else(|err| panic!("open {}: {err}", path.display()));
    let mut deck = Deck::new(DeckId::B);
    deck.attach(Box::new(prefetcher), None);
    deck.set_blocking(blocking);
    deck.set_gain(1.0);
    deck.play();
    deck
}

/// Render `blocks` blocks and return the loudest sample.
fn render(deck: &mut Deck, blocks: usize) -> f32 {
    let mut block = vec![0.0f32; BLOCK * 2];
    let mut peak = 0.0f32;
    for _ in 0..blocks {
        deck.render(&mut block);
        for sample in &block {
            peak = peak.max(sample.abs());
        }
    }
    peak
}

#[test]
fn a_key_locked_deck_comes_back_with_sound_after_a_jump() {
    // The engine keeps a synced deck under key lock as soon as its speed differs from the file's, and a
    // beat sync then seeks it. The stretcher answers from its own window, so a seek without a pre-roll is
    // the case to watch.
    let mut deck = deck(1.0, false);
    deck.set_keylock(true);
    deck.set_tempo(1.05);
    let before = render(&mut deck, 80);
    assert!(
        before > 0.01,
        "the key-locked deck should sound before the jump: {before}"
    );
    deck.seek(deck.position() + 12_000);
    let after = render(&mut deck, 100);
    println!(
        "key locked: peak after the jump {after:.4}, underruns {}, position {}",
        deck.stats().underruns,
        deck.position()
    );
    assert!(
        after > 0.01,
        "a key-locked deck must play again after the jump, its peak is {after}"
    );
}

#[test]
fn a_deck_comes_back_with_sound_after_a_jump() {
    let mut deck = deck(1.0, false);
    let before = render(&mut deck, 40);
    assert!(
        before > 0.01,
        "the deck should sound before the jump: {before}"
    );

    // Half a beat at 120 BPM, which is 12000 frames: the distance the sync loop moves a follower.
    let target = deck.position() + 12_000;
    let landed = deck.seek(target);
    println!(
        "seeked from {target} to {landed}, underruns {}",
        deck.stats().underruns
    );
    let after = render(&mut deck, 100);
    println!(
        "peak after the jump {after:.4}, underruns {}, position {}",
        deck.stats().underruns,
        deck.position()
    );
    assert!(
        after > 0.01,
        "the deck must play again after the jump, its peak is {after}"
    );
}

#[test]
fn a_deck_comes_back_with_sound_after_a_jump_that_leaves_the_window() {
    // The same, but further than the prefetched window: the decoder has to re-fill from the new place,
    // which is the case a beat sync creates when the follower is a bar behind.
    let mut deck = deck(0.5, false);
    let before = render(&mut deck, 40);
    assert!(
        before > 0.01,
        "the deck should sound before the jump: {before}"
    );
    let target = deck.position() + 96_000;
    deck.seek(target);
    let after = render(&mut deck, 200);
    println!(
        "peak after the far jump {after:.4}, underruns {}, position {}",
        deck.stats().underruns,
        deck.position()
    );
    assert!(
        after > 0.01,
        "the deck must play again after a far jump, its peak is {after}"
    );
}

#[test]
fn a_deck_comes_back_with_sound_after_a_jump_in_the_same_block() {
    // The engine asks every few blocks; a jump asked for while the previous one is still landing is what
    // a follower on a stopped master does, and it must not silence the deck for good.
    let mut deck = deck(1.0, false);
    let _ = render(&mut deck, 40);
    for round in 0..8 {
        let target = deck.position() + 4_000;
        deck.seek(target);
        let peak = render(&mut deck, 2);
        println!(
            "round {round}: peak {peak:.4}, underruns {}, position {}",
            deck.stats().underruns,
            deck.position()
        );
    }
    let after = render(&mut deck, 100);
    println!(
        "peak after the storm {after:.4}, underruns {}",
        deck.stats().underruns
    );
    assert!(
        after > 0.01,
        "a deck seeked eight times must still play, its peak is {after}"
    );
}
