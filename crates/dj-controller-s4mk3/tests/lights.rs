//! The lights: the report, the sweep, and the two readings of its length.
//!
//! Nothing here needs a device. What needs one is the answer the sweep is asking for — which byte lights which control —
//! and that answer is written down by a person watching the controller.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_controller_s4mk3::Colour;
use dj_controller_s4mk3::lights::{
    LIGHTS_LENGTH, LightsReport, Pad, Palette, REPORT_LIGHTS, Show, bright, brightness_probe,
    byte_of, bytes_for, colour_by_name, colour_name, colour_walk, colours, dim, fill_only,
    hotcue_bytes, known_bytes, map, map_line, name_of, nearest_colour, parse_map, pick_line,
    report_at_rest, report_for, report_with, sweep, walk,
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
    let mut hotcues = [[None; 8]; 2];
    hotcues[0][1] = Some(Pad {
        colour: Colour::Green,
        occupied: true,
    });
    let show = Show {
        hotcues,
        ..Show::default()
    };
    let report = report_for(&show);
    assert_eq!(
        report.body[1],
        bright(Colour::Green.code()),
        "Left CUE2, bright"
    );
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
    assert_eq!(
        report.body[usize::from(playing)],
        bright(Colour::Green.code())
    );
    assert_eq!(
        report.body[usize::from(monitoring)],
        bright(Colour::Blue.code())
    );
    assert_eq!(
        report.body[usize::from(sync)],
        bright(Colour::Yellow.code())
    );
}

#[test]
fn the_fx_buttons_of_four_decks_have_their_own_bytes() {
    let show = Show {
        fx: [false, false, true, false],
        ..Show::default()
    };
    let report = report_for(&show);
    let deck_c = byte_of("Deck C FX").expect("the map has deck C's fx button");
    assert_eq!(
        report.body[usize::from(deck_c)],
        bright(Colour::Magenta.code())
    );
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

#[test]
fn brightness_is_an_offset_to_the_colour() {
    // The colour table steps by four and the two low bits carry the level: the same colour at base is the faint state,
    // at base + 2 the bright one. Taken from the device's public controller script, checked on the panel.
    assert_eq!(dim(Colour::Green.code()), Colour::Green.code());
    assert_eq!(bright(Colour::Green.code()), Colour::Green.code() + 2);
    assert_eq!(dim(68), 68, "white at rest");
    assert_eq!(bright(68), 70, "white bright");
}

#[test]
fn a_dark_booth_lights_every_control_faintly_except_the_pads() {
    // The owner's request: in the dark the whole panel glows, and the active controls stand out. The hotcue pads are
    // the exception — they say what is in the deck, and an empty deck has nothing to say (owner's rule, 24.11).
    let report = report_at_rest(&Show::default(), &Palette::default());
    let pads = hotcue_bytes();
    assert_eq!(pads.len(), 16, "eight pads a side: {pads:?}");
    for (byte, name) in map() {
        if pads.contains(byte) {
            assert_eq!(
                report.body[usize::from(*byte)],
                0,
                "the pad {name} (byte {byte}) is dark until a record is loaded"
            );
        } else {
            assert_eq!(
                report.body[usize::from(*byte)],
                dim(Palette::default().resting.code()),
                "byte {byte} ({name}) is faint, not dark"
            );
        }
    }

    let mut hotcues = [[None; 8]; 2];
    hotcues[0][0] = Some(Pad {
        colour: Colour::Green,
        occupied: true,
    });
    let show = Show {
        hotcues,
        playing: [true, false],
        ..Show::default()
    };
    let report = report_at_rest(&show, &Palette::default());
    assert_eq!(report.body[0], bright(Colour::Green.code()), "the lit slot");
    let play = byte_of("Left PLAY").expect("the map has it");
    assert_eq!(
        report.body[usize::from(play)],
        bright(Colour::Green.code()),
        "the playing deck"
    );
    let neighbour = byte_of("Left CUE3").expect("the map has it");
    assert_eq!(
        report.body[usize::from(neighbour)],
        0,
        "a pad without a cue stays dark"
    );
}

#[test]
fn the_pads_are_the_sixteen_hotcue_buttons() {
    // Read from the map, so a renamed control moves the rule with it: eight pad names a side, and nothing else.
    let pads = hotcue_bytes();
    assert_eq!(pads.len(), 16, "{pads:?}");
    assert_eq!(byte_of("Left CUE1"), Some(pads[0]));
    assert_eq!(byte_of("Left CUE8"), Some(pads[7]));
    // `Left CUE` (the monitoring button, byte 8) and `Right CUE` (byte 31) are not pads: `byte_of` matches by substring,
    // so they are named by their own bytes rather than by a lookup that would find `Left CUE1` first.
    for not_a_pad in [8u8, 31] {
        assert!(
            !pads.contains(&not_a_pad),
            "{} is not a pad",
            name_of(not_a_pad).unwrap_or("?")
        );
    }
    for pad in &pads {
        let name = name_of(*pad).expect("a named control");
        assert!(name.contains("CUE"), "{name} is in the pad list");
    }
}

#[test]
fn the_brightness_probe_alternates_the_same_colour() {
    let probe = brightness_probe(55, 28, 3);
    assert_eq!(probe.len(), 6, "three rounds of two");
    for (index, report) in probe.iter().enumerate() {
        let want = if index % 2 == 0 { 28 } else { 30 };
        assert_eq!(report.body[55], want, "step {index}");
        assert_eq!(
            report.body.iter().filter(|byte| **byte != 0).count(),
            1,
            "one control at a time"
        );
    }
}

#[test]
fn the_colour_walk_visits_every_base_colour_in_both_states() {
    let steps = colour_walk(false);
    assert_eq!(steps.len(), 36, "eighteen colours, faint and bright");
    // Each colour appears twice, faint first: its base code and the same code plus two.
    for pair in steps.chunks(2) {
        let (faint, bright_step) = (&pair[0], &pair[1]);
        assert_eq!(faint.colour, bright_step.colour);
        assert_eq!(faint.state, "faint");
        assert_eq!(bright_step.state, "bright");
        assert_eq!(bright_step.code, faint.code + 2);
        assert_eq!(faint.code % 4, 0, "a base colour is a multiple of four");
    }
    assert!(
        steps
            .iter()
            .any(|step| step.colour == "green" && step.code == 28),
        "the green the owner checked is in the walk"
    );
}

#[test]
fn the_shifted_walk_adds_the_two_shades_between() {
    let steps = colour_walk(true);
    assert_eq!(steps.len(), 72, "eighteen colours, four shades each");
    let states: Vec<&str> = steps.iter().take(4).map(|step| step.state).collect();
    assert_eq!(
        states,
        vec!["faint", "bright", "faint shifted", "bright shifted"]
    );
}

#[test]
fn every_colour_has_a_name_a_person_can_read() {
    for step in colour_walk(false) {
        assert!(!step.colour.is_empty());
        assert_ne!(step.colour, "?");
    }
    assert_eq!(colour_name(Colour::Green), "green");
    assert_eq!(colour_name(Colour::White), "white");
}

#[test]
fn a_side_is_every_byte_the_owner_named_that_side() {
    // "Надо пробовать на всей деке": a side is a group of bytes, and the map is the dictionary that says which.
    let left = bytes_for("left").expect("the map knows the left side");
    assert!(left.len() > 20, "a deck has many lights: {}", left.len());
    for byte in &left {
        let name = name_of(*byte).expect("every byte in the map has a name");
        assert!(
            name.to_ascii_lowercase().contains("left"),
            "byte {byte} is {name}, which is not on the left"
        );
    }
    let right = bytes_for("right").expect("the map knows the right side");
    assert!(right.iter().all(|byte| !left.contains(byte)));
}

#[test]
fn all_means_the_whole_map_and_a_number_means_one_byte() {
    assert_eq!(bytes_for("all").expect("all").len(), map().len());
    assert_eq!(bytes_for("55").expect("a byte"), vec![55]);
    assert!(bytes_for("leftе").is_err(), "a typo is a typo");
    assert!(bytes_for("94").is_err(), "the report ends before 94");
}

#[test]
fn filling_only_these_bytes_leaves_the_others_dark() {
    let report = fill_only(30, &[1, 2]);
    assert_eq!(report.body[1], 30);
    assert_eq!(report.body[2], 30);
    assert_eq!(report.body[0], 0);
    assert_eq!(report.body[3], 0);
}

#[test]
fn a_picked_colour_is_recorded_in_the_same_shape_as_the_map() {
    let line = pick_line(68, "white").expect("a name");
    assert_eq!(line, "68\twhite\n");
    assert!(
        pick_line(68, "   ").is_none(),
        "a colour without a name is not a choice"
    );
}

#[test]
fn the_settings_choose_the_colours() {
    // "Давай сделаем цвета настраиваемыми": every state takes its colour from the palette, so the window can offer the
    // eighteen names and the panel follows.
    let palette = Palette {
        play: Colour::Red,
        hotcue: Colour::Purple,
        fx: Colour::Sky,
        cue: Colour::Salmon,
        sync: Colour::Lime,
        resting: Colour::Off,
    };
    let mut hotcues = [[None; 8]; 2];
    hotcues[0][0] = Some(Pad {
        colour: Colour::Purple,
        occupied: true,
    });
    let show = Show {
        hotcues,
        playing: [true, false],
        monitoring: [false, true],
        sync: [true, false],
        fx: [false, false, true, false],
    };
    let report = report_with(&show, &palette);
    assert_eq!(report.body[0], bright(Colour::Purple.code()), "the hotcue");
    let play = byte_of("Left PLAY").expect("the map has it");
    assert_eq!(report.body[usize::from(play)], bright(Colour::Red.code()));
    let cue = byte_of("Right CUE").expect("the map has it");
    assert_eq!(report.body[usize::from(cue)], bright(Colour::Salmon.code()));
    let fx = byte_of("Deck C FX").expect("the map has it");
    assert_eq!(report.body[usize::from(fx)], bright(Colour::Sky.code()));
}

#[test]
fn a_colour_is_found_by_the_name_the_settings_use() {
    assert_eq!(colour_by_name("green"), Some(Colour::Green));
    assert_eq!(colour_by_name(" White "), Some(Colour::White));
    assert_eq!(colour_by_name("chartreuse"), None);
}

#[test]
fn a_cue_marker_colour_becomes_the_nearest_light() {
    // The application paints a cue with `#rrggbb`; the panel has eighteen colours. The pad shows the nearest one, so a
    // hand finds the pad it painted rather than a colour the program preferred.
    assert_eq!(nearest_colour("#cc0000"), Some(Colour::Red));
    assert_eq!(nearest_colour("00CC00"), Some(Colour::Green));
    assert_eq!(nearest_colour("#cccccc"), Some(Colour::White));
    // A colour the panel does not have lands on the closest it does: a light blue is not mistaken for violet.
    let sky = nearest_colour("#33aaff").expect("a colour");
    assert!(
        matches!(sky, Colour::Sky | Colour::Celeste | Colour::Aqua),
        "#33aaff became {sky:?}"
    );
    assert_eq!(nearest_colour("nonsense"), None);
    assert_eq!(nearest_colour(""), None);
}

#[test]
fn a_pad_shows_the_colour_of_its_marker() {
    let mut hotcues = [[None; 8]; 2];
    // The slot's own colour wins over the palette.
    let palette = Palette {
        hotcue: Colour::White,
        ..Palette::default()
    };
    hotcues[0][0] = Some(Pad {
        colour: Colour::Salmon,
        occupied: true,
    });
    let show = Show {
        hotcues,
        ..Show::default()
    };
    let report = report_with(&show, &palette);
    assert_eq!(
        report.body[0],
        bright(Colour::Salmon.code()),
        "a stored cue is bright"
    );
}

#[test]
fn an_empty_slot_glows_faintly_and_a_stored_one_brightly() {
    // The owner's rule (24.11): both are visible in the dark, but only a stored cue is at full brightness — an empty pad
    // that shines like a full one promises a cue it does not have.
    let palette = Palette::default();
    let mut hotcues = [[None; 8]; 2];
    hotcues[0][0] = Some(Pad {
        colour: Colour::Green,
        occupied: true,
    });
    hotcues[0][1] = Some(Pad {
        colour: Colour::Green,
        occupied: false,
    });
    let report = report_with(
        &Show {
            hotcues,
            ..Show::default()
        },
        &palette,
    );
    assert_eq!(report.body[0], bright(Colour::Green.code()), "a stored cue");
    assert_eq!(report.body[1], dim(Colour::Green.code()), "an empty slot");
    assert!(
        report.body[1] < report.body[0],
        "the empty pad is the fainter of the two"
    );
}
