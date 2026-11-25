//! What a deck's screen says.
//!
//! Two 320×240 panels sit between the decks, and the hardware takes RGB565 rectangles. This module owns the picture:
//! the layout, the colours, the text and the two ways out of it — a frame the device would accept, and a file a person
//! can look at. The file matters: a screen design that is only verified by sending bytes to a device is a design nobody
//! has seen, and the owner has to approve it before it goes anywhere near a stage.
//!
//! The layout is deliberately spare. A screen read at a glance in a dark booth carries four things: which deck, what is
//! playing, how far in, and where the beat is. Everything else is noise at that size.

use crate::text;
use crate::{FrameError, Rect, SCREEN_HEIGHT, SCREEN_WIDTH, Screen};

/// Background: near-black with a hint of blue, so a bright title does not glow on the whole panel.
pub const BACKGROUND: u16 = 0x0841;
/// Ordinary text.
pub const INK: u16 = 0xFFFF;
/// Labels and the second line: present, but not competing.
pub const DIM: u16 = 0x8410;
/// The state of the deck, and the played part of the bar.
pub const ACCENT: u16 = 0x07E0;
/// A paused deck: warm, so the state reads without a word.
pub const WARM: u16 = 0xFD20;
/// Loop brackets and the bar's groove.
pub const GROOVE: u16 = 0x2104;

/// A picture, in the hardware's own colour format.
pub struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u16>,
}

impl Canvas {
    /// A blank panel of `width` by `height`, filled with `background`.
    pub fn new(width: usize, height: usize, background: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![background; width * height],
        }
    }

    /// A panel the size of one screen of this controller.
    pub fn screen(background: u16) -> Self {
        Self::new(
            usize::from(SCREEN_WIDTH),
            usize::from(SCREEN_HEIGHT),
            background,
        )
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The pixels, row by row, as the device wants them.
    pub fn pixels(&self) -> &[u16] {
        &self.pixels
    }

    /// One pixel, or `None` outside the panel.
    pub fn at(&self, x: usize, y: usize) -> Option<u16> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.pixels[y * self.width + x])
    }

    /// Fill a rectangle, clipping whatever falls outside the panel.
    ///
    /// Clipping is not politeness: a title two pixels too far right must not wrap round to the left edge of the next
    /// row, and a bar that runs off the end must end at the end. The arithmetic is signed for the same reason — a
    /// caller may ask for a rectangle that starts off-screen, and the part that is on-screen still belongs on-screen.
    pub fn fill_rect(&mut self, x: isize, y: isize, width: usize, height: usize, colour: u16) {
        let from_x = x.max(0) as usize;
        let from_y = y.max(0) as usize;
        let to_x = (x.max(0) as usize).saturating_add(width).min(self.width);
        let to_y = (y.max(0) as usize).saturating_add(height).min(self.height);
        for row in from_y..to_y {
            let start = row * self.width;
            for column in from_x..to_x {
                self.pixels[start + column] = colour;
            }
        }
    }

    /// The same rectangle, but only where it falls inside the panel to begin with.
    pub fn fill_rect_exact(
        &mut self,
        x: isize,
        y: isize,
        width: usize,
        height: usize,
        colour: u16,
    ) {
        if x < 0 || y < 0 {
            return;
        }
        self.fill_rect(x, y, width, height, colour);
    }

    /// Draw `text` at `x`,`y` with whole-number `scale`. Returns the advance in pixels.
    ///
    /// A character the font does not know is a space: printing a guessed shape would be a picture of a glyph that is
    /// not in the text, and a DJ reading a track name needs the letters that are there.
    pub fn text(&mut self, x: isize, y: isize, scale: usize, ink: u16, text: &str) -> usize {
        let scale = scale.max(1);
        let mut pen = x;
        for letter in text.chars() {
            if let Some(rows) = text_glyph(letter) {
                for (row, bits) in rows.iter().enumerate() {
                    for column in 0..text::GLYPH_WIDTH {
                        if bits & (1 << (text::GLYPH_WIDTH - 1 - column)) == 0 {
                            continue;
                        }
                        self.fill_rect(
                            pen + (column * scale) as isize,
                            y + (row * scale) as isize,
                            scale,
                            scale,
                            ink,
                        );
                    }
                }
            }
            pen += ((text::GLYPH_WIDTH + text::GLYPH_SPACING) * scale) as isize;
        }
        text::width_of(text, scale)
    }

    /// A full-screen frame, ready for the bulk endpoint.
    pub fn frame(&self, screen: Screen) -> Result<Vec<u8>, FrameError> {
        crate::full_screen_frame(screen, &self.pixels)
    }

    /// The picture as a binary PPM, so a person can look at it without the hardware.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.reserve(self.pixels.len() * 3);
        for pixel in &self.pixels {
            let (red, green, blue) = from_rgb565(*pixel);
            out.push(red);
            out.push(green);
            out.push(blue);
        }
        out
    }
}

fn text_glyph(letter: char) -> Option<[u8; text::GLYPH_HEIGHT]> {
    text::glyph(letter)
}

/// The eight-bit channels behind a five-six-five colour.
pub fn from_rgb565(pixel: u16) -> (u8, u8, u8) {
    let red = ((pixel >> 11) & 0x1F) as u8;
    let green = ((pixel >> 5) & 0x3F) as u8;
    let blue = (pixel & 0x1F) as u8;
    (
        (u32::from(red) * 255 / 31) as u8,
        (u32::from(green) * 255 / 63) as u8,
        (u32::from(blue) * 255 / 31) as u8,
    )
}

/// Everything one deck's screen shows.
#[derive(Clone, Debug, Default)]
pub struct DeckPanel {
    /// The deck's letter: `A`, `B`, `C` or `D`.
    pub deck: char,
    /// Track title, as the file says it.
    pub title: String,
    /// Artist, as the file says it.
    pub artist: String,
    /// Whether the deck is playing: the state is drawn, not only written.
    pub playing: bool,
    /// Whether a track is loaded at all.
    pub loaded: bool,
    /// Seconds played.
    pub elapsed_seconds: f64,
    /// Seconds left, negative when the track has run out.
    pub remaining_seconds: f64,
    /// Measured or tagged tempo, when there is one.
    pub bpm: Option<f64>,
    /// Pitch in percent: what the tempo fader is doing.
    pub pitch_percent: f64,
    /// Position in the track, 0 to 1.
    pub position: f64,
    /// The loop's start and end as fractions of the track, when a loop is active.
    pub loop_fraction: Option<(f64, f64)>,
    /// Where the beat is: 0 to 1 through the beat, when the grid is known.
    pub beat_phase: Option<f64>,
    /// Beats in the bar, for the tick marks.
    pub beats_per_bar: u8,
    /// Musical key, when the file says it.
    pub key: Option<String>,
    /// The amount one click of the MOVE knob jumps, in beats: the step the knob beside the screen applies.
    pub jump_beats: Option<f64>,
    /// The loop size the LOOP knob carries, in beats.
    pub loop_beats: Option<u32>,
    /// The strip's window as bars, one per column, left to right: the amplitudes the window would draw
    /// (`waveform::bars` — the window's own algorithm). Empty when the track has no envelope.
    pub waveform: Vec<f32>,
    /// Where the play head sits in that window, 0 to 1: the strip is a window around the play head, so the marker is
    /// not where the track has got to but where the sound is inside what is drawn.
    pub wave_position: f32,
}

/// A clock as a DJ reads it: `1:23`, `1:02:05`, and `-0:05` for the last five seconds.
pub fn clock(seconds: f64) -> String {
    let negative = seconds < 0.0;
    let total = seconds.abs();
    let whole = total.floor() as u64;
    let (hours, minutes, rest) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    let body = if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    };
    if negative { format!("-{body}") } else { body }
}

/// The picture for one deck.
///
/// The picture is built in bands, top to bottom, in the order a hand looks: which deck and what it is doing, what is
/// playing, how far in, and where the beat is. Every band is measured in whole glyph sizes, so the design does not
/// depend on a hidden number that only the author knows.
pub fn render_deck(panel: &DeckPanel) -> Canvas {
    let mut canvas = Canvas::screen(BACKGROUND);
    let width = canvas.width();
    let height = canvas.height();
    let margin = 10;
    let room = width - margin * 2;

    // Band 1 — the two numbers the knobs beside the screen carry, in the corners, because that is where the eye finds
    // them while the hand is still on the knob. Nothing else shares this row: a number that has to be looked for is a
    // number the DJ reads after the miss, not before it (owner's request, 25.11).
    let jump = match panel.jump_beats {
        Some(beats) => format!("MOVE {}", beats_label(beats)),
        None => String::from("MOVE --"),
    };
    canvas.text(margin as isize, 2, 2, INK, &jump);
    let looped = match panel.loop_beats {
        Some(beats) => format!("LOOP {beats}"),
        None => String::from("LOOP --"),
    };
    let looped_width = text_fit_width(&looped, 2);
    canvas.text(
        width as isize - looped_width - margin as isize,
        2,
        2,
        WARM,
        &looped,
    );

    // Band 2 — which deck, what it is doing, and the tempo it is doing it at.
    let letter = if panel.deck == '\0' { '?' } else { panel.deck };
    canvas.text(
        margin as isize,
        22,
        2,
        INK,
        &letter.to_string().to_ascii_uppercase(),
    );
    let (state_word, state_colour) = match (panel.loaded, panel.playing) {
        (false, _) => ("READY", DIM),
        (true, true) => ("PLAY", ACCENT),
        (true, false) => ("PAUSE", WARM),
    };
    canvas.text(
        margin as isize + 3 * text::GLYPH_WIDTH as isize * 2,
        27,
        1,
        state_colour,
        state_word,
    );
    if let Some(bpm) = panel.bpm {
        let tempo = format!("{bpm:.1}");
        let advance = text_fit_width(&tempo, 2);
        canvas.text(
            width as isize - advance - margin as isize,
            22,
            2,
            INK,
            &tempo,
        );
    }
    let pitch = format!("{:+.1}%", panel.pitch_percent);
    let pitch_width = text_fit_width(&pitch, 1);
    canvas.text(
        width as isize - pitch_width - margin as isize,
        37,
        1,
        DIM,
        &pitch,
    );

    // Band 3 — the track. A title too long is cut with two dots rather than wrapped: a second line of a title reads as
    // a second track.
    let title = text::fit(&panel.title, room, 2);
    canvas.text(margin as isize, 46, 2, INK, &title);
    let artist = text::fit(&panel.artist, room, 1);
    canvas.text(margin as isize, 64, 1, DIM, &artist);

    // Band 4 — the clocks. Elapsed is what a hand counts bars against, so it is the largest number on the screen;
    // remaining is what decides the next move and sits under it.
    canvas.text(margin as isize, 76, 4, INK, &clock(panel.elapsed_seconds));
    let left = clock(panel.remaining_seconds);
    let left_width = text_fit_width(&left, 2);
    canvas.text(
        width as isize - left_width - margin as isize,
        94,
        2,
        DIM,
        &left,
    );
    if let Some(key) = panel.key.as_deref() {
        canvas.text(margin as isize, 106, 1, DIM, &text::fit(key, room / 2, 1));
    }

    // Band 5 — the whole track as a waveform, with the play head on it. The bottom of the screen is where the eye goes
    // for "where am I and what is coming"; a bar of one colour answers only the first half (owner's request, 25.11).
    let wave_top = 118;
    let wave_height = 94;
    let middle = wave_top + wave_height / 2;
    canvas.fill_rect(
        margin as isize,
        wave_top as isize,
        room,
        wave_height,
        GROOVE,
    );
    // The strip is a window around the play head (the window's own default of sixteen beats), so the marker sits where
    // the sound is inside that window. A track with no envelope falls back to its place in the whole track, which is
    // what the strip shows when there is nothing to draw inside it.
    let marker = if panel.waveform.is_empty() {
        panel.position.clamp(0.0, 1.0) as f32
    } else {
        panel.wave_position.clamp(0.0, 1.0)
    };
    let played_x = margin as isize + (f64::from(marker) * room as f64).round() as isize;
    if !panel.waveform.is_empty() {
        // The bars come from the window's own algorithm (`waveform::bars`): smoothed, normalised and contrasted the
        // way the strip on the monitor is. This only paints them, mirrored around the middle — a bar that runs one way
        // only reads as a spike rather than as a track.
        let amplitude = wave_height as f32 / 2.0 - 3.0;
        let columns = panel.waveform.len();
        for column in 0..room {
            let at = (column * columns / room).min(columns - 1);
            let half = (panel.waveform[at] * amplitude).max(0.5);
            let x = margin as isize + column as isize;
            // What has played is drawn brighter than what is still coming: the same ink for both makes the play head
            // the only thing telling them apart, and a finger covers the play head.
            let ink = if x < played_x { ACCENT } else { DIM };
            canvas.fill_rect(
                x,
                (middle as f32 - half).round() as isize,
                1,
                (half * 2.0).max(1.0) as usize,
                ink,
            );
        }
    }
    if let Some((from, to)) = panel.loop_fraction {
        let from = from.clamp(0.0, 1.0);
        let to = to.clamp(from, 1.0);
        let left = margin as isize + (from * room as f64).round() as isize;
        let right = margin as isize + (to * room as f64).round() as isize;
        canvas.fill_rect(
            left,
            wave_top as isize - 6,
            (right - left).max(2) as usize,
            5,
            WARM,
        );
    }
    // The play head last, so nothing drawn over it can hide where the sound is.
    canvas.fill_rect(played_x, wave_top as isize - 6, 2, wave_height + 12, INK);

    // The floor: a rule that closes the picture, and the state repeated in words a colour-blind eye can read.
    canvas.fill_rect(0, height as isize - 16, width, 1, GROOVE);
    canvas.text(margin as isize, height as isize - 13, 1, DIM, state_word);
    if panel.loop_fraction.is_some() {
        let hint = "LOOP ON";
        let hint_width = text_fit_width(hint, 1);
        canvas.text(
            width as isize - hint_width - margin as isize,
            height as isize - 13,
            1,
            DIM,
            hint,
        );
    }

    canvas
}

/// A beat count as a hand says it: whole beats without a tail, and a fraction with two digits.
fn beats_label(beats: f64) -> String {
    if (beats - beats.round()).abs() < 1e-9 {
        format!("{}", beats.round() as i64)
    } else {
        format!("{beats:.2}")
    }
}

fn text_fit_width(text: &str, scale: usize) -> isize {
    text::width_of(text, scale.max(1)) as isize
}

/// A screen with nothing loaded says so, rather than showing an empty deck that looks broken.
pub fn render_idle(deck: char) -> Canvas {
    let mut canvas = Canvas::screen(BACKGROUND);
    let width = canvas.width();
    let height = canvas.height();
    canvas.text(10, 6, 3, INK, &deck.to_string().to_ascii_uppercase());
    canvas.text(10, 48, 2, DIM, "NO TRACK");
    canvas.text(10, 70, 1, DIM, "LOAD ONE WITH THE BROWSE KNOB");
    canvas.fill_rect(0, height as isize - 14, width, 1, GROOVE);
    canvas
}

/// The one frame a check renders: enough to tell a person what the screen looks like.
pub fn sample_panel() -> DeckPanel {
    DeckPanel {
        deck: 'A',
        title: "Midnight City Lights (Extended Mix)".to_string(),
        artist: "Somebody & The Other One".to_string(),
        playing: true,
        loaded: true,
        elapsed_seconds: 83.4,
        remaining_seconds: 236.6,
        bpm: Some(124.0),
        pitch_percent: 2.4,
        position: 0.26,
        loop_fraction: Some((0.24, 0.31)),
        beat_phase: Some(0.35),
        beats_per_bar: 4,
        key: Some("8A".to_string()),
        jump_beats: Some(4.0),
        loop_beats: Some(8),
        wave_position: 0.5,
        // A made-up track: a swell, a chorus and one quiet break, so the strip shows what a track looks like.
        waveform: (0..300)
            .map(|column| {
                let x = column as f32 / 300.0;
                let body = 0.42 + 0.4 * (x * std::f32::consts::PI).sin();
                let break_in = if (0.62..0.72).contains(&x) { 0.35 } else { 0.0 };
                (body - break_in).clamp(0.0, 0.92)
            })
            .collect(),
    }
}

/// Make a frame from the sample panel, for `--screen-preview`.
pub fn sample_frame(screen: Screen) -> Result<(Canvas, Vec<u8>), FrameError> {
    let canvas = render_deck(&sample_panel());
    let frame = canvas.frame(screen)?;
    Ok((canvas, frame))
}

/// A rectangle covering the whole panel — kept here so a caller does not have to build one to ask for a full frame.
pub fn whole_screen() -> Rect {
    Rect::full()
}
