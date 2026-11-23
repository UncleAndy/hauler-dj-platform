//! The two screens: the picture, the font, and the bytes the device would take.
//!
//! None of this needs a device. What needs one is the bulk endpoint and the permission to open it, and that is the one
//! thing a person with the controller in front of them has to confirm.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_controller_s4mk3::screen::{
    ACCENT, BACKGROUND, Canvas, DeckPanel, clock, from_rgb565, render_deck, render_idle,
    sample_panel,
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
fn a_pixel_goes_out_high_byte_first() {
    // A red pixel: 0xF800. The wire carries 0xF8 then 0x00, because the device wants the high byte first.
    let mut canvas = Canvas::screen(0);
    canvas.fill_rect(0, 0, 1, 1, 0xF800);
    let frame = full_screen_frame(Screen::Left, canvas.pixels()).expect("the frame fits");
    assert_eq!(&frame[16..18], &[0xF8, 0x00]);
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
        let in_header = (8..=30).contains(y);
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
