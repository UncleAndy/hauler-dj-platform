//! The session document: what a set-up looks like when the application is closed (DJP-SET-002).
//!
//! Everything here is a user decision, not audio data: no samples are copied into the document, only
//! paths and numbers. Times are frames of the source file, as everywhere else (DJP-DATA-002).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Schema name written into every document.
pub const SCHEMA: &str = "djp-session-v1";
/// Version of that schema. A reader refuses what is newer and migrates what is older.
pub const VERSION: u32 = 1;

/// The audio device and block the session expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    /// Device name as the backend reported it, when the user pinned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Sample rate.
    #[serde(default = "default_rate")]
    pub sample_rate: u32,
    /// Frames per block.
    #[serde(default = "default_block")]
    pub block: u32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: default_rate(),
            block: default_block(),
        }
    }
}

fn default_rate() -> u32 {
    48_000
}

fn default_block() -> u32 {
    256
}

/// The mixer as it was left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixerSettings {
    #[serde(default = "default_curve")]
    pub crossfader_curve: String,
    #[serde(default)]
    pub crossfader: f32,
    #[serde(default = "unity")]
    pub master_gain: f32,
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self {
            crossfader_curve: default_curve(),
            crossfader: 0.0,
            master_gain: unity(),
        }
    }
}

fn default_curve() -> String {
    "constant_power".to_owned()
}

fn unity() -> f32 {
    1.0
}

/// One deck: which track, where, and how it was trimmed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeckState {
    /// Track path, when a track was loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// Playhead in frames of the source file.
    #[serde(default)]
    pub position_frames: u64,
    /// Channel gain, linear.
    #[serde(default = "unity")]
    pub gain: f32,
}

impl Default for DeckState {
    fn default() -> Self {
        Self {
            track: None,
            position_frames: 0,
            gain: unity(),
        }
    }
}

/// The whole saved state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDoc {
    pub schema: String,
    pub version: u32,
    /// When the document was last written, milliseconds since the epoch.
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub mixer: MixerSettings,
    /// Deck id (`A`…`D`) to its state.
    #[serde(default)]
    pub decks: BTreeMap<String, DeckState>,
    /// Fields written by a newer build are carried through: an older one must not erase them.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for SessionDoc {
    fn default() -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            version: VERSION,
            updated_at_ms: 0,
            audio: AudioSettings::default(),
            mixer: MixerSettings::default(),
            decks: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl SessionDoc {
    /// A fresh document, stamped with the write time.
    pub fn new(now_ms: u64) -> Self {
        Self {
            updated_at_ms: now_ms,
            ..Self::default()
        }
    }
}
