//! The lights: what can be said to the controller, and how to find out where.
//!
//! The colour vocabulary came with the protocol notes: an LED report of 94 bytes, colours numbered in steps of four, and
//! eighteen named colours up to 68. What did **not** come with them is the map from a byte to a control — that table was
//! deliberately not copied, and inventing it would produce a controller that lights "on average right", which is another
//! way of saying wrong.
//!
//! So the map is learned on the device, by the only honest method: send one byte at a time and watch which control
//! lights up. This module builds those reports. It is safe by construction — a light cannot move a hand or a platter —
//! and it touches nothing but the lights report: the motor reports are behind a decision that has not been made.

use crate::Colour;

/// The output report that carries the lights.
pub const REPORT_LIGHTS: u8 = 128;
/// Its length, as the protocol notes have it. Whether this count includes the report identifier is exactly the kind of
/// question the device answers and a document does not, so both readings can be sent (`--body-only`).
pub const LIGHTS_LENGTH: usize = 94;

/// One lights report: the identifier and the body, kept apart so a caller can decide what goes on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightsReport {
    /// Report identifier, as the protocol notes name it.
    pub id: u8,
    /// The body: one byte per light, or per group of lights.
    pub body: Vec<u8>,
}

impl LightsReport {
    /// A report of `LIGHTS_LENGTH` bytes, all dark.
    pub fn dark() -> Self {
        Self {
            id: REPORT_LIGHTS,
            body: vec![0u8; LIGHTS_LENGTH],
        }
    }

    /// One byte on, the rest dark: the report that asks "which control is this one?".
    pub fn one(byte: usize, value: u8) -> Option<Self> {
        if byte >= LIGHTS_LENGTH {
            return None;
        }
        let mut report = Self::dark();
        report.body[byte] = value;
        Some(report)
    }

    /// The bytes for the wire, identifier first, or the body alone when `with_id` is false.
    pub fn wire(&self, with_id: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 1);
        if with_id {
            out.push(self.id);
        }
        out.extend_from_slice(&self.body);
        out
    }
}

/// The reports for a sweep from `from` up to and including `to`.
///
/// A sweep exists to be watched: one byte at a time, one control at a time. Two bytes at once would light two controls
/// and leave the observer guessing which of them answered.
pub fn sweep(from: usize, to: usize, value: u8) -> Vec<LightsReport> {
    let last = to.min(LIGHTS_LENGTH.saturating_sub(1));
    if from > last {
        return Vec::new();
    }
    (from..=last)
        .filter_map(|byte| LightsReport::one(byte, value))
        .collect()
}

/// One line of the map as it is written down: the byte, a tab, what it lights.
///
/// A tab rather than a comma because a name will contain one sooner or later ("CUE, deck A"), and a map whose columns
/// shift is a map that lies. Empty names are not written: a byte that lights nothing is an absence in the file, and an
/// absence is honest.
pub fn map_line(byte: usize, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(format!("{byte}\t{name}\n"))
}

/// The map as records: byte and name, in the order the file gives them.
pub fn parse_map(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let (byte, name) = line.split_once('\t')?;
            let byte = byte.trim().parse().ok()?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((byte, name.to_string()))
        })
        .collect()
}

/// The colour codes a probe walks through, from `4,8,12` or with spaces.
///
/// The point of more than one code on the same byte is the question the first sweep could not answer: does this byte
/// set a colour, a brightness, or merely "on"? One byte in three colours answers it for that byte, and a person
/// watching says which of the three it was.
pub fn colours(list: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for part in list
        .split([',', ' '])
        .filter(|part| !part.trim().is_empty())
    {
        let code: u8 = part
            .trim()
            .parse()
            .map_err(|_| format!("a colour is one byte, and {part:?} is not"))?;
        out.push(code);
    }
    if out.is_empty() {
        return Err("no colours given".to_string());
    }
    Ok(out)
}

/// The map, as the controller showed it: one byte of the lights report to what lit up.
///
/// The bytes are in `data/lights-map.txt`, taken down by the owner with `--lights-map`, one byte at a time, and that
/// file — not this code — is the record. It is data for the same reason the binding table is: a map in code cannot be
/// argued with, corrected after a second look, or replaced by a different controller.
pub const MAP_SOURCE: &str = include_str!("../data/lights-map.txt");

/// The map, parsed once.
pub fn map() -> &'static [(u8, &'static str)] {
    static PARSED: std::sync::OnceLock<Vec<(u8, &'static str)>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        let mut out: Vec<(u8, &'static str)> = Vec::new();
        for line in MAP_SOURCE.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let Some((byte, name)) = line.split_once('\t') else {
                continue;
            };
            let Ok(byte) = byte.trim().parse::<u8>() else {
                continue;
            };
            let name = name.trim();
            if !name.is_empty() {
                out.push((byte, name));
            }
        }
        out
    })
}

/// What a byte of the lights report lights up, as the owner saw it.
pub fn name_of(byte: u8) -> Option<&'static str> {
    map()
        .iter()
        .find(|(candidate, _)| *candidate == byte)
        .map(|(_, name)| *name)
}

/// The byte that lights a control whose name contains `needle`, ignoring case.
///
/// A name in the map is a person's words ("Left CUE3", "Deck select A"), so a lookup by part of those words is what a
/// caller can do without owning a table of byte numbers.
pub fn byte_of(needle: &str) -> Option<u8> {
    let needle = needle.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    map()
        .iter()
        .find(|(_, name)| name.to_ascii_lowercase().contains(&needle))
        .map(|(byte, _)| *byte)
}

/// The bytes the map has heard about, in order, for a caller that wants to light everything the owner named.
pub fn known_bytes() -> Vec<u8> {
    map().iter().map(|(byte, _)| *byte).collect()
}

/// What the panel shows right now.
///
/// Two halves, because the controller has two: the decks with their platters and hotcues are the left and the right
/// pair, while the mixer section has an FX button for each of the four decks the program runs. The struct says exactly
/// that and nothing more — no field for a light the map has not taught us.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Show {
    /// Hotcue slots lit, for the left and the right deck.
    pub hotcues: [[bool; 8]; 2],
    /// Playing, for the left and the right deck.
    pub playing: [bool; 2],
    /// Monitoring in the headphones (the CUE button), for the left and the right deck.
    pub monitoring: [bool; 2],
    /// Sync engaged, for the left and the right deck.
    pub sync: [bool; 2],
    /// FX on, for decks A, B, C and D: the mixer section has a button for each.
    pub fx: [bool; 4],
}

/// The byte that lights a hotcue slot, found by the name the owner wrote down.
fn hotcue_byte(side: usize, slot: usize) -> Option<u8> {
    let side = if side == 0 { "Left" } else { "Right" };
    byte_of(&format!("{side} CUE{}", slot + 1))
}

/// The byte for a named control, or nothing when the map has never heard of it.
fn control(name: &str) -> Option<u8> {
    byte_of(name)
}

/// The report for a state: every control that should be lit, in its colour, and everything else dark.
///
/// Colours are a choice, not a measurement: a lit hotcue is green, a playing deck green, monitoring blue, sync yellow,
/// FX magenta. The byte sets the colour — that is what the probe on byte 8 showed — so what is left is deciding which
/// colour means what, and that is written here in one place a person can argue with.
pub fn report_for(show: &Show) -> LightsReport {
    let mut report = LightsReport::dark();
    let mut set = |byte: Option<u8>, colour: u8| {
        if let Some(byte) = byte
            && let Some(slot) = report.body.get_mut(usize::from(byte))
        {
            *slot = colour;
        }
    };

    for (side, slots) in show.hotcues.iter().enumerate() {
        for (slot, lit) in slots.iter().enumerate() {
            if *lit {
                set(hotcue_byte(side, slot), Colour::Green.code());
            }
        }
    }
    for side in 0..2 {
        let names = if side == 0 {
            ("Left PLAY button", "Left CUE", "Sync/Lock")
        } else {
            ("Right PLAY button", "Right CUE", "Right Sync")
        };
        if show.playing[side] {
            set(control(names.0), Colour::Green.code());
        }
        if show.monitoring[side] {
            set(control(names.1), Colour::Blue.code());
        }
        if show.sync[side] {
            set(control(names.2), Colour::Yellow.code());
        }
    }
    for (deck, lit) in show.fx.iter().enumerate() {
        if *lit {
            let name = match deck {
                0 => "Deck A FX on",
                1 => "Deck B FX on button",
                2 => "Deck C FX on button",
                _ => "Deck D FX on button",
            };
            set(control(name), Colour::Magenta.code());
        }
    }
    report
}

/// Every control the map knows, in order, for a panel test that lights them one at a time.
pub fn walk() -> Vec<LightsReport> {
    let mut out = Vec::new();
    for (byte, _) in map() {
        if let Some(report) = LightsReport::one(usize::from(*byte), Colour::Green.code()) {
            out.push(report);
        }
    }
    out
}
