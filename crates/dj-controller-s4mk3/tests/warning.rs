//! The deck's screen wears a red frame in the last seconds of its track (owner's request, 25.11).

use dj_controller_s4mk3::screen::{BACKGROUND, WARNING, render_deck, sample_panel};
use dj_controller_s4mk3::{SCREEN_HEIGHT, SCREEN_WIDTH, Screen};

#[test]
fn a_deck_inside_its_warning_wears_a_red_frame() {
    let mut panel = sample_panel();
    panel.warning = true;
    let canvas = render_deck(&panel);

    let (width, height) = (usize::from(SCREEN_WIDTH), usize::from(SCREEN_HEIGHT));
    let thickness = 10;
    // All four sides, and the same colour: a frame that leaves one edge out is a frame the eye reads as a glitch.
    for column in 0..width {
        assert_eq!(canvas.at(column, 0), Some(WARNING), "top edge at {column}");
        assert_eq!(
            canvas.at(column, height - 1),
            Some(WARNING),
            "bottom edge at {column}"
        );
    }
    for row in 0..height {
        assert_eq!(canvas.at(0, row), Some(WARNING), "left edge at {row}");
        assert_eq!(
            canvas.at(width - 1, row),
            Some(WARNING),
            "right edge at {row}"
        );
    }
    // A frame, not a wash: the middle of the screen is still the deck.
    assert_ne!(
        canvas.at(width / 2, height / 2),
        Some(WARNING),
        "the warning covers the deck's own numbers"
    );
    // And it is exactly the frame's width, not a border that grows into the picture.
    assert_eq!(canvas.at(thickness - 1, height / 2), Some(WARNING));
    assert_ne!(
        canvas.at(thickness + 2, height / 2),
        Some(WARNING),
        "the frame is wider than {thickness} px"
    );
    assert!(
        canvas.frame(Screen::Left).is_ok(),
        "the frame does not build"
    );
}

#[test]
fn a_deck_without_the_warning_has_no_red_at_all() {
    let mut panel = sample_panel();
    panel.warning = false;
    let canvas = render_deck(&panel);
    // No pixel of the ordinary deck screen is the warning red: the colour means one thing on this panel.
    assert!(
        !canvas.pixels().iter().any(|pixel| *pixel == WARNING),
        "a deck that is nowhere near its end is wearing the warning"
    );
    assert_ne!(
        canvas.at(usize::from(SCREEN_WIDTH) / 2, 0),
        Some(WARNING),
        "the top edge is red without a warning"
    );
    assert_eq!(
        *canvas.pixels().first().unwrap_or(&0),
        canvas.pixels()[0],
        "the corner is the screen's own background"
    );
    assert_eq!(canvas.at(0, 0), Some(BACKGROUND), "the corner is not empty");
}
