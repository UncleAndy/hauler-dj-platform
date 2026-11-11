//! What a track remembers about itself (DJP-DATA-006).
//!
//! The file on disk is the source of truth for everything the user decided: cue points, the beat
//! grid, loops, gain, key, rating, notes and the analysis lock. Times are frames of the source file
//! (DJP-DATA-002), never milliseconds, so a sample rate change cannot move a cue.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The schema name written into every file. A reader that does not know it refuses the file rather
/// than guessing at its fields.
pub const SCHEMA: &str = "djp-track-v1";

/// How a track is recognised after a move or a rename (DJP-DATA-004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// Size in bytes.
    pub size: u64,
    /// Hash of the first [`crate::fingerprint::WINDOW`] bytes, hex.
    pub head: String,
    /// Hash of the last window, hex.
    pub tail: String,
    /// Total frames, once something has analysed the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_frames: Option<u64>,
    /// Native sample rate of the file, which gives the frames their meaning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

impl Fingerprint {
    /// A stable key for the index: the same bytes on another machine land on the same row.
    pub fn key(&self) -> String {
        format!("{}:{}:{}", self.size, self.head, self.tail)
    }
}

/// One hot cue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cue {
    /// 0-based slot.
    pub slot: u8,
    /// Position in frames of the source file.
    pub frame: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `#rrggbb` of the pad, as the DJ chose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// One point where the grid was pinned by hand, for files whose tempo drifts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub frame: u64,
    /// Beat number counted from the first beat.
    pub beat: u64,
}

/// The beat grid: a tempo and the frame of its first beat, plus manual anchors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeatGrid {
    pub bpm: f64,
    pub first_beat_frame: u64,
    #[serde(default)]
    pub anchors: Vec<Anchor>,
    /// Beats in a bar, as the measurement counted them. A file written before this was measured keeps
    /// its four, which is what those files were drawn with.
    #[serde(default = "four_beats_to_the_bar")]
    pub beats_per_bar: u8,
}

/// A bar of four beats: what every grid without a measurement behind it is.
fn four_beats_to_the_bar() -> u8 {
    4
}

/// A stored loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loop {
    pub slot: u8,
    pub start: u64,
    pub end: u64,
}

/// Everything the metadata file holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackMeta {
    pub schema: String,
    /// Counts writes: a copy that travelled is compared by this before anything else.
    pub revision: u64,
    /// Wall clock of the last write, in milliseconds since the epoch.
    pub updated_at_ms: u64,
    pub fingerprint: Fingerprint,
    /// Content identity: the chromaprint subfingerprints, base64 of little-endian `u32`
    /// (DJP-MEDIA-005).
    ///
    /// It lives here rather than in the index because identity must travel with the file: a collection
    /// copied without its prints would have to decode every track again to know what it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chromaprint: Option<String>,
    #[serde(default)]
    pub cues: Vec<Cue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beatgrid: Option<BeatGrid>,
    #[serde(default)]
    pub loops: Vec<Loop>,
    #[serde(default = "unity_gain")]
    pub gain: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub rating: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub analysis_lock: bool,
    /// The marker that tells this copy from the one it duplicates (DJP-MEDIA-005). A pair marked this
    /// way is not a duplicate on the next addition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Fields written by a newer build are carried through, not dropped: an old version must not
    /// erase what it does not understand.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Base64 of a chromaprint, without a dependency: 4 bytes per subfingerprint, little-endian.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The print as text. Empty print gives an empty string, which readers treat as "not measured".
pub fn encode_print(print: &[u32]) -> String {
    let mut bytes = Vec::with_capacity(print.len() * 4);
    for value in print {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).copied().map(u32::from);
        let third = chunk.get(2).copied().map(u32::from);
        let index = |value: u32| ALPHABET[(value & 0b11_1111) as usize];
        out.push(char::from(index(first >> 2)));
        out.push(char::from(index(
            ((first & 0b11) << 4) | (second.unwrap_or(0) >> 4),
        )));
        match second {
            Some(second) => out.push(char::from(index(
                ((second & 0b1111) << 2) | (third.unwrap_or(0) >> 6),
            ))),
            None => out.push('='),
        }
        match third {
            Some(third) => out.push(char::from(index(third))),
            None => out.push('='),
        }
    }
    out
}

/// The print back from text. `None` when the text is not a print this build wrote.
pub fn decode_print(text: &str) -> Option<Vec<u32>> {
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let mut quartet = [0_u8; 4];
    let mut count = 0;
    let mut padding = 0;
    for character in text.bytes() {
        if character == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return None;
        }
        quartet[count] =
            u8::try_from(ALPHABET.iter().position(|known| *known == character)?).ok()?;
        count += 1;
        if count == 4 {
            bytes.push((quartet[0] << 2) | (quartet[1] >> 4));
            bytes.push(((quartet[1] & 0b1111) << 4) | (quartet[2] >> 2));
            bytes.push(((quartet[2] & 0b11) << 6) | quartet[3]);
            count = 0;
        }
    }
    match count {
        0 => {}
        2 => bytes.push((quartet[0] << 2) | (quartet[1] >> 4)),
        3 => {
            bytes.push((quartet[0] << 2) | (quartet[1] >> 4));
            bytes.push(((quartet[1] & 0b1111) << 4) | (quartet[2] >> 2));
        }
        _ => return None,
    }
    if bytes.len() % 4 != 0 {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

fn unity_gain() -> f64 {
    1.0
}

impl TrackMeta {
    /// A fresh record for a file nobody has edited yet.
    pub fn new(fingerprint: Fingerprint, now_ms: u64) -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            revision: 1,
            updated_at_ms: now_ms,
            fingerprint,
            chromaprint: None,
            cues: Vec::new(),
            beatgrid: None,
            loops: Vec::new(),
            gain: unity_gain(),
            key: None,
            rating: 0,
            notes: None,
            analysis_lock: false,
            variant: None,
            extra: BTreeMap::new(),
        }
    }

    /// True when this record should win over `other` (DJP-DATA-006: revision first, then time).
    pub fn wins_over(&self, other: &Self) -> bool {
        (self.revision, self.updated_at_ms) > (other.revision, other.updated_at_ms)
    }

    /// Mark a change: the revision counts writes, the timestamp says when.
    pub fn bump(&mut self, now_ms: u64) {
        self.revision = self.revision.saturating_add(1);
        self.updated_at_ms = now_ms;
    }
}
