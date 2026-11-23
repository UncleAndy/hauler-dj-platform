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

use std::path::{Path, PathBuf};

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
    /// Collection and the history of plays.
    pub library: LibrarySettings,
    /// Whether a DJ controller is used at all (ADR-0006).
    pub controller: ControllerSettings,
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
            library: LibrarySettings::default(),
            controller: ControllerSettings::default(),
        }
    }
}

/// Whether a controller is used, and which driver reads it (ADR-0006).
///
/// Off by default, and that is a promise rather than a style: with `enabled = false` the program is exactly the
/// mouse-and-keyboard one it was before controllers existed — no process is started, no HID node is opened, and no
/// branch in the audio or the picture changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControllerSettings {
    /// Whether a controller is used at all.
    pub enabled: bool,
    /// Which driver to run when one is enabled. `none` means "enabled but nothing to read", which is said out loud in
    /// the log instead of being quietly ignored.
    pub driver: ControllerDriver,
    /// The HID node to open, when the driver should not search by itself.
    pub device: Option<String>,
}

impl Default for ControllerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            driver: ControllerDriver::None,
            device: None,
        }
    }
}

/// The drivers this build knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControllerDriver {
    /// Nothing: the setting is on but no device is claimed.
    #[default]
    None,
    /// Native Instruments Traktor Kontrol S4 MK3.
    #[serde(rename = "s4mk3")]
    S4Mk3,
}

/// Collection and history settings (DJP-MEDIA-004).
///
/// A play is only recorded when the deck played long enough to count as listening, and a track stays
/// "played recently" for as long as the DJ says. Both are the owner's numbers, so both live here
/// instead of in the code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LibrarySettings {
    /// Shortest play that counts as listening, in seconds.
    pub history_min_seconds: u32,
    /// How long a played track stays "recently played", in hours.
    pub played_recently_hours: u32,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            history_min_seconds: 30,
            played_recently_hours: 4,
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

/// Where the headphones are and what they hear (owner's decisions, 20.11).
///
/// Monitoring always has an output of its own: a headphone signal mixed into the main output would be heard by the
/// room, which is the one thing monitoring exists to avoid. So the choice is only *which* output it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadphoneMode {
    /// Four or more channels on the card: the pair beside the main one. Otherwise nothing, and the UI says so.
    Auto,
    /// A named pair of channels on the same card: the two stereo pairs a DJ controller carries.
    SameDevice,
    /// Another device altogether, opened as a second stream.
    Device,
}

/// Serde's `default` for a switch that is on unless the file says otherwise.
fn yes() -> bool {
    true
}

/// A channel pair, written either as an array or as the text of one.
///
/// The engine only ever writes `[0, 1]`; the shell wrote `"[0, 1]"` for one afternoon, and a file with that in it has to
/// keep working rather than stop the engine at start (owner's report, 21.11).
fn pair_or_text<'de, D>(deserializer: D) -> Result<Option<(usize, usize)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Pair((usize, usize)),
        Text(String),
    }
    let value = Option::<Either>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(Either::Pair(pair)) => Ok(Some(pair)),
        Some(Either::Text(text)) => {
            let body = text.trim();
            let body = body
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
                .ok_or_else(|| serde::de::Error::custom(format!("{text} is not a channel pair")))?;
            let mut numbers = body.split(',').map(|part| part.trim().parse::<usize>());
            match (numbers.next(), numbers.next()) {
                (Some(Ok(left)), Some(Ok(right))) => Ok(Some((left, right))),
                _ => Err(serde::de::Error::custom(format!(
                    "{text} is not a channel pair"
                ))),
            }
        }
    }
}

/// Headphone output: mode, pair, device, and the two knobs the mixer shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HeadphoneSettings {
    /// Which of the three arrangements is in force.
    pub mode: HeadphoneMode,
    /// Device name substring for [`HeadphoneMode::Device`]; `None` means the system default.
    pub device: Option<String>,
    /// Channel pair for [`HeadphoneMode::SameDevice`]; `None` lets the engine take the first free pair.
    ///
    /// Both `[0, 1]` and the quoted `"[0, 1]"` are read: a build from before 21.11 wrote the string form, and a settings
    /// file an older version wrote must not stop the program from starting.
    #[serde(default, deserialize_with = "pair_or_text")]
    pub pair: Option<(usize, usize)>,
    /// Whether the cue leaves the engine at all.
    ///
    /// `false` is the "None" the settings window shows when the device has no spare pair: the cue is mixed but goes
    /// nowhere, which is honest — monitoring that cannot exist must not pretend to (owner's decision, 21.11).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Volume of the headphones, linear: 1.0 is unity.
    pub gain: f32,
    /// Proportion between monitoring and the main mix, 0.0 … 1.0: 1.0 is monitoring only.
    pub mix: f32,
}

impl Default for HeadphoneSettings {
    fn default() -> Self {
        // Monitoring starts open and on the monitoring side: a DJ puts the headphones on to hear the cued deck,
        // and the mix knob is there to bring the main mix in, not the other way round.
        Self {
            mode: HeadphoneMode::Auto,
            device: None,
            pair: None,
            enabled: true,
            gain: 1.0,
            mix: 1.0,
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
    /// Which channels carry the main output: `[0, 1]` by default, and `[1, 0]` swaps the stereo sides.
    ///
    /// An interface is a row of sockets and its numbering is the driver's; a booth whose monitors are wired the other
    /// way round needs a setting, not a crossed cable (owner's decision, 21.11). `None` keeps the route plan's pair.
    #[serde(default)]
    pub master: Option<(usize, usize)>,
    /// Headphones: where they are and what they hear.
    pub headphones: HeadphoneSettings,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            block_frames: 256,
            device: None,
            prefetch_seconds: 2.0,
            master: None,
            headphones: HeadphoneSettings::default(),
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
    /// Memory, in gigabytes, that the engine may spend holding whole tracks.
    ///
    /// This is a different budget from `max_size_gb`: that one is the decoder's cache on disk, this one
    /// is playback material kept in RAM so the read path cannot be late. One gigabyte holds four
    /// ten-minute tracks at 48 kHz in stereo f32 (922 MB); zero turns the whole-track cache off.
    pub whole_track_gb: f64,
    /// Whether the whole-track cache also takes lossless files (WAV, FLAC).
    ///
    /// Lossy formats are cached by policy anyway; lossless costs a second decode and buys a read path
    /// that never touches the disk. Off by default would leave most DJ libraries uncached, so it is on.
    pub lossless: bool,
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
            whole_track_gb: 1.0,
            lossless: true,
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
    /// How much the logs say: `error`, `warn`, `info`, `debug` or `trace`.
    ///
    /// `None` keeps the engine's own default; a value here is used only when `HAULER_LOG` is not set, so a shell that
    /// sets the variable for one run still wins over the file.
    #[serde(default)]
    pub log_level: Option<String>,
}

impl Default for DiagnosticsSettings {
    fn default() -> Self {
        Self {
            late_threshold_periods: 1.5,
            early_threshold_periods: 0.5,
            stall_threshold_us: 500,
            expensive_threshold_percent: 1.0,
            log_level: None,
        }
    }
}

impl DiagnosticsSettings {
    /// The level the file asks for, when it names one the logger understands.
    ///
    /// A typo must not silence the log: an unknown word reads as "nothing was asked for", and the engine keeps its
    /// own default.
    pub fn level(&self) -> Option<&str> {
        let level = self.log_level.as_deref()?;
        match level {
            "error" | "warn" | "info" | "debug" | "trace" => Some(level),
            _ => None,
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

/// Change one value of a settings file, leaving everything else exactly as it stood.
///
/// The settings file is written by hand as much as by the shell: it carries comments that explain why a number is
/// what it is, and keys a newer build added. Re-serialising the typed [`Config`] would keep the values and lose both,
/// so the file is edited as a document: the key is looked up by its dotted path (`audio.block_frames`), the value is
/// replaced where it stands, and a key that is not there yet is created inside its table.
///
/// `value` is TOML text on purpose — the caller writes `512`, `"s4"` or `true` and the editor parses it, so a typed
/// wrapper cannot silently write an integer where the file wants a string.
pub fn set_value(path: &std::path::Path, key: &str, value: &str) -> Result<(), ConfigError> {
    let parsed: toml_edit::Value = value
        .parse()
        .map_err(|_| ConfigError::Invalid(format!("{value} is not a value TOML understands")))?;
    write_value(path, key, parsed)
}

/// Set one value that is a string, taken literally.
///
/// The shell has a QString and no business knowing how TOML quotes and escapes one: this takes the text as it is and
/// lets the document format it, so a device name with a quote in it cannot break the file.
pub fn set_string(path: &std::path::Path, key: &str, text: &str) -> Result<(), ConfigError> {
    write_value(path, key, toml_edit::Value::from(text))
}

fn write_value(
    path: &std::path::Path,
    key: &str,
    value: toml_edit::Value,
) -> Result<(), ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(ConfigError::Io(error.to_string())),
    };
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| ConfigError::Parse(error.to_string()))?;

    let parts: Vec<&str> = key.split('.').collect();
    let (leaf, tables) = match parts.split_last() {
        Some(pair) => pair,
        None => {
            return Err(ConfigError::Invalid(String::from(
                "an empty key names nothing",
            )));
        }
    };
    let mut table = document.as_table_mut();
    for name in tables {
        if !table.contains_key(name) {
            table.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        table = table
            .get_mut(name)
            .and_then(|item| item.as_table_mut())
            .ok_or_else(|| ConfigError::Invalid(format!("{name} is not a table in {key}")))?;
    }
    table.insert(leaf, toml_edit::Item::Value(value));

    // A settings file may not exist yet — that is the normal state of a fresh install, and the first change the
    // window makes is what creates it.
    if let Some(folder) = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        std::fs::create_dir_all(folder).map_err(|error| ConfigError::Io(error.to_string()))?;
    }
    std::fs::write(path, document.to_string()).map_err(|error| ConfigError::Io(error.to_string()))
}

/// Read one value of a settings file as TOML text, for a shell that shows what the engine will read.
pub fn get_value(path: &std::path::Path, key: &str) -> Result<Option<String>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ConfigError::Io(error.to_string())),
    };
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| ConfigError::Parse(error.to_string()))?;
    let mut item: &toml_edit::Item = document.as_item();
    for name in key.split('.') {
        let Some(next) = item.get(name) else {
            return Ok(None);
        };
        item = next;
    }
    Ok(Some(item.to_string().trim().to_string()))
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

    /// Where the settings live unless a flag says otherwise: `$XDG_CONFIG_HOME/hauler-dj/config.toml`,
    /// or the same under the home folder when the variable is unset.
    pub fn default_path() -> Option<PathBuf> {
        let base = dj_paths::config_dir()?;
        Some(base.join("hauler-dj").join("config.toml"))
    }

    /// The settings in force for a run: the file the caller named, else the standard one if it is there,
    /// else the defaults. The answer carries the path the numbers came from, because a run that cannot say
    /// where its thresholds came from is a run nobody can argue with.
    ///
    /// A file that is there but cannot be read is an error, not a quiet fallback: an unknown key or a typo
    /// must not look like an applied setting, and a run judged by numbers nobody chose is worse than a run
    /// that refuses to start. A *named* file that is not there is an error too; the standard path being
    /// absent is simply a machine that never wrote one.
    pub fn load_or_default(named: Option<&Path>) -> Result<(Self, Option<PathBuf>), ConfigError> {
        match named {
            Some(path) => Ok((Self::load(path)?, Some(path.to_path_buf()))),
            None => match Self::default_path() {
                Some(path) if path.exists() => Ok((Self::load(&path)?, Some(path))),
                _ => Ok((Self::default(), None)),
            },
        }
    }

    /// Read settings from a file. The path is in every message this can produce: a run that refuses to
    /// start has to say *which* file it refused, or the DJ is left guessing between three of them.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| ConfigError::Io(format!("{}: {err}", path.display())))?;
        Self::from_toml(&text)
            .map_err(|err| ConfigError::Parse(format!("{}: {err}", path.display())))
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
        let headphones = &self.audio.headphones;
        if !headphones.gain.is_finite() || !(0.0..=2.0).contains(&headphones.gain) {
            return Err(ConfigError::Invalid(format!(
                "audio.headphones.gain {} is outside 0..=2",
                headphones.gain
            )));
        }
        if !headphones.mix.is_finite() || !(0.0..=1.0).contains(&headphones.mix) {
            return Err(ConfigError::Invalid(format!(
                "audio.headphones.mix {} is outside 0..=1",
                headphones.mix
            )));
        }
        if let Some((left, right)) = headphones.pair
            && left == right
        {
            return Err(ConfigError::Invalid(format!(
                "audio.headphones.pair ({left}, {right}) writes both sides to one channel"
            )));
        }
        if !(0.0..=600.0).contains(&self.loop_playback.buffer_max_seconds) {
            return Err(ConfigError::Invalid(format!(
                "loop_playback.buffer_max_seconds {} is outside 0..=600",
                self.loop_playback.buffer_max_seconds
            )));
        }
        if !(0.0..=64.0).contains(&self.pcm_cache.whole_track_gb) {
            return Err(ConfigError::Invalid(format!(
                "pcm_cache.whole_track_gb {} is outside 0..=64",
                self.pcm_cache.whole_track_gb
            )));
        }
        if !(0.0..=10_000.0).contains(&self.pcm_cache.max_size_gb) {
            return Err(ConfigError::Invalid(format!(
                "pcm_cache.max_size_gb {} is outside 0..=10000",
                self.pcm_cache.max_size_gb
            )));
        }
        if !(1..=3600).contains(&self.library.history_min_seconds) {
            return Err(ConfigError::Invalid(format!(
                "library.history_min_seconds {} is outside 1..=3600",
                self.library.history_min_seconds
            )));
        }
        if !(1..=168).contains(&self.library.played_recently_hours) {
            return Err(ConfigError::Invalid(format!(
                "library.played_recently_hours {} is outside 1..=168",
                self.library.played_recently_hours
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

/// Which output device the engine should ask for, given the command line and the settings file.
///
/// Three callers say three different things, and the order between them is the whole point:
///
/// - an explicit name (`--device ur22c`) is what the person typed just now, so it wins;
/// - an empty name is the launcher's placeholder — it does not know any device, so the settings file answers, and an
///   empty answer asks for the system's own choice;
/// - no name at all means the caller never asked for sound, and the engine keeps its silent null sink. A gate that
///   measures the mixer must not open a sound card just because a settings file names one.
///
/// Whitespace is not a name in either place: a file with `device = "  "` is a file with no device, not a search for a
/// device whose name contains spaces (an empty needle matches every device — the first one wins, and that is how the
/// controller chosen in the window ended up unplugged from the sound).
/// Whether the two headphone knobs belong to the hardware rather than to the window.
///
/// The S4 MK3 is an audio interface as well as a controller: its headphone volume and its cue/master proportion are
/// physical knobs, and the hardware never reports their positions to us. A second stage for the same job on screen
/// would be a knob that lies — it would move, the sound would change by a few decibels, and the DJ would have two
/// places to look for the same thing (owner's instruction, 23.11: "в режиме работы через контроллер не нужно ручками
/// в приложении регулировать громкость наушников и степень микса").
pub fn hardware_owns_cue(controller: &crate::ControllerSettings) -> bool {
    controller.enabled && controller.driver == ControllerDriver::S4Mk3
}

pub fn chosen_output_device(cli: Option<&str>, configured: Option<&str>) -> Option<String> {
    let configured = configured
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let requested = cli?;
    let requested = requested.trim();
    if requested.is_empty() {
        Some(configured.unwrap_or_default())
    } else {
        Some(requested.to_string())
    }
}

#[cfg(test)]
mod tests {
    /// A controller that switches itself on would change the program for every existing user.
    #[test]
    fn a_controller_is_off_until_the_file_says_otherwise() {
        let config = Config::default();
        assert!(!config.controller.enabled);
        assert_eq!(config.controller.driver, ControllerDriver::None);
        assert!(config.controller.device.is_none());
    }

    /// The setting has to survive a round trip through the file, because the window writes it and the service reads it.
    #[test]
    fn the_controller_section_is_written_and_read_back() {
        let file = toml::to_string(&Config::default()).unwrap_or_default();
        assert!(file.contains("[controller]"), "{file}");
        assert!(file.contains("enabled = false"), "{file}");
        let parsed: Config = toml::from_str(&file).unwrap_or_default();
        assert_eq!(parsed.controller, Config::default().controller);

        let turned_on = Config {
            controller: ControllerSettings {
                enabled: true,
                driver: ControllerDriver::S4Mk3,
                device: None,
            },
            ..Config::default()
        };
        let text = toml::to_string(&turned_on).unwrap_or_default();
        let back: Config = toml::from_str(&text).unwrap_or_default();
        assert!(back.controller.enabled);
        assert_eq!(back.controller.driver, ControllerDriver::S4Mk3);
        assert!(text.contains("driver = \"s4mk3\""), "{text}");
    }

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
    fn headphones_are_settings_not_constants() {
        // Owner's decisions of 20.11: monitoring is its own output, either another channel pair of the same
        // card or another device. Nothing about it is a constant in the code.
        let defaults = Config::default();
        assert_eq!(defaults.audio.headphones.mode, HeadphoneMode::Auto);
        assert_eq!(defaults.audio.headphones.gain, 1.0);
        assert_eq!(defaults.audio.headphones.mix, 1.0);
        assert!(defaults.audio.headphones.pair.is_none());
        assert!(defaults.audio.headphones.device.is_none());

        // What the owner set comes back unchanged.
        let text =
            "[audio.headphones]\nmode = \"same_device\"\npair = [2, 3]\ngain = 0.75\nmix = 0.4\n";
        let parsed: Config = toml::from_str(text).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(parsed.audio.headphones.mode, HeadphoneMode::SameDevice);
        assert_eq!(parsed.audio.headphones.pair, Some((2, 3)));
        assert_eq!(parsed.audio.headphones.gain, 0.75);
        assert_eq!(parsed.audio.headphones.mix, 0.4);

        // A file written before monitoring existed still loads: the section is missing, not required.
        let old: Config =
            toml::from_str("[audio]\nblock_frames = 512\n").unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(old.audio.block_frames, 512);
        assert_eq!(old.audio.headphones, HeadphoneSettings::default());
    }

    #[test]
    fn headphone_values_out_of_range_are_refused() {
        // A gain of NaN reaching the mixer is silence nobody can explain later, and a pair of the same channel
        // twice is a route that writes the same bus to one output.
        for (gain, mix, pair) in [
            (f32::NAN, 1.0, None),
            (2.5, 1.0, None),
            (1.0, 1.5, None),
            (1.0, 1.0, Some((3, 3))),
        ] {
            let mut config = Config::default();
            config.audio.headphones.gain = gain;
            config.audio.headphones.mix = mix;
            config.audio.headphones.pair = pair;
            assert!(
                config.validate().is_err(),
                "gain {gain}, mix {mix}, pair {pair:?} must be refused"
            );
        }
    }

    #[test]
    fn the_whole_track_budget_is_a_setting_and_its_default_holds_four_long_tracks() {
        // Four ten-minute tracks at 48 kHz stereo f32 are 921 600 000 bytes; the default budget must
        // hold them, and the file must be able to say otherwise.
        let defaults = Config::default();
        assert_eq!(defaults.pcm_cache.whole_track_gb, 1.0);
        assert!(defaults.pcm_cache.lossless, "lossless is cached by default");
        let needed_gb = 4.0 * 600.0 * 48_000.0 * 2.0 * 4.0 / 1e9;
        assert!(
            defaults.pcm_cache.whole_track_gb >= needed_gb,
            "the default budget {needed_gb} GB must fit four ten-minute tracks"
        );

        let smaller = Config::from_toml(
            &defaults
                .to_toml()
                .unwrap_or_else(|err| panic!("{err}"))
                .replace("whole_track_gb = 1.0", "whole_track_gb = 0.25"),
        )
        .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(smaller.pcm_cache.whole_track_gb, 0.25);

        let broken = Config::from_toml(
            &defaults
                .to_toml()
                .unwrap_or_else(|err| panic!("{err}"))
                .replace("whole_track_gb = 1.0", "whole_track_gb = 100.0"),
        );
        assert!(
            broken.is_err(),
            "a hundred gigabytes is a typo, not a setting"
        );
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
    #[allow(clippy::expect_used)]
    fn a_named_file_that_is_not_there_is_an_error() {
        let missing = std::env::temp_dir().join("dj-config-there-is-no-such-file.toml");
        std::fs::remove_file(&missing).ok();
        assert!(Config::load_or_default(Some(&missing)).is_err());
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn the_file_replaces_the_defaults_and_says_where_they_came_from() {
        let path = std::env::temp_dir().join(format!("dj-config-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "[library]\nhistory_min_seconds = 7\nplayed_recently_hours = 2\n[audio]\nblock_frames = 512\n",
        )
        .expect("write");
        let (config, used) = Config::load_or_default(Some(&path)).expect("load");
        assert_eq!(config.library.history_min_seconds, 7);
        assert_eq!(config.library.played_recently_hours, 2);
        assert_eq!(config.audio.block_frames, 512);
        assert_eq!(used.as_deref(), Some(path.as_path()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn a_broken_file_is_refused_with_its_name() {
        let path =
            std::env::temp_dir().join(format!("dj-config-broken-{}.toml", std::process::id()));
        std::fs::write(&path, "[library]\nhistory_min_seconds = \"soon\"\n").expect("write");
        let error = Config::load_or_default(Some(&path)).expect_err("a typo is an error");
        assert!(error.to_string().contains("dj-config-broken"), "{error}");
        std::fs::remove_file(&path).ok();
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
