//! What the screens say when the program closes: the emblem on the left, the name in a rainbow on the right (owner's
//! request, 25.11).
//!
//! Nothing here needs a panel: both pictures are canvases, and both frames build. The eye does its part through
//! `dj-controller --farewell-preview`, which writes the same two canvases to PPM files.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_controller_s4mk3::icon;
use dj_controller_s4mk3::screen::{BACKGROUND, Canvas, FAREWELL_LINES, render_farewell};
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

/// The hue of a colour in degrees, read out of the hardware's own five, six and five bits.
fn hue_of(pixel: u16) -> f32 {
    let red = f32::from((pixel >> 11) & 0x1f) / 31.0;
    let green = f32::from((pixel >> 5) & 0x3f) / 63.0;
    let blue = f32::from(pixel & 0x1f) / 31.0;
    let most = red.max(green).max(blue);
    let least = red.min(green).min(blue);
    let span = most - least;
    if span <= 0.000_1 {
        return 0.0;
    }
    let hue = if (most - red).abs() < f32::EPSILON {
        60.0 * ((green - blue) / span)
    } else if (most - green).abs() < f32::EPSILON {
        60.0 * (2.0 + (blue - red) / span)
    } else {
        60.0 * (4.0 + (red - green) / span)
    };
    hue.rem_euclid(360.0)
}

#[test]
fn the_icon_file_and_the_size_the_code_trusts_are_the_same_picture() {
    // The constant is what the drawing loops walk; the file is what is actually there. Moving one without the other
    // draws half an emblem or reads past the end, and neither says so out loud.
    assert_eq!(
        icon::pixels(),
        icon::SIZE * icon::SIZE,
        "the embedded icon has {} pixels, SIZE says {}",
        icon::pixels(),
        icon::SIZE * icon::SIZE
    );
    let side = icon::SIZE;
    assert!(
        side >= 200,
        "the emblem is {side} px — the owner asked for it bigger than the icon file itself"
    );
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
            >= 4,
        "the emblem sits against the edge of the screen"
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

    // Every pixel of the icon that is not the background is on the screen, and nothing else is: the picture is a copy,
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
        left.frame(Screen::Left).is_ok(),
        "the farewell frame does not build"
    );
}

#[test]
fn the_right_screen_carries_the_name_in_two_lines() {
    let right = render_farewell(Screen::Right);

    // Two bands of lit pixels: two lines.
    let rows: Vec<usize> = (0..right.height())
        .filter(|row| (0..right.width()).any(|column| right.at(column, *row) != Some(BACKGROUND)))
        .collect();
    assert!(!rows.is_empty(), "the name is not on the screen at all");
    let mut bands: Vec<(usize, usize)> = Vec::new();
    for row in rows {
        match bands.last_mut() {
            Some((_, last)) if *last + 1 == row => *last = row,
            _ => bands.push((row, row)),
        }
    }
    assert_eq!(
        bands.len(),
        FAREWELL_LINES.len(),
        "the name is {} lines, wanted {}",
        bands.len(),
        FAREWELL_LINES.len()
    );

    // Centred: every line on its own (each has its own width), and the two lines as a block.
    for (band, (first_row, last_row)) in bands.iter().enumerate() {
        let ink: Vec<usize> = (0..right.width())
            .filter(|column| {
                (*first_row..=*last_row).any(|row| right.at(*column, row) != Some(BACKGROUND))
            })
            .collect();
        let first = *ink.first().expect("ink in the band");
        let last = *ink.last().expect("ink in the band");
        assert!(
            first.abs_diff(right.width() - 1 - last) <= 2,
            "line {} is not centred: {first} px to the left, {} to the right",
            band + 1,
            right.width() - 1 - last
        );
    }
    let (_, _, top, bottom) = lit_box(&right);
    assert!(
        top.abs_diff(right.height() - 1 - bottom) <= 4,
        "the name is not centred: {top} px above, {} below",
        right.height() - 1 - bottom
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
fn the_name_runs_through_the_spectrum_from_left_to_right() {
    let right = render_farewell(Screen::Right);
    let (first, last, top, bottom) = lit_box(&right);

    // The hue of the ink, column by column: red at the left end of the run, violet at the right, and the rest of the
    // spectrum in between rather than one flat colour.
    let mut hues = Vec::new();
    for column in first..=last {
        for row in top..=bottom {
            if let Some(pixel) = right.at(column, row)
                && pixel != BACKGROUND
            {
                hues.push((column, hue_of(pixel)));
                break;
            }
        }
    }
    assert!(
        hues.len() > 60,
        "the run is only {} columns long",
        hues.len()
    );
    let (_, left_hue) = hues[0];
    let (_, right_hue) = *hues.last().expect("a run");
    assert!(
        !(40.0..=320.0).contains(&left_hue),
        "the run does not start red: its left end is at {left_hue:.0} degrees"
    );
    assert!(
        (250.0..=320.0).contains(&right_hue),
        "the run does not end violet: its right end is at {right_hue:.0} degrees"
    );

    // Twelve sectors of the circle: the run has to pass through most of them to be a spectrum.
    let mut sectors = [false; 12];
    for (_, hue) in &hues {
        sectors[((hue / 30.0) as usize).min(11)] = true;
    }
    let visited = sectors.iter().filter(|seen| **seen).count();
    assert!(
        visited >= 8,
        "the run visits only {visited} of twelve thirty-degree sectors: it is not a spectrum"
    );
}

#[test]
fn the_name_is_the_programs_name() {
    assert_eq!(FAREWELL_LINES, ["HAULER DJ", "PLATFORM"]);
}
