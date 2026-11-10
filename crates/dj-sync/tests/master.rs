//! Who leads: the explicit choice, the fallback, and no master at all.
use dj_domain::DeckId;
use dj_sync::{DeckCandidate, MasterPolicy, choose_master};

fn deck(deck: DeckId, loaded: bool, playing: bool) -> DeckCandidate {
    DeckCandidate {
        deck,
        loaded,
        playing,
    }
}

#[test]
fn the_deck_a_hand_chose_leads_even_when_it_is_paused() {
    let decks = [deck(DeckId::A, true, true), deck(DeckId::B, true, false)];
    // A is playing, but the DJ said B: B leads, because the choice is about the next beat.
    assert_eq!(
        choose_master(MasterPolicy::Explicit(DeckId::B), &decks),
        Some(DeckId::B)
    );
}

#[test]
fn a_chosen_deck_without_a_track_is_not_a_master() {
    let decks = [deck(DeckId::A, true, true), deck(DeckId::B, false, false)];
    assert_eq!(
        choose_master(MasterPolicy::Explicit(DeckId::B), &decks),
        None
    );
}

#[test]
fn without_a_choice_the_first_playing_deck_leads() {
    let decks = [
        deck(DeckId::A, true, false),
        deck(DeckId::B, true, true),
        deck(DeckId::C, true, true),
    ];
    // B plays before C in the order, so B leads: the same state always gives the same answer.
    assert_eq!(choose_master(MasterPolicy::Auto, &decks), Some(DeckId::B));
}

#[test]
fn a_paused_set_still_has_a_master_for_the_next_beat() {
    let decks = [deck(DeckId::A, true, false), deck(DeckId::B, false, false)];
    assert_eq!(choose_master(MasterPolicy::Auto, &decks), Some(DeckId::A));
}

#[test]
fn an_empty_set_has_none() {
    assert_eq!(choose_master(MasterPolicy::Auto, &[]), None);
}
