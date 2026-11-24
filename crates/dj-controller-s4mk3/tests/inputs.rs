//! A press on the panel becomes the command the engine understands.
//!
//! The link under test is the whole chain the owner asked for: a report from the device, a line in the map (the shipped
//! one or a learned one), the binding table's meaning, and a command.

#![allow(clippy::expect_used)]

use std::collections::HashMap;

use dj_controller_core::Mapping;
use dj_controller_s4mk3::inputs::{InputMap, merge, parse_inputs};
use dj_domain::{DeckId, DjCommand};

/// One control, learned on a panel: deck A's PLAY button is bit 0 of byte 4 in report 1.
const LEARNED: &str = "\
deck_a.play 1 4 0 1
deck_b.sync 1 14 4 1
";

#[test]
fn the_shipped_map_carries_the_six_bound_buttons() {
    // Read from the device's public description and confirmed on the panel: deck buttons live in report 1, report 3 is
    // the clock and the platters and never carries a button.
    let map = InputMap::built_in();
    assert_eq!(map.fields().len(), 6, "{:?}", map.fields());
    for field in map.fields() {
        assert_eq!(field.report, 1, "{} is not in report 1", field.control);
    }
    let names: Vec<&str> = map.fields().iter().map(|f| f.control.as_str()).collect();
    for wanted in [
        "deck_a.play_pause",
        "deck_a.cue",
        "deck_a.sync",
        "deck_b.play_pause",
        "deck_b.cue",
        "deck_b.sync",
    ] {
        assert!(
            names.contains(&wanted),
            "{wanted} is missing from {names:?}"
        );
    }
}

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
fn the_shipped_play_button_is_where_the_press_arrives() {
    // The same press, through the map that ships: this is the line the application actually runs with.
    let map = InputMap::built_in();
    let mut previous = HashMap::new();
    let mut payload = vec![0_u8; 48];
    assert!(map.decode(1, &payload, &mut previous).is_empty());
    payload[4] = 0b0000_0001;
    let events = map.decode(1, &payload, &mut previous);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].control, "deck_a.play_pause");
    // A stopped deck starts.
    assert_eq!(
        Mapping::new().resolve(&events[0], false),
        Some((
            DeckId::A,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true
            }
        ))
    );
    // The same button on a playing deck stops it: that is what the owner asked for, and what a hand expects.
    assert_eq!(
        Mapping::new().resolve(&events[0], true),
        Some((
            DeckId::A,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: false
            }
        ))
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
    payload[14] = 0b0001_0000;
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
fn a_learned_line_that_contradicts_the_shipped_one_loses() {
    // A file left over from an earlier attempt must not put a command on the wrong button.
    let shipped = InputMap::built_in();
    let learned = parse_inputs("deck_a.play_pause 1 5 0 1\ndeck_a.pause 1 5 6 1\n")
        .expect("the learned map parses");
    let (merged, notes) = merge(&shipped, &learned);
    let play = merged
        .fields()
        .iter()
        .find(|field| field.control == "deck_a.play_pause")
        .expect("the shipped line");
    assert_eq!((play.report, play.byte, play.bit), (1, 4, 0));
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("оставляю поставленную"), "{notes:?}");
    // A control the shipped map does not know is added.
    assert!(
        merged
            .fields()
            .iter()
            .any(|field| field.control == "deck_a.pause"),
        "{:?}",
        merged.fields()
    );
}

#[test]
fn a_learned_line_from_the_clock_report_is_dropped() {
    // The report that carries the counters also carries nothing a hand presses. A line learned there fires several times
    // a second on its own, and the application would start decks with nobody near the panel.
    let shipped = InputMap::built_in();
    let learned = parse_inputs("deck_a.play 3 7 2 1\ndeck_b.pause 3 9 0 1\n")
        .expect("the learned map parses");
    let (merged, notes) = merge(&shipped, &learned);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].contains("часы и джоги"), "{notes:?}");
    assert!(
        !merged
            .fields()
            .iter()
            .any(|field| field.control == "deck_a.play" || field.control == "deck_b.pause"),
        "{:?}",
        merged.fields()
    );
    // And nothing in the map lives in that report at all.
    assert!(
        merged
            .fields()
            .iter()
            .all(|field| field.report != dj_controller_s4mk3::inputs::CLOCK_REPORT),
        "{:?}",
        merged.fields()
    );
}

#[test]
fn a_learned_line_cannot_steal_a_byte_from_another_control() {
    let shipped = InputMap::built_in();
    let learned = parse_inputs("deck_a.pause 1 4 0 1\n").expect("the learned map parses");
    let (merged, notes) = merge(&shipped, &learned);
    assert!(
        !merged
            .fields()
            .iter()
            .any(|field| field.control == "deck_a.pause"),
        "a field already in use is not handed to a second control"
    );
    assert!(notes[0].contains("уже занят"), "{notes:?}");
}
