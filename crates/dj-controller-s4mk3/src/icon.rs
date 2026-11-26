//! The application's emblem, in the panel's own colour format: 128 by 128 pixels of RGB565.
//!
//! The picture is the icon the window and the installer carry (`assets/app-icon/Hauler DJ.128.png`), so the farewell
//! cannot drift away from the program it says goodbye for. The conversion happens once, in
//! `scripts/make-screen-icon.py`: RGB565 has no alpha, so the file's transparency is laid on the screen's own
//! background there instead of being blended on a panel thread.

/// Side of the emblem, in pixels: the icon file, enlarged to what the screen has room for (224 of 240 rows).
pub const SIZE: usize = 224;

/// How many pixels the embedded picture holds. Kept as a function so a test can catch the file and `SIZE` drifting
/// apart — the constant is what the drawing loops trust, the file is what is actually there.
pub fn pixels() -> usize {
    PIXELS.len() / 2
}

/// The box the icon's own pixels fill: `(first column, last column, first row, last row)`.
///
/// The file's plate carries a couple of pixels of slack on one side, and the farewell centres what a person can see, not
/// the transparent square around it.
pub fn lit_box() -> (usize, usize, usize, usize) {
    let (mut first, mut last, mut top, mut bottom) = (SIZE, 0, SIZE, 0);
    for row in 0..SIZE {
        for column in 0..SIZE {
            if pixel(column, row) == crate::screen::BACKGROUND {
                continue;
            }
            first = first.min(column);
            last = last.max(column);
            top = top.min(row);
            bottom = bottom.max(row);
        }
    }
    (first, last, top, bottom)
}

/// The colour of one pixel of the emblem, in the hardware's own format; the background for anything outside it.
pub fn pixel(x: usize, y: usize) -> u16 {
    let index = (y * SIZE + x) * 2;
    match PIXELS.get(index..index + 2) {
        Some(pair) => u16::from_le_bytes([pair[0], pair[1]]),
        None => crate::screen::BACKGROUND,
    }
}

/// Two bytes to a pixel, row by row, little-endian — the file `scripts/make-screen-icon.py` writes.
static PIXELS: &[u8] = include_bytes!("../data/farewell-icon.565");
