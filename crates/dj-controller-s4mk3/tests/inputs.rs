//! A press on the panel becomes the command the engine understands.
//!
//! The link under test is the whole chain the owner asked for: a report from the device, a line in the learned map, the
//! binding table's meaning, and a command. Each part is tested alone elsewhere; this is the chain.

#![allow(clippy::expect_used)]

use std::collections::HashMap;

use dj_controller_core::Mapping;
use dj_controller_s4mk3::inputs::parse_inputs;
use dj_domain::{DeckId, DjCommand};

/// One control, learned: deck A's PLAY button is bit 0 of byte 4 in report 1.
const LEARNED: &str = "\
deck_a.play 1 4 0 1
deck_b.sync 1 9 3 1
";

#[test]
fn a_pressed_button_asks_the_engine_to_play() {
    let map = parse_inputs(LEARNED).expect("the learned map parses");
    let mut previous = HashMap::new();
    let mut payload = vec![0_u8; 16];
    // The panel's resting state first: every button up.
    assert!(map.decode(1, &payload, &mut previous).is_empty());

    payload[4] = 0b0000_0001;
    let events = map.decode(1, &payload, &mut previous);
    assert_eq!(events.len(), 1, "one button, one event");
    let event = &events[0];
    assert_eq!(event.control, "deck_a.play");

    let resolved = Mapping::new().resolve(event, false);
    assert_eq!(
        resolved,
        Some((
            DeckId::A,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true
            }
        )),
        "the press asks deck A to play"
    );
}

#[test]
fn letting_go_asks_for_nothing() {
    // A button that reports its release is not a second command: the deck would otherwise stop the moment the hand
    // comes up.
    let map = parse_inputs(LEARNED).expect("the learned map parses");
    let mut previous = HashMap::new();
    let mut payload = vec![0_u8; 16];
    payload[4] = 0b0000_0001;
    assert_eq!(map.decode(1, &payload, &mut previous).len(), 1);
    payload[4] = 0;
    let events = map.decode(1, &payload, &mut previous);
    assert_eq!(events.len(), 1, "the release is seen");
    assert_eq!(
        Mapping::new().resolve(&events[0], true),
        None,
        "and means nothing"
    );
}

#[test]
fn a_second_deck_s_button_is_a_different_intent() {
    let map = parse_inputs(LEARNED).expect("the learned map parses");
    let mut previous = HashMap::new();
    let mut payload = vec![0_u8; 16];
    assert!(map.decode(1, &payload, &mut previous).is_empty());
    payload[9] = 0b0000_1000;
    let events = map.decode(1, &payload, &mut previous);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].control, "deck_b.sync");
    let (deck, command) = Mapping::new()
        .resolve(&events[0], false)
        .expect("sync is a command");
    assert_eq!(deck, DeckId::B);
    assert!(
        matches!(
            command,
            DjCommand::Sync {
                deck: DeckId::B,
                ..
            }
        ),
        "{command:?}"
    );
}

#[test]
fn the_shipped_map_is_empty_and_says_so() {
    // Nothing has been learned on this machine yet: an empty map must not pretend otherwise. A map that guessed would
    // put a command on the wrong button, and the DJ would find out on stage.
    let map = dj_controller_s4mk3::inputs::InputMap::built_in();
    assert!(
        map.is_empty(),
        "the shipped map has {} lines: they were learned on a panel and this build has none",
        map.fields().len()
    );
}
