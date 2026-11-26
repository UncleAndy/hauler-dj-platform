//! The two screens: the picture, the font, and the bytes the device would take.
//!
//! None of this needs a device. What needs one is the bulk endpoint and the permission to open it, and that is the one
//! thing a person with the controller in front of them has to confirm.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_controller_s4mk3::screen::{
    ACCENT, BACKGROUND, Canvas, DeckPanel, GROOVE, INK, clock, from_rgb565, render_deck,
    render_idle, sample_panel,
};
use dj_controller_s4mk3::text;
use dj_controller_s4mk3::{SCREEN_HEIGHT, SCREEN_WIDTH, Screen, full_screen_frame};

#[test]
fn a_screen_is_three_hundred_and_twenty_by_two_hundred_and_forty() {
    let canvas = Canvas::screen(BACKGROUND);
    assert_eq!(canvas.width(), usize::from(SCREEN_WIDTH));
    assert_eq!(canvas.height(), usize::from(SCREEN_HEIGHT));
    assert_eq!(canvas.pixels().len(), 320 * 240);
}

#[test]
fn a_full_frame_is_the_length_the_protocol_says() {
    // 16 bytes of header, 320×240 pixels of two bytes, 4 bytes of footer.
    let canvas = Canvas::screen(BACKGROUND);
    let frame = canvas.frame(Screen::Left).expect("the frame fits");
    assert_eq!(frame.len(), 16 + 320 * 240 * 2 + 4);
    assert_eq!(
        &frame[..4],
        &[0x84, 0x00, 0x00, 0x21],
        "the header names the screen"
    );
    assert_eq!(
        &frame[12..16],
        &[0x01, 0x40, 0x00, 0xf0],
        "320 by 240, big endian"
    );
    assert_eq!(
        &frame[frame.len() - 4..],
        &[0x40, 0x00, 0x00, 0x00],
        "the footer closes it"
    );
}

#[test]
fn the_second_screen_is_named_in_the_header_and_the_footer() {
    let canvas = Canvas::screen(BACKGROUND);
    let frame = canvas.frame(Screen::Right).expect("the frame fits");
    assert_eq!(frame[2], 0x01, "the right panel");
    assert_eq!(
        frame[frame.len() - 2],
        0x01,
        "the footer agrees with the header"
    );
}

#[test]
fn a_pixel_goes_out_high_byte_first_and_red_in_the_low_bits() {
    // A red pixel, drawn the usual way: 0xF800. This panel takes blue in the high five bits — a red frame came out blue on
    // the owner's unit (25.11) — so red leaves in the low ones, and the wire carries 0x00 then 0x1F.
    let mut canvas = Canvas::screen(0);
    canvas.fill_rect(0, 0, 1, 1, 0xF800);
    let frame = full_screen_frame(Screen::Left, canvas.pixels()).expect("the frame fits");
    assert_eq!(&frame[16..18], &[0x00, 0x1F]);
    // White, green and black are the same either way round: the swap must not touch them.
    for (drawn, expected) in [
        (0xFFFF_u16, [0xFF_u8, 0xFF]),
        (0x07E0, [0x07, 0xE0]),
        (0x0000, [0x00, 0x00]),
    ] {
        let canvas = Canvas::screen(drawn);
        let frame = full_screen_frame(Screen::Left, canvas.pixels()).expect("the frame fits");
        assert_eq!(&frame[16..18], &expected, "{drawn:#06x}");
    }
}

#[test]
fn a_rectangle_that_runs_off_the_panel_stops_at_the_edge() {
    let mut canvas = Canvas::screen(BACKGROUND);
    // Starting left of the panel and running past its end: only the part inside is drawn, and nothing wraps.
    canvas.fill_rect(-5, 10, 400, 4, ACCENT);
    assert_eq!(canvas.at(0, 10), Some(ACCENT));
    assert_eq!(canvas.at(319, 10), Some(ACCENT));
    assert_eq!(canvas.at(319, 9), Some(BACKGROUND));
    assert_eq!(canvas.at(319, 14), Some(BACKGROUND));
}

#[test]
fn the_font_draws_the_letters_it_promises_and_folds_case() {
    // Every glyph the font claims is non-empty except the space, and every row fits five columns.
    for letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,:-_/%+()[]?!'*=><".chars() {
        let glyph = text::glyph(letter).unwrap_or_else(|| panic!("{letter} is in the font"));
        for row in glyph {
            assert!(
                row & 0b1110_0000 == 0,
                "{letter}: a row wider than five columns"
            );
        }
        assert!(
            glyph.iter().any(|row| *row != 0),
            "{letter} is drawn as nothing"
        );
    }
    assert!(text::glyph(' ').is_some());
    assert_eq!(
        text::glyph('a'),
        text::glyph('A'),
        "lower case folds to upper: there is no room for two shapes"
    );
    assert!(
        text::glyph('@').is_none(),
        "the font does not invent glyphs"
    );
}

#[test]
fn a_letter_is_drawn_where_it_is_asked_for() {
    let mut canvas = Canvas::screen(0);
    canvas.text(10, 20, 1, ACCENT, "A");
    // The first row of A is 01110: lit columns 1, 2 and 3 of five.
    assert_eq!(
        canvas.at(10, 20),
        Some(0),
        "column 0 of the first row is empty"
    );
    assert_eq!(canvas.at(11, 20), Some(ACCENT));
    assert_eq!(canvas.at(13, 20), Some(ACCENT));
    assert_eq!(canvas.at(14, 20), Some(0));
    // The fourth row of A is 11111: the whole width.
    assert_eq!(canvas.at(10, 23), Some(ACCENT));
}

#[test]
fn a_scale_of_two_draws_four_pixels_per_dot() {
    let mut canvas = Canvas::screen(0);
    canvas.text(0, 0, 2, ACCENT, "A");
    for y in 0..2 {
        for x in 2..6 {
            assert_eq!(
                canvas.at(x, y),
                Some(ACCENT),
                "the dot at 1,0 covers {x},{y}"
            );
        }
    }
}

#[test]
fn a_title_too_long_is_cut_with_two_dots() {
    let long = "A track name that cannot possibly fit on a screen of this width at scale two";
    let room = 300;
    let fitted = text::fit(long, room, 2);
    assert!(fitted.len() < long.len(), "something was dropped: {fitted}");
    assert!(fitted.ends_with(".."), "and it says so: {fitted}");
    assert!(
        text::width_of(&fitted, 2) <= room,
        "{} does not fit {room}",
        text::width_of(&fitted, 2)
    );
}

#[test]
fn a_short_title_is_left_alone() {
    assert_eq!(text::fit("HELLO", 300, 1), "HELLO");
}

#[test]
fn the_clock_reads_like_a_clock() {
    assert_eq!(clock(0.0), "0:00");
    assert_eq!(clock(83.4), "1:23");
    assert_eq!(clock(59.9), "0:59");
    assert_eq!(clock(3_725.0), "1:02:05");
    assert_eq!(clock(-5.0), "-0:05");
}

#[test]
fn a_playing_deck_shows_its_state_and_a_paused_one_shows_another() {
    // The state is a word and a colour, and it lives in one band at the top of the screen. Comparing the whole picture
    // and then checking that every difference stays inside that band proves both halves: the state is drawn, and it is
    // the only thing that changed.
    let playing = render_deck(&sample_panel());
    let mut paused_panel = sample_panel();
    paused_panel.playing = false;
    let paused = render_deck(&paused_panel);

    let differing: Vec<(usize, usize)> = (0..playing.height())
        .flat_map(|y| (0..playing.width()).map(move |x| (x, y)))
        .filter(|(x, y)| playing.at(*x, *y) != paused.at(*x, *y))
        .collect();
    assert!(
        !differing.is_empty(),
        "the state is drawn, not only written"
    );
    // The state is written twice: large beside the deck's letter, and once more in the floor line where a
    // colour-blind eye reads it. Those two bands and nothing else.
    let height = playing.height();
    for (x, y) in &differing {
        // The header band holds the two knob numbers and, under them, the deck and its state: the state word moved
        // down when the corners took the top row (owner's request, 25.11).
        let in_header = (8..=42).contains(y);
        let in_floor = *y >= height - 16;
        assert!(
            in_header || in_floor,
            "the difference at {x},{y} is outside the two bands where the state is written"
        );
    }
}

#[test]
fn an_empty_deck_says_so() {
    let idle = render_idle('B');
    let blank = Canvas::screen(BACKGROUND);
    assert_ne!(idle.pixels(), blank.pixels(), "something is written there");
    let panel = DeckPanel {
        deck: 'B',
        ..DeckPanel::default()
    };
    let ready = render_deck(&panel);
    assert_ne!(ready.pixels(), blank.pixels());
}

#[test]
fn the_picture_a_person_can_look_at_is_a_ppm() {
    let canvas = render_deck(&sample_panel());
    let ppm = canvas.to_ppm();
    assert!(
        ppm.starts_with(b"P6\n320 240\n255\n"),
        "a binary PPM header"
    );
    assert_eq!(ppm.len(), "P6\n320 240\n255\n".len() + 320 * 240 * 3);
}

#[test]
fn rgb565_is_turned_back_into_channels() {
    assert_eq!(from_rgb565(0xF800), (255, 0, 0));
    assert_eq!(from_rgb565(0x07E0), (0, 255, 0));
    assert_eq!(from_rgb565(0x001F), (0, 0, 255));
}

/// Pixels of a band that are not the screen's background: ink, paper, or the strip's own groove.
fn inked(canvas: &Canvas, x: usize, y: usize, width: usize, height: usize) -> usize {
    let mut count = 0;
    for column in x..x + width {
        for row in y..y + height {
            if canvas
                .at(column, row)
                .is_some_and(|pixel| pixel != BACKGROUND)
            {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn the_corners_carry_the_jump_step_and_the_loop_size() {
    // The two numbers the knobs beside the screen carry sit in the corners, where the eye finds them while the hand is
    // still on the knob — and they follow the knob, so what the screen says and what a click does cannot disagree
    // (owner's request, 25.11).
    let mut panel = DeckPanel {
        deck: 'A',
        loaded: true,
        playing: true,
        ..DeckPanel::default()
    };
    panel.jump_beats = Some(4.0);
    panel.loop_beats = Some(8);
    let small = render_deck(&panel);
    panel.jump_beats = Some(16.0);
    panel.loop_beats = Some(32);
    let large = render_deck(&panel);

    // Both corners are written on, and both change when the numbers do.
    let (top, height) = (2usize, 16usize);
    assert!(
        inked(&small, 10, top, 110, height) > 0,
        "the jump step is not in the left corner"
    );
    assert!(
        inked(&small, 210, top, 100, height) > 0,
        "the loop size is not in the right corner"
    );
    let changed_left = (10..120).any(|x| small.at(x, 8) != large.at(x, 8));
    let changed_right = (210..310).any(|x| small.at(x, 8) != large.at(x, 8));
    assert!(changed_left, "the jump step did not follow its knob");
    assert!(changed_right, "the loop size did not follow its knob");
}

#[test]
fn the_bottom_of_the_screen_is_the_strip_with_a_play_head() {
    // The whole track at the bottom, with the play head on it: a strip of one colour says how far along but not what is
    // coming, and the next drop is what the eye is looking for (owner's request, 25.11).
    let mut panel = DeckPanel {
        deck: 'A',
        loaded: true,
        playing: true,
        position: 0.5,
        wave_position: 0.5,
        ..DeckPanel::default()
    };
    panel.waveform = vec![0.6; 300];
    let canvas = render_deck(&panel);

    // Sound on both sides of the play head, and the play head itself a bright line in the middle of the strip.
    assert!(
        canvas.at(60, 160).is_some_and(|pixel| pixel != GROOVE),
        "the track before the play head is not drawn"
    );
    assert!(
        canvas.at(240, 160).is_some_and(|pixel| pixel != GROOVE),
        "the track after the play head is not drawn"
    );
    let head = 10 + 150;
    assert_eq!(
        canvas.at(head, 160),
        Some(INK),
        "the play head is not where the sound is"
    );
}

#[test]
fn a_track_without_an_envelope_keeps_its_strip_empty() {
    // No envelope, no wave: an invented picture would be a strip that describes a track nobody loaded.
    let panel = DeckPanel {
        deck: 'A',
        loaded: true,
        playing: true,
        position: 0.25,
        ..DeckPanel::default()
    };
    let canvas = render_deck(&panel);
    assert_eq!(
        canvas.at(60, 160),
        Some(GROOVE),
        "an empty track drew a wave"
    );
    assert_eq!(
        canvas.at(10 + 75, 160),
        Some(INK),
        "the play head is missing without a wave"
    );
}

#[test]
fn a_bar_is_drawn_around_the_middle() {
    // One amplitude per bar, mirrored: a bar that runs one way only reads as a spike rather than as a track.
    let mut panel = DeckPanel {
        deck: 'A',
        loaded: true,
        playing: true,
        position: 0.5,
        ..DeckPanel::default()
    };
    panel.waveform = vec![0.6; 300];
    let canvas = render_deck(&panel);
    let middle = 118 + 94 / 2;
    assert!(
        canvas
            .at(60, middle - 20)
            .is_some_and(|pixel| pixel != GROOVE),
        "the half above the middle is missing"
    );
    assert!(
        canvas
            .at(60, middle + 20)
            .is_some_and(|pixel| pixel != GROOVE),
        "the half below the middle is missing"
    );
    // And a silent column stays a thin line rather than a bar.
    panel.waveform = vec![0.0; 300];
    let quiet = render_deck(&panel);
    assert_eq!(
        quiet.at(60, middle - 10),
        Some(GROOVE),
        "a silent track drew a bar"
    );
}
