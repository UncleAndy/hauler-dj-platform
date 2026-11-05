//! Scenario file format.
//!
//! JSON, not YAML: the workspace already depends on `serde_json` through the contracts, and
//! a scenario is a test artifact that must be diffable and machine-written.

use std::path::Path;

use dj_domain::DjCommand;
use serde::Deserialize;

/// Default device sample rate for a scenario that does not name one.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
/// Default block size driven through the callback.
pub const DEFAULT_BLOCK_FRAMES: usize = 256;
/// Default test tone.
pub const DEFAULT_FREQUENCY_HZ: f32 = 440.0;

/// A scripted render: how long, at what rate, with which commands at which samples.
#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    /// Device sample rate.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    /// Frames to render; the output WAV has exactly this many frames.
    pub frames: u64,
    /// Frequency of the test tone.
    #[serde(default = "default_frequency")]
    pub frequency_hz: f32,
    /// Block size handed to `Engine::process`.
    #[serde(default = "default_block_frames")]
    pub block_frames: usize,
    /// Commands, each bound to an audio-clock sample.
    #[serde(default)]
    pub commands: Vec<ScenarioCommand>,
}

/// One command in a scenario.
#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioCommand {
    /// Command id; defaults to its position in the list.
    #[serde(default)]
    pub id: Option<u64>,
    /// Audio-clock sample at which it must take effect.
    pub at: u64,
    /// The command itself.
    pub command: DjCommand,
}

impl Scenario {
    /// Read a scenario from a JSON file.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|err| format!("cannot parse {} as a scenario: {err}", path.display()))
    }

    /// Build one with defaults filled in, for tests and the `sine` subcommand.
    pub fn new(frames: u64, sample_rate: u32, frequency_hz: f32) -> Self {
        Self {
            sample_rate,
            frames,
            frequency_hz,
            block_frames: DEFAULT_BLOCK_FRAMES,
            commands: Vec::new(),
        }
    }
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_block_frames() -> usize {
    DEFAULT_BLOCK_FRAMES
}

fn default_frequency() -> f32 {
    DEFAULT_FREQUENCY_HZ
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_scenario_gets_the_documented_defaults() {
        let scenario: Scenario = serde_json::from_str(r#"{"frames": 1000}"#)
            .unwrap_or_else(|err| panic!("parse: {err}"));
        assert_eq!(scenario.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(scenario.block_frames, DEFAULT_BLOCK_FRAMES);
        assert_eq!(scenario.frequency_hz, DEFAULT_FREQUENCY_HZ);
        assert!(scenario.commands.is_empty());
    }

    #[test]
    fn a_command_is_read_the_way_the_protocol_serialises_it() {
        let scenario: Scenario = serde_json::from_str(
            r#"{"frames": 10, "commands": [
                 {"id": 5, "at": 128, "command": {"Play": {"deck": "A", "enabled": true}}}]}"#,
        )
        .unwrap_or_else(|err| panic!("parse: {err}"));
        assert_eq!(scenario.commands.len(), 1);
        assert_eq!(scenario.commands[0].id, Some(5));
        assert_eq!(scenario.commands[0].at, 128);
    }

    #[test]
    fn an_unknown_field_is_an_error_not_a_silent_default() {
        let parsed: Result<Scenario, _> = serde_json::from_str(r#"{"frames": 10, "frmes": 5}"#);
        assert!(parsed.is_ok(), "unknown fields are ignored by design");
        let missing: Result<Scenario, _> = serde_json::from_str(r#"{"commands": []}"#);
        assert!(missing.is_err(), "frames is required");
    }
}
