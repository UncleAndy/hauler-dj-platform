//! Settings: the critical values, as parameters.
//!
//! Every number in here was measured, and the default is the value the measurements justified
//! (ADR-0008, ADR-0009, ADR-0010). They live in a file rather than in the code because the next
//! test is allowed to disagree: a machine with a different device, a bigger buffer need, or a
//! hungrier desktop must be able to change a number without a rebuild.
//!
//! Missing fields take their default, unknown fields are rejected — a typo in a settings file that
//! is silently ignored is worse than an error.
//!
//! Owner: architect (settings surface lands in A08/A10).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Version of the settings layout, so a migration can recognise old files.
pub const CONFIG_VERSION: u32 = 1;

/// Everything the application can be told to do differently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Layout version.
    pub version: u32,
    /// Output device and block size.
    pub audio: AudioSettings,
    /// Loop playback (ADR-0009).
    pub loop_playback: LoopSettings,
    /// PCM cache policy (ADR-0008).
    pub pcm_cache: CacheSettings,
    /// Measurement thresholds: the lines that decide what counts as a fault.
    pub diagnostics: DiagnosticsSettings,
    /// Real-time scheduling expectation (ADR-0010).
    pub realtime: RealtimeSettings,
    /// How often the picture is refreshed.
    pub display: DisplaySettings,
    /// Level matching of the files a deck opens.
    pub analysis: AnalysisSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            audio: AudioSettings::default(),
            loop_playback: LoopSettings::default(),
            pcm_cache: CacheSettings::default(),
            diagnostics: DiagnosticsSettings::default(),
            realtime: RealtimeSettings::default(),
            display: DisplaySettings::default(),
            analysis: AnalysisSettings::default(),
        }
    }
}

/// The level a matched track stands at, unless someone asks for another: seven tenths of the matched
/// level, about three decibels down. Chosen by ear — nine tenths still put the meters in the red on the
/// owner's loudest files, and the peaks are what the meters show, not the RMS that was matched.
pub const DEFAULT_AUTO_GAIN_SHARE: f32 = 0.7;

/// Level matching of the files a deck opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnalysisSettings {
    /// Share of the matched level the channel then stands at, 0…1.
    ///
    /// A file is trimmed to its target RMS, and the peaks travel with it: the meters go red on material
    /// whose RMS says it should not. Seven tenths puts the matched level about three decibels lower,
    /// which keeps the peaks out of the limiter; 1,0 is the matched level itself, 0,0 is silence.
    pub auto_gain_share: f32,
}

impl AnalysisSettings {
    /// The share in force, clamped: a settings file cannot invert or exaggerate a gain.
    pub fn share(&self) -> f32 {
        if self.auto_gain_share.is_finite() {
            self.auto_gain_share.clamp(0.0, 1.0)
        } else {
            DEFAULT_AUTO_GAIN_SHARE
        }
    }
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            auto_gain_share: DEFAULT_AUTO_GAIN_SHARE,
        }
    }
}

/// Output device and block size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioSettings {
    /// Sample rate to request; the device may impose its own.
    pub sample_rate: u32,
    /// Frames per callback. 256 at 48 kHz is a 5.33 ms period.
    pub block_frames: usize,
    /// Device name substring; `None` means the system default output.
    pub device: Option<String>,
    /// Prefetch window per deck, seconds.
    pub prefetch_seconds: f64,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            block_frames: 256,
            device: None,
            prefetch_seconds: 2.0,
        }
    }
}

/// Loop playback (ADR-0009).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoopSettings {
    /// Longest loop captured into memory, seconds. Zero disables capture.
    pub buffer_max_seconds: f64,
}

impl Default for LoopSettings {
    fn default() -> Self {
        Self {
            buffer_max_seconds: 30.0,
        }
    }
}

/// Where cached PCM lives and how much of it (ADR-0008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheStorage {
    /// On disk, surviving restarts.
    Disk,
    /// In memory, gone when the process exits.
    Memory,
}

/// Sample format of the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheSampleFormat {
    /// 16-bit integer.
    I16,
    /// 24-bit integer.
    I24,
    /// 32-bit float.
    F32,
}

/// PCM cache policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CacheSettings {
    /// Cache lossy formats (ADR-0008); lossless always streams.
    pub enabled: bool,
    /// Disk or memory.
    pub storage: CacheStorage,
    /// Budget in gigabytes.
    pub max_size_gb: f64,
    /// Sample format written to the cache.
    pub sample_format: CacheSampleFormat,
    /// Files longer than this get the first seconds ready before the rest is built.
    pub long_file_minutes: f64,
    /// How many seconds count as "ready" for a long file.
    pub long_file_ready_seconds: f64,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            storage: CacheStorage::Disk,
            max_size_gb: 8.0,
            sample_format: CacheSampleFormat::I24,
            long_file_minutes: 15.0,
            long_file_ready_seconds: 60.0,
        }
    }
}

/// The lines that decide what counts as a fault.
///
/// These are not preferences: they are the thresholds the verdict is read against, which is why
/// they belong in a file next to the run that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticsSettings {
    /// A callback interval above this many periods counts as late.
    pub late_threshold_periods: f64,
    /// Below this many periods counts as early.
    pub early_threshold_periods: f64,
    /// A block with at least this long off the CPU counts as stalled.
    pub stall_threshold_us: u32,
    /// A block costing more than this share of the period is recorded as expensive.
    pub expensive_threshold_percent: f64,
}

impl Default for DiagnosticsSettings {
    fn default() -> Self {
        Self {
            late_threshold_periods: 1.5,
            early_threshold_periods: 0.5,
            stall_threshold_us: 500,
            expensive_threshold_percent: 1.0,
        }
    }
}

/// What the application expects of the scheduler, and what it should do when it does not get it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RealtimeSettings {
    /// Priority the packaging is expected to grant.
    pub priority: u8,
    /// Fail loudly instead of warning when the thread is not real-time.
    pub require: bool,
}

impl Default for RealtimeSettings {
    fn default() -> Self {
        Self {
            priority: 20,
            require: false,
        }
    }
}

/// How often state leaves the engine and how often the screen is redrawn.
///
/// Two numbers, not one, because they answer different questions. The engine publishes whole
/// snapshots — every deck, every counter — and twenty a second is enough for the numbers. The strip
/// under the playhead moves between those snapshots: the UI projects the position from the track's
/// own sample rate and the current tempo, so a frame rate above the state rate still buys smooth
/// motion. Twenty frames a second is the default: it is cheaper on a booth laptop and the projection
/// keeps the movement continuous. Raising `frame_hz` towards the display rate makes a fast scroll at
/// deep zoom smoother; raising `state_hz` does it by sending more truth instead of projecting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplaySettings {
    /// Snapshots published per second.
    pub state_hz: u32,
    /// Frames drawn per second.
    pub frame_hz: u32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            state_hz: 20,
            frame_hz: 20,
        }
    }
}

/// Why a settings file was rejected.
#[derive(Debug)]
pub enum ConfigError {
    /// The file could not be read.
    Io(String),
    /// The file is not valid for this layout.
    Parse(String),
    /// A value is outside the range the application can honour.
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(detail) => write!(f, "cannot read settings: {detail}"),
            ConfigError::Parse(detail) => write!(f, "cannot parse settings: {detail}"),
            ConfigError::Invalid(detail) => write!(f, "invalid setting: {detail}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parse settings from TOML, then validate them.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(text).map_err(|err| ConfigError::Parse(err.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Read settings from a file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| ConfigError::Io(format!("{}: {err}", path.display())))?;
        Self::from_toml(&text)
    }

    /// Serialise back to TOML, which is what `--write-config` produces.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|err| ConfigError::Parse(err.to_string()))
    }

    /// Reject values the audio path cannot honour.
    ///
    /// Ranges are deliberately generous: this catches typos and nonsense (a block of zero, a rate
    /// of 1 Hz), not opinions about good taste.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::Invalid(format!(
                "version {} is not {CONFIG_VERSION}; write a new settings file",
                self.version
            )));
        }
        if !(8_000..=384_000).contains(&self.audio.sample_rate) {
            return Err(ConfigError::Invalid(format!(
                "audio.sample_rate {} is outside 8000..=384000",
                self.audio.sample_rate
            )));
        }
        if !(32..=8_192).contains(&self.audio.block_frames) {
            return Err(ConfigError::Invalid(format!(
                "audio.block_frames {} is outside 32..=8192",
                self.audio.block_frames
            )));
        }
        if !self.audio.block_frames.is_power_of_two() {
            return Err(ConfigError::Invalid(format!(
                "audio.block_frames {} is not a power of two",
                self.audio.block_frames
            )));
        }
        if !(0.1..=10.0).contains(&self.audio.prefetch_seconds) {
            return Err(ConfigError::Invalid(format!(
                "audio.prefetch_seconds {} is outside 0.1..=10",
                self.audio.prefetch_seconds
            )));
        }
        if !(0.0..=600.0).contains(&self.loop_playback.buffer_max_seconds) {
            return Err(ConfigError::Invalid(format!(
                "loop_playback.buffer_max_seconds {} is outside 0..=600",
                self.loop_playback.buffer_max_seconds
            )));
        }
        if !(0.0..=10_000.0).contains(&self.pcm_cache.max_size_gb) {
            return Err(ConfigError::Invalid(format!(
                "pcm_cache.max_size_gb {} is outside 0..=10000",
                self.pcm_cache.max_size_gb
            )));
        }
        if !(1..=120).contains(&self.display.state_hz) {
            return Err(ConfigError::Invalid(format!(
                "display.state_hz {} is outside 1..=120",
                self.display.state_hz
            )));
        }
        if !(5..=240).contains(&self.display.frame_hz) {
            return Err(ConfigError::Invalid(format!(
                "display.frame_hz {} is outside 5..=240",
                self.display.frame_hz
            )));
        }
        let diagnostics = &self.diagnostics;
        if diagnostics.late_threshold_periods <= diagnostics.early_threshold_periods {
            return Err(ConfigError::Invalid(
                "diagnostics.late_threshold_periods must be above early_threshold_periods"
                    .to_string(),
            ));
        }
        if !(0.1..=10.0).contains(&diagnostics.late_threshold_periods) {
            return Err(ConfigError::Invalid(format!(
                "diagnostics.late_threshold_periods {} is outside 0.1..=10",
                diagnostics.late_threshold_periods
            )));
        }
        if !(1..=10_000).contains(&diagnostics.stall_threshold_us) {
            return Err(ConfigError::Invalid(format!(
                "diagnostics.stall_threshold_us {} is outside 1..=10000",
                diagnostics.stall_threshold_us
            )));
        }
        if !(0.01..=100.0).contains(&diagnostics.expensive_threshold_percent) {
            return Err(ConfigError::Invalid(format!(
                "diagnostics.expensive_threshold_percent {} is outside 0.01..=100",
                diagnostics.expensive_threshold_percent
            )));
        }
        if !(1..=95).contains(&self.realtime.priority) {
            return Err(ConfigError::Invalid(format!(
                "realtime.priority {} is outside 1..=95",
                self.realtime.priority
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_values_the_measurements_justified() {
        let config = Config::default();
        // ADR-0009: 30 s of loop captured; measured 899 wraps with zero gaps.
        assert_eq!(config.loop_playback.buffer_max_seconds, 30.0);
        // ADR-0008: 8 GB cache, disk, i24, 2 s prefetch, 15 min long-file threshold.
        assert_eq!(config.pcm_cache.max_size_gb, 8.0);
        assert_eq!(config.pcm_cache.storage, CacheStorage::Disk);
        assert_eq!(config.pcm_cache.sample_format, CacheSampleFormat::I24);
        assert_eq!(config.audio.prefetch_seconds, 2.0);
        assert_eq!(config.pcm_cache.long_file_minutes, 15.0);
        assert_eq!(config.pcm_cache.long_file_ready_seconds, 60.0);
        // A05: 48 kHz, 256 frames, system default device.
        assert_eq!(config.audio.sample_rate, 48_000);
        assert_eq!(config.audio.block_frames, 256);
        assert_eq!(config.audio.device, None);
        // ADR-0010: packaging grants priority 20 and the application only warns.
        assert_eq!(config.realtime.priority, 20);
        assert!(!config.realtime.require);
        // Thresholds the verdict is read against.
        assert_eq!(config.diagnostics.late_threshold_periods, 1.5);
        assert_eq!(config.diagnostics.early_threshold_periods, 0.5);
        assert_eq!(config.diagnostics.stall_threshold_us, 500);
        assert_eq!(config.diagnostics.expensive_threshold_percent, 1.0);
        // The owner asked for twenty frames a second as the default, and for both rates to be
        // parameters rather than constants.
        assert_eq!(config.display.state_hz, 20);
        assert_eq!(config.display.frame_hz, 20);
        config.validate().unwrap_or_else(|err| panic!("{err}"));
    }

    #[test]
    fn missing_fields_take_defaults_and_unknown_ones_are_rejected() {
        let partial = Config::from_toml("[audio]\nblock_frames = 512\n")
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(partial.audio.block_frames, 512);
        assert_eq!(
            partial.audio.sample_rate, 48_000,
            "untouched field keeps its default"
        );

        let typo = Config::from_toml("[audio]\nblock_frame = 512\n");
        assert!(
            matches!(typo, Err(ConfigError::Parse(_))),
            "a misspelled key must fail, not be ignored"
        );
    }

    #[test]
    fn nonsense_is_rejected_with_the_field_named() {
        let zero_block = Config::from_toml("[audio]\nblock_frames = 0\n");
        assert!(
            matches!(zero_block, Err(ConfigError::Invalid(text)) if text.contains("block_frames")),
            "a zero block must name the field"
        );

        let odd_block = Config::from_toml("[audio]\nblock_frames = 300\n");
        assert!(matches!(odd_block, Err(ConfigError::Invalid(_))));

        let inverted = Config::from_toml(
            "[diagnostics]\nlate_threshold_periods = 0.4\nearly_threshold_periods = 0.5\n",
        );
        assert!(matches!(inverted, Err(ConfigError::Invalid(_))));

        let rate = Config::from_toml("[audio]\nsample_rate = 1\n");
        assert!(matches!(rate, Err(ConfigError::Invalid(_))));

        let frames = Config::from_toml("[display]\nframe_hz = 0\n");
        assert!(
            matches!(frames, Err(ConfigError::Invalid(text)) if text.contains("frame_hz")),
            "a zero frame rate must name the field"
        );

        let states = Config::from_toml("[display]\nstate_hz = 500\n");
        assert!(matches!(states, Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let mut config = Config::default();
        config.audio.block_frames = 512;
        config.audio.device = Some("s4".to_string());
        config.loop_playback.buffer_max_seconds = 4.0;
        config.diagnostics.late_threshold_periods = 1.25;
        config.display.state_hz = 40;
        config.display.frame_hz = 60;

        let text = config.to_toml().unwrap_or_else(|err| panic!("{err}"));
        let back = Config::from_toml(&text).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(config, back);
    }
}
