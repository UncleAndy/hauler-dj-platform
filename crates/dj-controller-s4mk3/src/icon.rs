//! The application's emblem, in the panel's own colour format: 128 by 128 pixels of RGB565.
//!
//! The picture is the icon the window and the installer carry (`assets/app-icon/Hauler DJ.128.png`), so the farewell
//! cannot drift away from the program it says goodbye for. The conversion happens once, in
//! `scripts/make-screen-icon.py`: RGB565 has no alpha, so the file's transparency is laid on the screen's own
//! background there instead of being blended on a panel thread.

/// Side of the emblem, in pixels: the largest of the icon files.
pub const SIZE: usize = 128;

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
