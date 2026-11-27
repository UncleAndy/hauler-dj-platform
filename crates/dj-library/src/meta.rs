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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub frame: u64,
    /// Beat number counted from the first beat.
    pub beat: u64,
    /// Tempo of the stretch this anchor starts, in BPM.
    ///
    /// Written by this build for every anchor. A file written before it has none: such a stretch took
    /// its tempo from the anchor after it, which is how a two-point grid used to be read — and which
    /// left the last stretch with the tempo of the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
}

/// How far a track's stretches may differ and still count as one tempo: three percent (owner's rule, 26.11).
///
/// The comparison is the fastest stretch against the slowest. A track whose sections sit at 128 and 132 is one tempo to
/// an ear and one number to a list, so it wears no symbol; below this a "trend" would be a claim about noise.
pub const VARIABLE_BPM_TOLERANCE: f64 = 0.03;

/// How far a stretch must differ from the one before it to count as a step: a tenth of a percent (owner's rule, 26.11).
///
/// The measured tempo carries noise, and a step built from a hundredth of a BPM would be a symbol that says nothing
/// about the track. A tenth of a percent is above that noise and below anything a DJ would call a change of tempo.
pub const BPM_STEP_TOLERANCE: f64 = 0.001;

/// The suffix a track's tempo wears in a list, after its number (owner's request, 26.11).
///
/// `RISING` for a track that only ever gets faster, `FALLING` for one that only ever slows down, `FLUCTUATING` for one
/// that does both, and an empty string for a track of one tempo. The number is the stretch that plays longest
/// (`dj_domain::BeatGrid::dominant_bpm`), and the suffix says the number is not the whole track.
///
/// Each constant is the whole suffix, spaces included: an arrow reads as part of the number (`125.00↗`), and a tilde is
/// a separate mark and takes its own space (`125.00 ~`). A list appends the string and decides nothing about spacing.
pub const BPM_RISING: &str = "\u{2197}";
pub const BPM_FALLING: &str = "\u{2198}";
pub const BPM_FLUCTUATING: &str = " ~";

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

impl BeatGrid {
    /// The tempo of each stretch in track order: the first one from `bpm`, the rest from their anchors.
    ///
    /// A file written before anchors carried a tempo states none for those stretches, and such a grid cannot be told
    /// apart from a single-tempo one — it answers with the one tempo it states, which is what it was drawn as.
    fn stretch_tempos(&self) -> Vec<f64> {
        let mut tempos = vec![self.bpm];
        for anchor in &self.anchors {
            match anchor.bpm.filter(|value| value.is_finite() && *value > 0.0) {
                Some(bpm) => tempos.push(bpm),
                // A stretch with no tempo of its own is not a second tempo: the file says one number for the track.
                None => return tempos,
            }
        }
        tempos
    }

    /// Whether this grid holds more than one tempo: the owner's three percent, the same stretch against stretch.
    pub fn is_variable(&self) -> bool {
        let tempos = self.stretch_tempos();
        let Some(&first) = tempos.first() else {
            return false;
        };
        if !(first.is_finite() && first > 0.0) {
            return false;
        }
        let slowest = tempos.iter().copied().fold(first, f64::min);
        let fastest = tempos.iter().copied().fold(first, f64::max);
        slowest > 0.0 && fastest > slowest * (1.0 + VARIABLE_BPM_TOLERANCE)
    }

    /// Which way the track's tempo moves, as the symbol that follows its number.
    ///
    /// A track of one tempo wears nothing — the symbol answers only where the number is not the whole track
    /// ([`Self::is_variable`]). Then every stretch is compared with the one before it: all steps up is a rise, all steps
    /// down is a fall, both is a fluctuation. Steps smaller than [`BPM_STEP_TOLERANCE`] are not steps, so a track that
    /// rises and then holds keeps its arrow.
    pub fn bpm_trend(&self) -> &'static str {
        if !self.is_variable() {
            return "";
        }
        let tempos = self.stretch_tempos();
        let mut rose = false;
        let mut fell = false;
        for pair in tempos.windows(2) {
            let before = pair[0];
            let after = pair[1];
            if !(before.is_finite() && after.is_finite() && before > 0.0) {
                return "";
            }
            if after > before * (1.0 + BPM_STEP_TOLERANCE) {
                rose = true;
            } else if after < before * (1.0 - BPM_STEP_TOLERANCE) {
                fell = true;
            }
            if rose && fell {
                return BPM_FLUCTUATING;
            }
        }
        match (rose, fell) {
            (true, false) => BPM_RISING,
            (false, true) => BPM_FALLING,
            _ => "",
        }
    }
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

/// The names a file was imported under, so a collection list shows them without decoding anything:
/// the file name carries the artist and the title, but never the album.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTags {
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub title: String,
    /// The genres the tag carried, already split by `genres::split`: one string can list several, and
    /// the collection filters by them. An older record has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
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
    /// The file's own loudness correction in decibels, measured once. The share from the settings is applied
    /// to it, so a changed share needs no second decode of the file (DJP-MEDIA-004).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_gain_db: Option<f64>,
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
    /// The tags the import read, so the browser can list a collection by artist, album and title
    /// (DJP-MEDIA-004). An older record has none: the file name still says most of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<FileTags>,
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
            auto_gain_db: None,
            key: None,
            rating: 0,
            notes: None,
            analysis_lock: false,
            variant: None,
            tags: None,
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{Anchor, BeatGrid};

    fn grid(bpm: f64, anchors: Vec<Anchor>) -> BeatGrid {
        BeatGrid {
            bpm,
            first_beat_frame: 0,
            anchors,
            beats_per_bar: 4,
        }
    }

    fn anchor(frame: u64, bpm: Option<f64>) -> Anchor {
        Anchor {
            frame,
            beat: frame / 22_500,
            bpm,
        }
    }

    #[test]
    fn a_track_says_which_way_its_tempo_moves() {
        let grid_of = |bpm: f64, anchors: Vec<Anchor>| grid(bpm, anchors);
        assert_eq!(
            grid_of(128.0, vec![]).bpm_trend(),
            "",
            "one stretch is one tempo, and wears nothing"
        );
        assert_eq!(
            grid_of(128.0, vec![anchor(960_000, Some(131.0))]).bpm_trend(),
            "",
            "131 against 128 is 2,3 % — one tempo to an ear, so no symbol either"
        );
        assert_eq!(
            grid_of(128.0, vec![anchor(960_000, Some(132.0))]).bpm_trend(),
            "\u{2197}",
            "132 against 128 is 3,125 %: faster, and the list says so with the arrow"
        );
        assert_eq!(
            grid_of(128.0, vec![anchor(960_000, Some(120.0))]).bpm_trend(),
            "\u{2198}",
            "slower sections count the same as faster ones, and the arrow points down"
        );
        assert_eq!(
            grid_of(
                128.0,
                vec![anchor(480_000, Some(132.0)), anchor(960_000, Some(140.0))]
            )
            .bpm_trend(),
            "\u{2197}",
            "faster and faster: one arrow up"
        );
        assert_eq!(
            grid_of(
                128.0,
                vec![anchor(480_000, Some(136.0)), anchor(960_000, Some(128.0))]
            )
            .bpm_trend(),
            " ~",
            "up and back down is a tempo that wanders, not one that rises"
        );
        assert_eq!(
            grid_of(128.0, vec![anchor(960_000, None)]).bpm_trend(),
            "",
            "a file written before anchors carried a tempo cannot be told apart from a single-tempo one"
        );
    }
}
