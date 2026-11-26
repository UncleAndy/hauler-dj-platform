//! What the screens say when the program closes: the emblem on the left, the name on the right (owner's request, 25.11).
//!
//! Nothing here needs a panel: both pictures are canvases, and both frames build. The eye does its part through
//! `dj-controller --farewell-preview`, which writes the same two canvases to PPM files.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_controller_s4mk3::icon;
use dj_controller_s4mk3::screen::{BACKGROUND, Canvas, FAREWELL_LINES, INK, render_farewell};
use dj_controller_s4mk3::{SCREEN_WIDTH, Screen, text};

/// How many pixels of a canvas are not the background.
fn lit(canvas: &Canvas) -> usize {
    canvas
        .pixels()
        .iter()
        .filter(|pixel| **pixel != BACKGROUND)
        .count()
}

/// The half-open box `(first column, last column, first row, last row)` the lit pixels of a canvas fill.
fn lit_box(canvas: &Canvas) -> (usize, usize, usize, usize) {
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    for row in 0..canvas.height() {
        for column in 0..canvas.width() {
            if canvas.at(column, row) != Some(BACKGROUND) {
                columns.push(column);
                rows.push(row);
            }
        }
    }
    (
        *columns.iter().min().expect("something is lit"),
        *columns.iter().max().expect("something is lit"),
        *rows.iter().min().expect("something is lit"),
        *rows.iter().max().expect("something is lit"),
    )
}

#[test]
fn the_left_screen_shows_the_emblem_in_its_own_colours() {
    let left = render_farewell(Screen::Left);

    // The emblem is the icon at its own size, in the middle of the screen: the square the mark covers is the file's
    // square, and the file's own rounded corners are what is left dark inside it.
    let (first, last, top, bottom) = lit_box(&left);
    let left_margin = first;
    let right_margin = left.width() - 1 - last;
    let top_margin = top;
    let bottom_margin = left.height() - 1 - bottom;
    assert!(
        left_margin.abs_diff(right_margin) <= 2,
        "the emblem is off centre: {left_margin} px to the left, {right_margin} to the right"
    );
    assert!(
        top_margin.abs_diff(bottom_margin) <= 2,
        "the emblem is off centre: {top_margin} px above, {bottom_margin} below"
    );
    let side = last + 1 - first;
    assert!(
        (icon::SIZE * 9 / 10..=icon::SIZE).contains(&side),
        "the emblem covers {side} px of its {} — a different size from the icon file",
        icon::SIZE
    );
    assert!(
        left_margin
            .min(right_margin)
            .min(top_margin)
            .min(bottom_margin)
            >= 20,
        "the emblem sits against the edge of the screen: {} px of room at the tightest",
        left_margin
            .min(right_margin)
            .min(top_margin)
            .min(bottom_margin)
    );

    // In colour, not as a silhouette: somewhere in the emblem red leads green by half the scale.
    let redness = left
        .pixels()
        .iter()
        .map(|pixel| {
            let red = f32::from((pixel >> 11) & 0x1f) / 31.0;
            let green = f32::from((pixel >> 5) & 0x3f) / 63.0;
            red - green
        })
        .fold(f32::MIN, f32::max);
    assert!(
        redness > 0.5,
        "the emblem has no red in it: red leads green by {redness:.2} at its best"
    );
    // Every pixel of the icon that is not the background is on the screen, and nothing else is: the ceremony is a copy,
    // not a redrawing.
    let inked = (0..icon::SIZE)
        .flat_map(|row| (0..icon::SIZE).map(move |column| (column, row)))
        .filter(|(column, row)| icon::pixel(*column, *row) != BACKGROUND)
        .count();
    assert_eq!(
        lit(&left),
        inked,
        "the left screen lights {} pixels, the icon has {inked} that are not the background",
        lit(&left)
    );
    assert!(
        inked > icon::SIZE * icon::SIZE * 8 / 10,
        "the icon is mostly transparent: {inked} pixels of {}",
        icon::SIZE * icon::SIZE
    );

    // Its transparent corners sit on the panel's own background rather than on black: RGB565 has no alpha, so the
    // generator lays the file on this colour. One step of slack covers the rounding of that blend.
    let corner = (usize::from(SCREEN_WIDTH) - icon::SIZE) / 2;
    let pixel = left.at(corner, corner).expect("a pixel");
    let channel = |word: u16, mask: u16, shift: u32| (word >> shift) & mask;
    for (mask, shift) in [(0x1f_u16, 11_u32), (0x3f, 5), (0x1f, 0)] {
        let drawn = channel(pixel, mask, shift);
        let background = channel(BACKGROUND, mask, shift);
        assert!(
            drawn.abs_diff(background) <= 1,
            "the emblem's corner is {drawn} where the background is {background}"
        );
    }
    assert_eq!(
        left.at(0, 0),
        Some(BACKGROUND),
        "the screen around the emblem is not empty"
    );
    assert!(
        left.frame(Screen::Left).is_ok(),
        "the farewell frame does not build"
    );
}

#[test]
fn the_right_screen_carries_the_name_in_two_lines_of_ink() {
    let right = render_farewell(Screen::Right);
    // Nothing but the background and the ink: the name is set in the font, not blended with anything.
    assert!(
        right
            .pixels()
            .iter()
            .all(|pixel| *pixel == BACKGROUND || *pixel == INK),
        "the right screen carries something besides the name"
    );

    // Two bands of ink: two lines.
    let rows: Vec<usize> = (0..right.height())
        .filter(|row| (0..right.width()).any(|column| right.at(column, *row) == Some(INK)))
        .collect();
    assert!(!rows.is_empty(), "the name is not on the screen at all");
    let bands = 1 + rows
        .windows(2)
        .filter(|pair| pair[1] != pair[0] + 1)
        .count();
    assert_eq!(
        bands,
        FAREWELL_LINES.len(),
        "the name is {bands} lines, wanted {}",
        FAREWELL_LINES.len()
    );

    // Centred: every line on its own, and the two lines as a block.
    //
    // Each line has its own width, so the box of ink they share says nothing about centring — «Hauler DJ» is the wider
    // of the two and the block's box is what a reader sees. That is measured band by band.
    let mut bands_of_rows: Vec<(usize, usize)> = Vec::new();
    for row in rows {
        match bands_of_rows.last_mut() {
            Some((_, last)) if *last + 1 == row => *last = row,
            _ => bands_of_rows.push((row, row)),
        }
    }
    assert_eq!(bands_of_rows.len(), FAREWELL_LINES.len());
    for (band, (first_row, last_row)) in bands_of_rows.iter().enumerate() {
        let ink: Vec<usize> = (0..right.width())
            .filter(|column| {
                (*first_row..=*last_row).any(|row| right.at(*column, row) == Some(INK))
            })
            .collect();
        let first = *ink.first().expect("ink in the band");
        let last = *ink.last().expect("ink in the band");
        let left_margin = first;
        let right_margin = right.width() - 1 - last;
        assert!(
            left_margin.abs_diff(right_margin) <= 2,
            "line {} is not centred: {left_margin} px to the left, {right_margin} to the right",
            band + 1
        );
    }
    let (_, _, top, bottom) = lit_box(&right);
    let top_margin = top;
    let bottom_margin = right.height() - 1 - bottom;
    assert!(
        top_margin.abs_diff(bottom_margin) <= 4,
        "the name is not centred: {top_margin} px above, {bottom_margin} below"
    );

    // Every line fits the screen at the scale it is drawn: a name cut off at the edge is not a name.
    for line in FAREWELL_LINES {
        assert!(
            text::width_of(line, 4) <= usize::from(SCREEN_WIDTH),
            "{line} is wider than the screen at scale 4"
        );
    }
    assert!(
        right.frame(Screen::Right).is_ok(),
        "the farewell frame does not build"
    );
}

#[test]
fn the_name_is_the_programs_name() {
    assert_eq!(FAREWELL_LINES, ["HAULER DJ", "PLATFORM"]);
}
