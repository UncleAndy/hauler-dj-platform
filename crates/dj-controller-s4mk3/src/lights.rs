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
