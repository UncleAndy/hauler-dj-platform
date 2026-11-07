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
    /// Fields written by a newer build are carried through, not dropped: an old version must not
    /// erase what it does not understand.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
            cues: Vec::new(),
            beatgrid: None,
            loops: Vec::new(),
            gain: unity_gain(),
            key: None,
            rating: 0,
            notes: None,
            analysis_lock: false,
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
