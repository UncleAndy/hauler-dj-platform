//! The lights: the report, the sweep, and the two readings of its length.
//!
//! Nothing here needs a device. What needs one is the answer the sweep is asking for — which byte lights which control —
//! and that answer is written down by a person watching the controller.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_controller_s4mk3::Colour;
use dj_controller_s4mk3::lights::{
    LIGHTS_LENGTH, LightsReport, REPORT_LIGHTS, Show, byte_of, colours, known_bytes, map, map_line,
    name_of, parse_map, report_for, sweep, walk,
};

#[test]
fn a_dark_report_is_the_length_the_notes_say() {
    let report = LightsReport::dark();
    assert_eq!(report.id, REPORT_LIGHTS);
    assert_eq!(report.body.len(), LIGHTS_LENGTH);
    assert!(report.body.iter().all(|byte| *byte == 0));
}

#[test]
fn one_byte_on_and_the_rest_dark() {
    let report = LightsReport::one(12, 68).expect("byte 12 is inside the report");
    assert_eq!(report.body[12], 68);
    let lit = report.body.iter().filter(|byte| **byte != 0).count();
    assert_eq!(lit, 1, "a sweep lights one control at a time");
}

#[test]
fn a_byte_outside_the_report_is_refused() {
    assert!(LightsReport::one(LIGHTS_LENGTH, 4).is_none());
    assert!(LightsReport::one(LIGHTS_LENGTH - 1, 4).is_some());
}

#[test]
fn the_identifier_is_optional_on_the_wire() {
    let report = LightsReport::one(3, 4).expect("inside");
    let with = report.wire(true);
    let without = report.wire(false);
    assert_eq!(with.len(), LIGHTS_LENGTH + 1);
    assert_eq!(with[0], REPORT_LIGHTS);
    assert_eq!(without.len(), LIGHTS_LENGTH);
    assert_eq!(&with[1..], &without[..], "the body is the same either way");
}

#[test]
fn a_sweep_walks_one_byte_at_a_time_in_order() {
    let reports = sweep(10, 14, 4);
    assert_eq!(reports.len(), 5);
    for (offset, report) in reports.iter().enumerate() {
        let byte = 10 + offset;
        assert_eq!(report.body[byte], 4, "byte {byte} is the one lit");
        assert_eq!(
            report.body.iter().filter(|value| **value != 0).count(),
            1,
            "and it is the only one"
        );
    }
}

#[test]
fn a_sweep_stops_at_the_end_of_the_report() {
    let reports = sweep(LIGHTS_LENGTH - 2, LIGHTS_LENGTH + 10, 4);
    assert_eq!(reports.len(), 2, "the last two bytes of the report");
}

#[test]
fn a_sweep_that_starts_after_it_ends_is_empty() {
    assert!(sweep(5, 4, 4).is_empty());
}

#[test]
fn the_map_is_written_one_record_per_line_and_read_back() {
    let mut text = String::from("# byte\twhat\n");
    for (byte, name) in [(1usize, "play deck A"), (2, "cue deck A")] {
        text.push_str(&map_line(byte, name).expect("a name was given"));
    }
    let map = parse_map(&text);
    assert_eq!(
        map,
        vec![
            (1, "play deck A".to_string()),
            (2, "cue deck A".to_string())
        ]
    );
}

#[test]
fn a_byte_that_lit_nothing_is_not_written() {
    // An absence is how a missing light is recorded: a line with no name would read as "this byte lights nothing, and
    // we checked" — which is a different claim.
    assert!(map_line(7, "   ").is_none());
    assert!(parse_map("7\t\n").is_empty());
}

#[test]
fn names_with_punctuation_survive_the_round_trip() {
    let line = map_line(9, "CUE, deck A (upper)").expect("a name");
    let map = parse_map(&line);
    assert_eq!(map, vec![(9, "CUE, deck A (upper)".to_string())]);
}

#[test]
fn a_list_of_colours_is_read_in_order() {
    assert_eq!(colours("4,8,12").expect("three codes"), vec![4, 8, 12]);
    assert_eq!(colours("4 8").expect("two codes"), vec![4, 8]);
    assert!(colours("4,x").is_err(), "a colour is one byte");
    assert!(colours("").is_err(), "an empty list is not a probe");
}

#[test]
fn the_map_the_controller_taught_us_is_readable() {
    // The file is the record: it was taken byte by byte on the device, and a map that silently loses half its lines
    // would look complete while the panel went dark in the middle of a set.
    let map = map();
    assert!(
        map.len() >= 40,
        "the map has {} records; the sweep covered bytes 0..46",
        map.len()
    );
    let mut seen = std::collections::HashSet::new();
    for (byte, name) in map {
        assert!(
            *byte < LIGHTS_LENGTH as u8,
            "byte {byte} is inside the report"
        );
        assert!(!name.trim().is_empty(), "byte {byte} has no name");
        assert!(
            seen.insert(*byte),
            "byte {byte} is recorded twice, so one of the names is a correction that did not replace the first"
        );
    }
}

#[test]
fn a_known_byte_says_what_it_lights() {
    assert_eq!(name_of(0), Some("Left CUE1"));
    assert!(name_of(8).is_some_and(|name| name.contains("CUE")));
    assert!(name_of(46).is_some_and(|name| name.contains("FX")));
    assert_eq!(
        name_of(LIGHTS_LENGTH as u8),
        None,
        "the report ends before 94"
    );
}

#[test]
fn a_control_is_found_by_part_of_its_name() {
    assert_eq!(byte_of("left cue3"), Some(2));
    assert_eq!(byte_of("Left CUE3"), Some(2));
    assert!(byte_of("nothing lights under this name").is_none());
    assert!(byte_of("  ").is_none());
}

#[test]
fn every_recorded_byte_is_in_the_report() {
    let bytes = known_bytes();
    assert!(!bytes.is_empty());
    assert!(bytes.iter().all(|byte| *byte < LIGHTS_LENGTH as u8));
}

#[test]
fn an_empty_show_lights_nothing() {
    let report = report_for(&Show::default());
    assert!(
        report.body.iter().all(|byte| *byte == 0),
        "a deck that is doing nothing must not light the panel"
    );
}

#[test]
fn a_lit_hotcue_lights_the_slot_the_map_names() {
    let mut hotcues = [[false; 8]; 2];
    hotcues[0][1] = true;
    let show = Show {
        hotcues,
        ..Show::default()
    };
    let report = report_for(&show);
    assert_eq!(report.body[1], Colour::Green.code(), "Left CUE2");
    let lit = report.body.iter().filter(|byte| **byte != 0).count();
    assert_eq!(lit, 1, "one slot, one light");
}

#[test]
fn playing_monitoring_and_sync_use_their_own_bytes() {
    let show = Show {
        playing: [false, true],
        monitoring: [true, false],
        sync: [true, false],
        ..Show::default()
    };
    let report = report_for(&show);

    let playing = byte_of("Right PLAY").expect("the map has the right play button");
    let monitoring = byte_of("Left CUE").expect("the map has the left cue");
    let sync = byte_of("Sync/Lock").expect("the map has sync");
    assert_eq!(report.body[usize::from(playing)], Colour::Green.code());
    assert_eq!(report.body[usize::from(monitoring)], Colour::Blue.code());
    assert_eq!(report.body[usize::from(sync)], Colour::Yellow.code());
}

#[test]
fn the_fx_buttons_of_four_decks_have_their_own_bytes() {
    let show = Show {
        fx: [false, false, true, false],
        ..Show::default()
    };
    let report = report_for(&show);
    let deck_c = byte_of("Deck C FX").expect("the map has deck C's fx button");
    assert_eq!(report.body[usize::from(deck_c)], Colour::Magenta.code());
}

#[test]
fn a_walk_visits_every_control_the_map_knows() {
    let walk = walk();
    assert_eq!(walk.len(), map().len());
    for report in &walk {
        assert_eq!(
            report.body.iter().filter(|byte| **byte != 0).count(),
            1,
            "a walk lights one control at a time"
        );
    }
}
