//! A five-by-seven font: what a deck's screen needs and nothing more.
//!
//! The screens are 320×240 and sit an arm's length away on a dark stage, so the font is drawn as a grid of dots that
//! scale by whole numbers: at scale 1 a capital is five by seven pixels, at scale 4 it is twenty by twenty-eight. The
//! glyphs are written in the source as pictures, because a table of hex numbers cannot be checked by eye and a typo in
//! it looks like a design decision.
//!
//! Only capitals, digits and the punctuation a track title uses: a DJ display that renders lower case at this size is
//! a screen full of unreadable smudges, so lower case folds to upper.

/// Width of one glyph, in pixels, at scale 1.
pub const GLYPH_WIDTH: usize = 5;
/// Height of one glyph, in pixels, at scale 1.
pub const GLYPH_HEIGHT: usize = 7;
/// Blank columns between two glyphs, in pixels, at scale 1.
pub const GLYPH_SPACING: usize = 1;

/// Turn seven rows of `0`/`1` into the five low bits of seven bytes, at compile time.
const fn rows(art: [&str; GLYPH_HEIGHT]) -> [u8; GLYPH_HEIGHT] {
    let mut packed = [0u8; GLYPH_HEIGHT];
    let mut row = 0;
    while row < GLYPH_HEIGHT {
        let line = art[row].as_bytes();
        let mut column = 0;
        let mut bits = 0u8;
        while column < GLYPH_WIDTH && column < line.len() {
            if line[column] == b'1' {
                bits |= 1 << (GLYPH_WIDTH - 1 - column);
            }
            column += 1;
        }
        packed[row] = bits;
        row += 1;
    }
    packed
}

const SPACE: [u8; GLYPH_HEIGHT] = rows([
    "00000", "00000", "00000", "00000", "00000", "00000", "00000",
]);

/// The glyph for `letter`, or `None` when the font has nothing to say about it.
///
/// A caller that prints a title must decide what an unknown character means; drawing nothing is honest, drawing a
/// guessed shape is not.
pub fn glyph(letter: char) -> Option<[u8; GLYPH_HEIGHT]> {
    let upper = letter.to_ascii_uppercase();
    let art: [&str; GLYPH_HEIGHT] = match upper {
        ' ' => return Some(SPACE),
        'A' => [
            "01110", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'B' => [
            "11110", "10001", "10001", "11110", "10001", "10001", "11110",
        ],
        'C' => [
            "01110", "10001", "10000", "10000", "10000", "10001", "01110",
        ],
        'D' => [
            "11110", "10001", "10001", "10001", "10001", "10001", "11110",
        ],
        'E' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "11111",
        ],
        'F' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "10000",
        ],
        'G' => [
            "01110", "10001", "10000", "10111", "10001", "10001", "01111",
        ],
        'H' => [
            "10001", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'I' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "11111",
        ],
        'J' => [
            "00111", "00010", "00010", "00010", "00010", "10010", "01100",
        ],
        'K' => [
            "10001", "10010", "10100", "11000", "10100", "10010", "10001",
        ],
        'L' => [
            "10000", "10000", "10000", "10000", "10000", "10000", "11111",
        ],
        'M' => [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ],
        'N' => [
            "10001", "11001", "10101", "10011", "10001", "10001", "10001",
        ],
        'O' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'P' => [
            "11110", "10001", "10001", "11110", "10000", "10000", "10000",
        ],
        'Q' => [
            "01110", "10001", "10001", "10001", "10101", "10011", "01101",
        ],
        'R' => [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ],
        'S' => [
            "01111", "10000", "10000", "01110", "00001", "00001", "11110",
        ],
        'T' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        'U' => [
            "10001", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'V' => [
            "10001", "10001", "10001", "10001", "10001", "01010", "00100",
        ],
        'W' => [
            "10001", "10001", "10001", "10101", "10101", "11011", "10001",
        ],
        'X' => [
            "10001", "10001", "01010", "00100", "01010", "10001", "10001",
        ],
        'Y' => [
            "10001", "10001", "01010", "00100", "00100", "00100", "00100",
        ],
        'Z' => [
            "11111", "00001", "00010", "00100", "01000", "10000", "11111",
        ],
        '0' => [
            "01110", "10001", "10011", "10101", "11001", "10001", "01110",
        ],
        '1' => [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ],
        '2' => [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ],
        '3' => [
            "11110", "00001", "00001", "01110", "00001", "00001", "11110",
        ],
        '4' => [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ],
        '5' => [
            "11111", "10000", "11110", "00001", "00001", "10001", "01110",
        ],
        '6' => [
            "00110", "01000", "10000", "11110", "10001", "10001", "01110",
        ],
        '7' => [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ],
        '8' => [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ],
        '9' => [
            "01110", "10001", "10001", "01111", "00001", "00010", "01100",
        ],
        '.' => [
            "00000", "00000", "00000", "00000", "00000", "01100", "01100",
        ],
        ',' => [
            "00000", "00000", "00000", "00000", "01100", "00100", "01000",
        ],
        ':' => [
            "00000", "01100", "01100", "00000", "01100", "01100", "00000",
        ],
        '-' => [
            "00000", "00000", "00000", "11111", "00000", "00000", "00000",
        ],
        '_' => [
            "00000", "00000", "00000", "00000", "00000", "00000", "11111",
        ],
        '/' => [
            "00001", "00010", "00010", "00100", "01000", "01000", "10000",
        ],
        '\\' => [
            "10000", "01000", "01000", "00100", "00010", "00010", "00001",
        ],
        '%' => [
            "11001", "11010", "00010", "00100", "01000", "01011", "10011",
        ],
        '+' => [
            "00000", "00100", "00100", "11111", "00100", "00100", "00000",
        ],
        '(' => [
            "00010", "00100", "01000", "01000", "01000", "00100", "00010",
        ],
        ')' => [
            "01000", "00100", "00010", "00010", "00010", "00100", "01000",
        ],
        '[' => [
            "00110", "00100", "00100", "00100", "00100", "00100", "00110",
        ],
        ']' => [
            "01100", "00100", "00100", "00100", "00100", "00100", "01100",
        ],
        '?' => [
            "01110", "10001", "00001", "00110", "00100", "00000", "00100",
        ],
        '!' => [
            "00100", "00100", "00100", "00100", "00100", "00000", "00100",
        ],
        '\'' => [
            "00100", "00100", "00000", "00000", "00000", "00000", "00000",
        ],
        '*' => [
            "00000", "10101", "01110", "11111", "01110", "10101", "00000",
        ],
        '=' => [
            "00000", "00000", "11111", "00000", "11111", "00000", "00000",
        ],
        '>' => [
            "01000", "00100", "00010", "00001", "00010", "00100", "01000",
        ],
        '<' => [
            "00010", "00100", "01000", "10000", "01000", "00100", "00010",
        ],
        _ => return None,
    };
    Some(rows(art))
}

/// Width of `text` in pixels at `scale`, spacing included.
pub fn width_of(text: &str, scale: usize) -> usize {
    let letters = text.chars().count();
    if letters == 0 {
        return 0;
    }
    letters * (GLYPH_WIDTH + GLYPH_SPACING) * scale - GLYPH_SPACING * scale
}

/// The same text cut to fit `room` pixels at `scale`, with `..` when something was dropped.
///
/// A title on a deck screen is often longer than the screen: showing its first letters and saying so with two dots is
/// better than a word that looks complete and is not.
pub fn fit(text: &str, room: usize, scale: usize) -> String {
    if width_of(text, scale) <= room {
        return text.to_string();
    }
    let dots = "..";
    let mut kept = String::new();
    for letter in text.chars() {
        let candidate_width = width_of(&format!("{kept}{letter}{dots}"), scale);
        if candidate_width > room {
            break;
        }
        kept.push(letter);
    }
    if kept.is_empty() {
        return dots.to_string();
    }
    format!("{kept}{dots}")
}
