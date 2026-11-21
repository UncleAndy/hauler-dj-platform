//! The engine process: it owns the sound, the UI only asks.
//!
//! Everything a client needs is in [`dj_bridge`] and [`dj_domain`]; this binary is the loop that
//! ties them to real decks. Two modes:
//!
//! - `--null-sink` runs the whole graph against a generated tone and a clock, so the command path
//!   can be exercised without a sound card (CI, this sandbox, a headless server);
//! - `--device <substr>` opens the real output through `dj-audio-io`.
//!
//! Commands are applied at their `effective_sample`, clamped to the current audio clock: a client
//! that asks for the past gets now, never a silent no-op.
// A release build on Windows is a windowed program: a console subsystem binary is given a console window on
// startup, and that window was the "terminal" seen next to the player. Started from a console (the diagnostic
// .bat), it still inherits that console and prints there — the flag only decides whether a new one is made.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dj_bridge::{BridgeServer, ClientMessage, ServerMessage};
use dj_deck::{Deck, DeckSource};
use dj_domain::DeckId;
use dj_engine::{
    DeckStatic, DeckUpdate, EngineGraph, GraphLoad, LoadRequest, Published, StateCells, Submission,
    prepare_track_with_key,
};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

/// Which volume the mirror is filed under. One label for the machine's own files: the sidecar beside
/// the music is the source of truth, and this only keeps the mirror path stable.
/// The deck a command is about to stop, if it is about stopping.
///
/// A load is refused for a deck that is playing, and the flag that says so is refreshed at display
/// rate — slower than the block boundary a stop is applied at. The two commands that end playback are
/// named here so the flag can follow the intent at once.
/// The deck a command is about to start, if it is about starting.
fn starting_deck(command: &dj_domain::DjCommand) -> Option<dj_domain::DeckId> {
    match command {
        dj_domain::DjCommand::Play {
            deck,
            enabled: true,
        } => Some(*deck),
        _ => None,
    }
}

fn stopping_deck(command: &dj_domain::DjCommand) -> Option<dj_domain::DeckId> {
    match command {
        dj_domain::DjCommand::Play {
            deck,
            enabled: false,
        }
        | dj_domain::DjCommand::Eject { deck } => Some(*deck),
        _ => None,
    }
}

fn volume() -> &'static str {
    "root"
}

/// Tracks are filed in the mirror by their full path, so two files called `track.flac` in different
/// albums do not collide.
fn root() -> &'static std::path::Path {
    std::path::Path::new("/")
}

const USAGE: &str = "\
dj-engine — the audio engine process

Usage:
  dj-engine [--socket PATH] [--null-sink] [--device SUBSTR] [--block N] [--rate HZ]
            [--auto-gain-share 0..1]
            [--seconds S] [--autoplay] [--state-hz N] [--crossfader-curve LAW]
  dj-engine [--socket PATH] --stop      ask a running engine to shut down
  dj-engine --rebuild-index ROOT...     make the index out of the .djmeta records and exit

Defaults: socket $XDG_RUNTIME_DIR/hauler-dj-engine.sock on Linux, named pipe hauler-dj-engine-<user> on
          Windows; null sink, 48 kHz, block 256.
--device SUBSTR opens the real output (substring of the backend name); --null-sink keeps a
generated tone and a clock instead, so the command path works anywhere, including CI.
On a device with four or more outputs master goes to the first pair and cue to the second
(ADR-0005).

--seconds S renders for S seconds, prints the timing report as JSON and exits: the same
counters the probe reports, for the evidence directory.
--autoplay starts every deck playing, so a bounded run actually carries signal. Without it the
decks stay paused and every level reads zero, which says nothing about the output path.
--state-hz N publishes state N times a second (default 20, from display.state_hz in the settings).
--crossfader-curve LAW picks the crossfader law: constant_power (default), linear, smooth, sharp.
--cache-mb N keeps N megabytes of whole tracks in memory (default from pcm_cache.whole_track_gb,
one gigabyte, which holds four ten-minute tracks at 48 kHz); zero turns it off.
--cache-lossless on|off decides whether lossless files are kept there too (default on).
--stop asks the running engine to shut down over its socket. Ctrl-C works too, but a signal can be
blocked by the audio backend, so a stop must also be possible without one.

Exit codes: 0 stopped by a signal, 2 bad usage or a device that will not open.
";

/// A generated tone, so the command path works without a device or a file.
struct ToneSource {
    position: u64,
    frames: u64,
    phase: f32,
    step: f32,
    /// The rate the tone was generated at: a source that claims another rate is played at the wrong
    /// speed, and the measurement tone is the one thing a headless run can compare against.
    rate: u32,
}

impl ToneSource {
    fn new(sample_rate: u32, frequency: f32, seconds: f64) -> Self {
        Self {
            position: 0,
            frames: (f64::from(sample_rate) * seconds) as u64,
            phase: 0.0,
            step: std::f32::consts::TAU * frequency / sample_rate as f32,
            rate: sample_rate,
        }
    }
}

impl DeckSource for ToneSource {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for frame in 0..take {
            let value = self.phase.sin() * 0.2;
            self.phase = (self.phase + self.step) % std::f32::consts::TAU;
            out[frame * 2] = value;
            out[frame * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames);
        Ok(self.position)
    }
}

/// Clients currently attached to the fan-out.
///
/// The lock is taken by the publisher thread and by client threads, never by the render thread.
type Subscribers = Arc<Mutex<Vec<Sender<ServerMessage>>>>;

/// Set or read one value of the settings file, then leave.
///
/// The shell edits the same file the engine reads, and the editing has to keep comments and keys this build does not
/// know — which is a document edit, not a serialisation. It lives here, next to `dj-config`, and the shell calls it
/// with this one flag rather than keeping a second TOML writer of its own.
fn settings_request(argv: &[String]) -> Option<std::process::ExitCode> {
    let mut path: Option<String> = None;
    let mut set: Option<String> = None;
    let mut set_text: Option<String> = None;
    let mut get: Option<String> = None;
    let mut arguments = argv.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--settings" => path = arguments.next().cloned(),
            "--set-config" => set = arguments.next().cloned(),
            "--set-config-string" => set_text = arguments.next().cloned(),
            "--get-config" => get = arguments.next().cloned(),
            _ => {}
        }
    }
    if set.is_none() && set_text.is_none() && get.is_none() {
        return None;
    }
    let file = match path
        .map(std::path::PathBuf::from)
        .or_else(dj_config::Config::default_path)
    {
        Some(file) => file,
        None => {
            eprintln!("dj-engine: there is no settings file to write on this system");
            return Some(std::process::ExitCode::from(2));
        }
    };
    if let Some(pair) = set_text {
        let Some((key, value)) = pair.split_once('=') else {
            eprintln!("dj-engine: --set-config-string wants key=value, got {pair}");
            return Some(std::process::ExitCode::from(2));
        };
        return match dj_config::set_string(&file, key.trim(), value) {
            Ok(()) => {
                println!("{}", file.display());
                Some(std::process::ExitCode::SUCCESS)
            }
            Err(error) => {
                eprintln!("dj-engine: {error}");
                Some(std::process::ExitCode::from(2))
            }
        };
    }
    if let Some(pair) = set {
        let Some((key, value)) = pair.split_once('=') else {
            eprintln!("dj-engine: --set-config wants key=value, got {pair}");
            return Some(std::process::ExitCode::from(2));
        };
        match dj_config::set_value(&file, key.trim(), value.trim()) {
            Ok(()) => {
                println!("{}", file.display());
                Some(std::process::ExitCode::SUCCESS)
            }
            Err(error) => {
                eprintln!("dj-engine: {error}");
                Some(std::process::ExitCode::from(2))
            }
        }
    } else {
        match dj_config::get_value(&file, get.unwrap_or_default().trim()) {
            Ok(Some(value)) => {
                println!("{value}");
                Some(std::process::ExitCode::SUCCESS)
            }
            Ok(None) => Some(std::process::ExitCode::from(3)),
            Err(error) => {
                eprintln!("dj-engine: {error}");
                Some(std::process::ExitCode::from(2))
            }
        }
    }
}

/// What the headphones would get in each mode, so the window can show the decision instead of guessing it.
///
/// The rules live here, next to the audio device: `auto` takes the first free pair of the main interface, `same_device`
/// takes the DJ's pair when it is free, `device` would need a second stream — which this build does not open, so the cue
/// stays off and the answer says so.
fn cue_request(argv: &[String]) -> Option<std::process::ExitCode> {
    let mut mode: Option<String> = None;
    let mut path: Option<String> = None;
    let mut arguments = argv.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--resolve-cue" => mode = arguments.next().cloned(),
            "--settings" => path = arguments.next().cloned(),
            _ => {}
        }
    }
    let mode = mode?;
    let (config, _used) =
        match dj_config::Config::load_or_default(path.as_deref().map(std::path::Path::new)) {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("dj-engine: settings: {error}");
                return Some(std::process::ExitCode::from(2));
            }
        };
    let master = config.audio.master.unwrap_or((0, 1));
    let shape = match dj_audio_io::resolve_device(config.audio.device.as_deref()) {
        Ok(shape) => shape,
        Err(error) => {
            eprintln!("dj-engine: {error}");
            return Some(std::process::ExitCode::from(3));
        }
    };
    let headphones = &config.audio.headphones;
    let (device, cue) = match mode.trim() {
        "auto" => (
            shape.name.clone(),
            dj_audio_io::cue_channels(shape.channels, master, None, true),
        ),
        "same_device" => (
            shape.name.clone(),
            dj_audio_io::cue_channels(shape.channels, master, headphones.pair, headphones.enabled),
        ),
        // A second device is a second stream with its own clock; this build mixes one graph into one interface.
        // A device of its own: every channel on it is free, because the main output is not there.
        "device" => {
            let name = headphones.device.clone().unwrap_or_default();
            if name.trim().is_empty() {
                (name, None)
            } else {
                let cue_shape = dj_audio_io::resolve_device(Some(&name));
                let pair = cue_shape
                    .map(|shape| dj_audio_io::free_pair_on(shape.channels, headphones.pair))
                    .unwrap_or(None);
                // The DJ's own channels are kept when they exist; a cue device with room for them gets them, not 0/1.
                let pair = if pair.is_none() && !headphones.enabled {
                    None
                } else {
                    pair
                };
                (name, pair)
            }
        }
        other => {
            eprintln!("dj-engine: --resolve-cue does not know the mode {other:?}");
            return Some(std::process::ExitCode::from(2));
        }
    };
    match cue {
        Some((left, right)) => println!("{device}\t{left}\t{right}"),
        None => println!("{device}\tnone\tnone"),
    }
    Some(std::process::ExitCode::SUCCESS)
}

/// Answer a question about the audio devices, then leave.
///
/// The settings window offers a list instead of a text field, and asks one device to describe itself before the DJ
/// commits a name to a file the engine reads at start. Both answers come from the same backend the run will use, so a
/// name that looks plausible in a dialog cannot turn out to be unknown to the engine.
fn device_request(argv: &[String]) -> Option<std::process::ExitCode> {
    let mut list = false;
    let mut check: Option<String> = None;
    let mut pairs: Option<String> = None;
    let mut routes: Option<Option<String>> = None;
    let mut arguments = argv.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--list-devices" => list = true,
            "--check-device" => check = arguments.next().cloned(),
            "--device-pairs" => pairs = arguments.next().cloned(),
            // The value is optional: `--device-routes` alone means "the default device".
            "--device-routes" => {
                routes = Some(
                    argv.iter()
                        .position(|arg| arg == "--device-routes")
                        .and_then(|at| argv.get(at + 1))
                        .filter(|next| !next.starts_with("--"))
                        .cloned(),
                )
            }
            _ => {}
        }
    }
    if !list && check.is_none() && pairs.is_none() && routes.is_none() {
        return None;
    }
    if let Some(name) = routes {
        return match dj_audio_io::device_routes(name.as_deref()) {
            Ok(found) => {
                for (left, right) in found {
                    println!("[{left}, {right}]");
                }
                Some(std::process::ExitCode::SUCCESS)
            }
            Err(error) => {
                eprintln!("dj-engine: {error}");
                Some(std::process::ExitCode::from(3))
            }
        };
    }
    if let Some(name) = pairs {
        return match dj_audio_io::device_pairs(Some(name.trim())) {
            Ok(found) => {
                for (left, right) in found {
                    println!("[{left}, {right}]");
                }
                Some(std::process::ExitCode::SUCCESS)
            }
            Err(error) => {
                eprintln!("dj-engine: {error}");
                Some(std::process::ExitCode::from(3))
            }
        };
    }
    if list {
        let devices = match dj_audio_io::output_device_list() {
            Ok(devices) => devices,
            Err(error) => {
                eprintln!("dj-engine: {error}");
                return Some(std::process::ExitCode::from(2));
            }
        };
        for device in devices {
            // One device per line, tab separated: name, channels, rate, and a star for the default one.
            println!(
                "{}\t{}\t{}\t{}\t{}",
                device.name,
                device.channels,
                device.native_channels,
                device.sample_rate,
                if device.default { "*" } else { "" }
            );
        }
        return Some(std::process::ExitCode::SUCCESS);
    }
    let needle = check.unwrap_or_default();
    match dj_audio_io::resolve_device(Some(needle.trim())) {
        Ok(shape) => {
            // Characteristics only: the window already shows the name it asked about, and repeating it wastes the one
            // line the DJ reads (owner's decision, 21.11).
            println!(
                "{}\t{}\t{}\t{}",
                shape.sample_rate, shape.bits, shape.channels, shape.format
            );
            Some(std::process::ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("dj-engine: {error}");
            Some(std::process::ExitCode::from(3))
        }
    }
}

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // A settings edit is not a run: it answers and leaves before the logger, the index or the audio device.
    if let Some(exit) = settings_request(&argv) {
        return exit;
    }
    if let Some(exit) = device_request(&argv) {
        return exit;
    }
    if let Some(exit) = cue_request(&argv) {
        return exit;
    }
    // Before anything else: the level, then the environment. A report that names the file, the folders and the
    // switches is what tells two installations apart when a fix seems not to have arrived.
    // The level can be asked for in the settings file; `HAULER_LOG` still wins when it is set.
    let mut settings_path: Option<String> = None;
    let mut arguments = argv.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--settings" {
            settings_path = arguments.next().cloned();
            break;
        }
    }
    let file_level = settings_path
        .as_deref()
        .map(std::path::Path::new)
        .and_then(|path| dj_config::Config::load_or_default(Some(path)).ok())
        .and_then(|(config, _used)| config.diagnostics.level().map(str::to_string));
    dj_engine::log::init_with(file_level.as_deref());
    dj_engine::log::capture_panics();
    dj_engine::log::startup_report(&argv);
    // The writer brings the database up to the current schema, once, at start. A reader no longer does
    // it — a reader that migrates takes the writer's lock away from a load that is recording names — so
    // if nobody else did, the page query would look for a column that is not there and answer "nothing".
    if !argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        match dj_library::Index::open_default() {
            Ok(index) => drop(index),
            Err(error) => eprintln!("dj-engine: the index stayed as it was: {error}"),
        }
    }
    match run(&argv) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            // The same words in both places: the terminal shows them to whoever started the engine, and the log carries
            // them to the launcher, which reports a start that died and has nothing else to read (owner's report, 21.11).
            eprintln!("dj-engine: {message}");
            dj_engine::log::error(message.clone());
            std::process::ExitCode::from(2)
        }
    }
}

/// Make the index out of the `.djmeta` records under these roots, then leave.
///
/// The records are the source of truth, so this is the recovery path after a lost database and the
/// command for a collection that was filled before the index existed. The audio is never opened.
fn rebuild_index(roots: &[PathBuf]) -> Result<(), String> {
    let mut index = dj_library::Index::open_default().map_err(|error| error.to_string())?;
    let mut shown = 0usize;
    let report = index
        .rebuild_from_with(roots, &mut |progress| {
            // A line per tenth: walking a large collection is a minute, and silence looks like a hang.
            let step = (progress.total / 10).max(1);
            if progress.done >= shown + step || !progress.running {
                shown = progress.done;
                println!(
                    "dj-engine: rebuild {} of {} ({} skipped)",
                    progress.done, progress.total, progress.failed
                );
            }
        })
        .map_err(|error| error.to_string())?;
    println!(
        "dj-engine: rebuild wrote {} tracks, {} genre links, skipped {} records",
        report.tracks, report.genres, report.skipped
    );
    Ok(())
}

fn run(argv: &[String]) -> Result<(), String> {
    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return Ok(());
    }

    let mut socket: Option<PathBuf> = None;
    let mut null_sink = false;
    let mut device: Option<String> = None;
    let mut block = 256usize;
    let mut rate = 48_000u32;
    let mut seconds: Option<f64> = None;
    let mut autoplay = false;
    let mut request_stop = false;
    // Settings first, so the flags default to them rather than to numbers scattered in the code. The file
    // is found in a pre-pass, because every default below depends on it: a flag still wins over the file,
    // and a file that cannot be read is named rather than quietly ignored (docs/operations/settings.md).
    let named_settings = argv
        .iter()
        .position(|arg| arg == "--config")
        .and_then(|at| argv.get(at + 1))
        .map(PathBuf::from);
    let (defaults, settings_used) = dj_config::Config::load_or_default(named_settings.as_deref())
        .map_err(|error| format!("settings: {error}"))?;
    if let Some(path) = settings_used.as_ref() {
        eprintln!("dj-engine: settings from {}", path.display());
    }
    let mut state_hz = defaults.display.state_hz;
    let frame_hz = defaults.display.frame_hz;
    // A play shorter than this is not listening, and both numbers live in `crates/dj-config` so the
    // owner can move them without a build (DJP-MEDIA-004).
    let mut history_min_seconds = defaults.library.history_min_seconds;
    // How long a track stays "played recently": the collection column is drawn against this number.
    let mut played_recently_hours = defaults.library.played_recently_hours;
    // Every law is a choice, not a constant: the default is the DJ one, and a person can ask for any.
    let mut crossfader_curve = dj_mixer::CrossfaderCurve::default();
    // How much of the measured level correction a deck applies. Nine tenths is the default because a
    // full one puts peaks in the limiter; the flag is here so that is a choice, not a constant.
    let mut auto_gain_share = defaults.analysis.share();
    // The whole-track cache. The budget is the owner's setting (`pcm_cache.whole_track_gb`), because a
    // gigabyte is a number about tracks, not about the machine: four ten-minute tracks at 48 kHz are
    // 922 MB, and anything larger only waits to be evicted.
    let mut cache_mb: u64 = (defaults.pcm_cache.whole_track_gb * 1024.0)
        .round()
        .max(0.0) as u64;
    let mut cache_lossless = defaults.pcm_cache.lossless;
    // Which sockets carry which bus: the settings file names them, the plan refuses what the device does not have.
    let chosen_routes = ChosenRoutes {
        master: defaults.audio.master,
        cue: defaults.audio.headphones.pair,
        mode: defaults.audio.headphones.mode,
        cue_enabled: defaults.audio.headphones.enabled,
        cue_device: defaults.audio.headphones.device.clone(),
    };

    let mut index = 0;
    while index < argv.len() {
        let flag = argv[index].as_str();
        let value = |offset: usize| -> Result<String, String> {
            argv.get(index + offset)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "--socket" => {
                socket = Some(PathBuf::from(value(1)?));
                index += 2;
            }
            "--rebuild-index" => {
                // Everything after the flag is a collection root. This run makes the database out of
                // the records and exits: no socket, no audio, no engine.
                let roots: Vec<PathBuf> = argv[index + 1..].iter().map(PathBuf::from).collect();
                if roots.is_empty() {
                    return Err("--rebuild-index needs at least one collection root".to_string());
                }
                return rebuild_index(&roots);
            }
            "--null-sink" => {
                null_sink = true;
                index += 1;
            }
            "--device" => {
                device = Some(value(1)?);
                index += 2;
            }
            "--block" => {
                block = value(1)?.parse().map_err(|_| "--block expects a number")?;
                index += 2;
            }
            "--rate" => {
                rate = value(1)?.parse().map_err(|_| "--rate expects a number")?;
                index += 2;
            }
            "--state-hz" => {
                let parsed: u32 = value(1)?
                    .parse()
                    .map_err(|_| "--state-hz expects a number")?;
                if !(1..=120).contains(&parsed) {
                    return Err("--state-hz must be 1..=120 snapshots a second".to_string());
                }
                state_hz = parsed;
                index += 2;
            }
            "--played-recently-hours" => {
                let parsed: u32 = value(1)?
                    .parse()
                    .map_err(|_| "--played-recently-hours expects a number")?;
                if !(1..=168).contains(&parsed) {
                    return Err("--played-recently-hours must be 1..=168 hours".to_string());
                }
                played_recently_hours = parsed;
                index += 2;
            }
            "--config" => {
                // Read in the pre-pass above: the defaults of every other flag came from it.
                let _ = value(1)?;
                index += 2;
            }
            "--history-min-seconds" => {
                let parsed: u32 = value(1)?
                    .parse()
                    .map_err(|_| "--history-min-seconds expects a number")?;
                if !(1..=3600).contains(&parsed) {
                    return Err("--history-min-seconds must be 1..=3600 seconds".to_string());
                }
                history_min_seconds = parsed;
                index += 2;
            }
            "--auto-gain-share" => {
                let parsed: f32 = value(1)?
                    .parse()
                    .map_err(|_| "--auto-gain-share expects a number")?;
                if !(0.0..=1.0).contains(&parsed) {
                    return Err("--auto-gain-share must be 0..=1".to_string());
                }
                auto_gain_share = parsed;
                index += 2;
            }
            "--cache-mb" => {
                cache_mb = value(1)?
                    .parse()
                    .map_err(|_| "--cache-mb expects megabytes")?;
                index += 2;
            }
            "--cache-lossless" => {
                cache_lossless = match value(1)?.as_str() {
                    "on" | "true" | "yes" => true,
                    "off" | "false" | "no" => false,
                    _ => return Err("--cache-lossless expects on or off".to_string()),
                };
                index += 2;
            }
            "--crossfader-curve" => {
                let name = value(1)?;
                crossfader_curve = dj_mixer::CrossfaderCurve::parse(&name).ok_or_else(|| {
                    let known: Vec<&str> = dj_mixer::CrossfaderCurve::ALL
                        .iter()
                        .map(|law| law.as_str())
                        .collect();
                    format!("--crossfader-curve expects one of: {}", known.join(", "))
                })?;
                index += 2;
            }
            "--stop" => {
                request_stop = true;
                index += 1;
            }
            "--autoplay" => {
                autoplay = true;
                index += 1;
            }
            "--seconds" => {
                let parsed: f64 = value(1)?
                    .parse()
                    .map_err(|_| "--seconds expects a number")?;
                if !parsed.is_finite() || parsed <= 0.0 {
                    return Err("--seconds must be a positive number".to_string());
                }
                seconds = Some(parsed);
                index += 2;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }

    // Look at the device before claiming the socket: a wrong name should not leave a socket behind
    // that a client can connect to and then wait on for ever.
    if request_stop {
        let socket = socket.unwrap_or_else(default_socket);
        let (mut client, _welcome) =
            dj_bridge::connect_with_timeout(&socket, Some(Duration::from_secs(2)))
                .map_err(|err| format!("no engine at {}: {err}", socket.display()))?;
        client
            .send_client(&dj_bridge::ClientMessage::Quit)
            .map_err(|err| format!("cannot ask the engine to stop: {err}"))?;
        eprintln!("dj-engine: stop requested at {}", socket.display());
        return Ok(());
    }

    // A device that will not open costs the DJ the sound, not the program: the engine keeps running on the null
    // sink and says so, so the shell still starts and the log names the reason. Exiting left a window with no
    // engine at all, which is a worse answer than a silent one.
    let device_channels = match &device {
        Some(requested) => match dj_audio_io::output_channels(Some(requested), 4) {
            Ok(channels) => Some(channels),
            Err(err) => {
                let named = if requested.is_empty() {
                    "the default output device".to_string()
                } else {
                    format!("the device {requested:?}")
                };
                dj_engine::log::info(format!(
                    "{named} would not open: {err}; falling back to the null sink"
                ));
                device = None;
                None
            }
        },
        None => None,
    };

    if device.is_some() {
        dj_engine::log::output_report();
    }
    // Which output the engine took: a silent app on Windows was the null sink, and nothing in the log said so.
    match &device {
        Some(name) if name.is_empty() => {
            dj_engine::log::info("output: the system's default device");
        }
        Some(name) => dj_engine::log::info(format!("output: device matching {name:?}")),
        None => dj_engine::log::info(
            "output: null sink (a generated tone; pass --device for real sound)",
        ),
    }

    let socket = socket.unwrap_or_else(default_socket);
    let server = BridgeServer::bind(&socket).map_err(|err| err.to_string())?;
    eprintln!("dj-engine: listening on {}", server.path().display());

    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1);

    // Decks and mixer live on the render loop; clients only ever send commands to it.
    let mut decks: Vec<Deck> = Vec::new();
    // Four decks, because the shell draws four and the mixer has a strip for each: a deck the engine
    // does not create is a panel whose controls can never do anything.
    for id in [DeckId::A, DeckId::B, DeckId::C, DeckId::D] {
        let mut deck = Deck::new(id);
        // What rate the deck is played at: a file recorded at another rate is resampled to this one
        // instead of being played at the wrong speed.
        deck.set_output_rate(rate);
        // A deck starts empty: "Ready" has to mean "a file is in the deck", or a client cannot tell
        // a track from the measurement tone that used to sit here.
        if autoplay {
            deck.attach(Box::new(ToneSource::new(rate, 440.0, 600.0)), None);
        }
        deck.set_blocking(false);
        // The loop seam is a setting in the deck; until it reaches the settings file (A08/A10) the ear
        // that matters can choose it here: DJ_LOOP_SEAM_MS=0 plays the seam as a cut, 5 as a longer
        // crossfade. Measured on the reference tracks, two milliseconds is the best of the tested values
        // on three of four, and a longer fade blurs the attack of whatever the loop starts with.
        if let Ok(value) = std::env::var("DJ_LOOP_SEAM_MS")
            && let Ok(milliseconds) = value.trim().parse::<f64>()
            && milliseconds.is_finite()
        {
            // The shape is measurable but not yet in the settings file either: `DJ_LOOP_SEAM_SHAPE=power`
            // mixes the two sides by power (the classic micro-crossfade for uncorrelated material),
            // anything else by gain (the sum stays at one, right for the same track a millisecond apart).
            let shape = match std::env::var("DJ_LOOP_SEAM_SHAPE").as_deref() {
                Ok("power") => dj_deck::SeamShape::EqualPower,
                _ => dj_deck::SeamShape::EqualGain,
            };
            deck.set_loop_config(dj_deck::LoopConfig {
                buffer_max_seconds: 30.0,
                seam_shape: shape,
                seam_ms: milliseconds.max(0.0),
            });
        }
        decks.push(deck);
    }
    let mut mixer = Mixer::new(decks.len());
    mixer.prepare(rate, block);
    mixer.set_curve(crossfader_curve);
    for index in 0..decks.len() {
        mixer.set_channel(
            index,
            ChannelSettings {
                trim_db: 0.0,
                fader: 1.0,
                // Replaced below by the deck's side of the crossfader.
                assign: CrossAssign::Thru,
                cue: false,
            },
        );
    }
    let ids: Vec<DeckId> = decks.iter().map(dj_deck::Deck::id).collect();
    dj_engine::assign_crossfader_sides(&mut mixer, &ids);

    // A measurement of silence proves nothing about the signal path, and decks start paused.
    if autoplay {
        for deck in &mut decks {
            deck.play();
        }
    }

    // The render thread writes scalars here; nothing in the audio path allocates or locks.
    let cells = Arc::new(StateCells::new());
    // Что уже стоит в хранилище — с первой публикации, а не после команды: иначе после рестарта
    // экран говорит «built-in» про установленную модель, пока её не скачают заново.
    dj_engine::beatmodel::seed_model(&cells, &dj_models::Store::default_root());
    dj_engine::beatmodel::announce();
    // The playlists come back before anything else: the index says what this session has, the mirror says
    // what survived it, and the tree is published once so the panel opens on it (DJP-MEDIA-004).
    if let Some(mirror) = dj_engine::playlists::default_mirror() {
        let outcome = dj_library::Index::open_default()
            .map_err(|error| error.to_string())
            .and_then(|mut index| dj_engine::playlists::startup_pass(&mut index, &mirror));
        match outcome {
            Ok(tree) => {
                eprintln!(
                    "dj-engine: playlists ready: {} folders, {} playlists",
                    tree.folders.len(),
                    tree.playlists.len()
                );
                cells.set_playlists(tree);
            }
            Err(message) => eprintln!("dj-engine: playlists: {message}"),
        }
    }
    let statics = decks
        .iter()
        .map(|deck| DeckStatic::from_snapshot(&deck.snapshot()))
        .collect::<Vec<_>>();
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));
    // Whether a client has ever been here. An engine nobody has opened yet is not an idle engine: the grace is
    // for the one a shell has left, not for one that is still being set up by a script.
    let seen_client = Arc::new(AtomicBool::new(false));
    let (submissions, submissions_rx) = std::sync::mpsc::channel::<Submission>();
    // Requests to measure a tempo again; the worker below owns the answers.
    let (analysis_requests, analysis_requests_rx) = std::sync::mpsc::channel::<AnalysisRequest>();
    // Requests to edit a grid. The grid in force lives with the deck; the publisher hands a copy to
    // this map on every change, and the edit worker is the only one who reads it.
    let (grid_edits, grid_edits_rx) = std::sync::mpsc::channel::<GridEditRequest>();
    // A locked grid is a decision the DJ made about the track: the handler refuses an edit before it is
    // sent and the worker refuses one that slipped in, so the file cannot be rewritten behind his back.
    let locks: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(vec![false; dj_domain::DeckId::COUNT]));
    let grids: Arc<Mutex<Vec<Option<dj_domain::BeatGrid>>>> =
        Arc::new(Mutex::new(vec![None; dj_domain::DeckId::COUNT]));
    // The source rate of each deck's file: the grid is expressed in it, and a shift past the start
    // rebases by whole squares, which is a number of frames that needs the rate.
    let rates: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(vec![0; dj_domain::DeckId::COUNT]));
    // The file each deck holds. A reconnecting client never loaded anything and cannot name it, so the
    // engine answers for the deck: an edit that names only the deck still knows what to write beside.
    let paths: Arc<Mutex<Vec<Option<String>>>> =
        Arc::new(Mutex::new(vec![None; dj_domain::DeckId::COUNT]));
    // Loads never enter the audio callback: clients hand them to a loader thread, which hands a
    // prepared source back to the graph and a new description to the publisher.
    let (load_requests, load_requests_rx) = std::sync::mpsc::channel::<LoadRequest>();
    // Cue and loop edits become files here, on a thread of their own (DJP-DATA-006). The audio
    // callback only sends; a disk call inside it is the one thing the RT contract forbids.
    let store = dj_library::Store::with_default_mirror();
    // The writer owns the queue and is dropped before the process leaves, so an edit answered just
    // before the shutdown still reaches the disk (A13).
    // Kept alive until this function returns: dropping the writer closes the queue and waits for
    // the last edit, so an edit answered just before the shutdown still reaches the disk (A13).
    // The journal of plays: a finished interval goes to its own thread, the way a metadata edit does,
    // because the audio callback may not touch a database (DJP-MEDIA-004).
    let (_play_writer, play_records) = dj_engine::plays::PlayWriter::spawn();
    let journal = Arc::new(Mutex::new(dj_engine::history::History::new(
        history_min_seconds,
    )));
    let (_meta_writer, meta_edits) = dj_engine::meta::MetaWriter::spawn(
        store.clone(),
        volume().to_string(),
        root().to_path_buf(),
    );

    let (graph_loads, graph_loads_rx) = std::sync::mpsc::channel::<GraphLoad>();
    let (deck_updates, deck_updates_rx) = std::sync::mpsc::channel::<Published>();

    // Ctrl-C must stop the engine. PipeWire's thread loop blocks signals in the thread that creates
    // it, so the flag and the receiving thread have to exist before the stream does.
    let stop = Arc::new(AtomicBool::new(false));
    register_signals(&stop)?;
    keep_a_signal_receiving_thread(&stop);

    // The publisher owns snapshot assembly: one allocation, at display rate, off the audio path.
    // The count is kept so a bounded run can report the rate it actually achieved: the setting is
    // only worth having if the wire carries what it promises.
    let published = Arc::new(std::sync::atomic::AtomicU64::new(0));
    // Per-deck "is playing", refreshed by the publisher. A load has to be refused for a deck that is
    // playing, and the thread that answers the client never sees the graph.
    let playing: Arc<Vec<AtomicBool>> =
        Arc::new((0..DeckId::COUNT).map(|_| AtomicBool::new(false)).collect());
    // A stop is applied at the next block boundary, and the flag above is refreshed from the published
    // snapshot — so for one publish after an accepted stop it still says "playing", and a load sent in
    // that window is refused for a state that no longer exists. This flag says "the deck was told to
    // stop and has not been seen stopped yet", which is what a load has to respect: the DJ pressing
    // stop and then load is not cutting a track out from under anyone.
    let stopping: Arc<Vec<AtomicBool>> =
        Arc::new((0..DeckId::COUNT).map(|_| AtomicBool::new(false)).collect());
    {
        let cells = Arc::clone(&cells);
        let published = Arc::clone(&published);
        let playing = Arc::clone(&playing);
        let stopping = Arc::clone(&stopping);
        let subscribers = Arc::clone(&subscribers);
        let grids = Arc::clone(&grids);
        let rates = Arc::clone(&rates);
        let paths = Arc::clone(&paths);
        let locks = Arc::clone(&locks);
        // The state rate is a parameter (display.state_hz): twenty snapshots a second is the
        // default, and raising it trades IPC for less projection between snapshots.
        let publish_every = Duration::from_micros(1_000_000 / u64::from(state_hz));
        let journal = Arc::clone(&journal);
        let play_records = play_records.clone();
        let stop = Arc::clone(&stop);
        let recent_window_ms = u64::from(played_recently_hours) * 3_600_000;
        std::thread::spawn(move || {
            let mut revision: u64 = 0;
            let mut statics = statics;
            // Asked for a key only when an interval opens: one fingerprint per play, not one per snapshot.
            let print_of = |path: &str| {
                dj_library::fingerprint::of_path(std::path::Path::new(path))
                    .ok()
                    .map(|print| print.key())
            };
            let mut ticks: u64 = 0;
            let mut last_report = std::time::Instant::now();
            loop {
                std::thread::sleep(publish_every);
                // Once a second: is anything moving? A deck that says `Ready` while the graph renders nothing
                // looks exactly like a deck playing silence, and only the numbers tell them apart.
                if last_report.elapsed() >= Duration::from_secs(1) {
                    last_report = std::time::Instant::now();
                    let clients = subscribers.lock().map(|list| list.len()).unwrap_or(0);
                    let states: Vec<String> = statics
                        .iter()
                        .map(|entry| {
                            format!(
                                "{:?}={}{}",
                                entry.deck,
                                match &entry.load {
                                    dj_domain::DeckLoad::Empty => "Empty".to_string(),
                                    dj_domain::DeckLoad::Loading { .. } => "Loading".to_string(),
                                    dj_domain::DeckLoad::Ready => "Ready".to_string(),
                                    dj_domain::DeckLoad::Failed { message } => {
                                        format!("Failed({message})")
                                    }
                                },
                                entry
                                    .path
                                    .as_ref()
                                    .map(|path| format!(" {path}"))
                                    .unwrap_or_default()
                            )
                        })
                        .collect();
                    dj_engine::log::debug(format!(
                        "state: rev {revision}, ticks {ticks}, snapshots {}, clients {clients}, decks [{}]",
                        published.load(Ordering::Relaxed),
                        states.join(", ")
                    ));
                }
                ticks += 1;
                if stop.load(Ordering::Relaxed) {
                    // The way out: close what is still open and hand it over before leaving, or the play
                    // the DJ is listening to right now is the one nobody records.
                    if let Ok(mut held) = journal.lock() {
                        for record in held.close_all() {
                            let _ = play_records.send(dj_engine::plays::record_of(&record));
                        }
                    }
                    break;
                }
                // A load changes what the deck is; only this thread reads the description, so the
                // audio path never sees a lock.
                while let Ok(update) = deck_updates_rx.try_recv() {
                    match update {
                        Published::Deck(update) => {
                            let deck = update.deck;
                            let static_info = update.static_info;
                            let grid = static_info.grid.clone();
                            let rate = static_info.source_rate;
                            // A track that arrives already locked must refuse an edit at once, not after the
                            // DJ touches the button: the deck is the one that knows what the file said. That
                            // knowledge is as old as the load, though, and a load already in flight lands
                            // after the press — the rule that keeps the two apart, with its reasoning and its
                            // case, is `dj_engine::apply_deck_publication`.
                            let lock = match statics.iter_mut().find(|entry| entry.deck == deck) {
                                Some(slot) => dj_engine::apply_deck_publication(slot, static_info),
                                None => static_info.analysis_lock,
                            };
                            if let Ok(mut held) = grids.lock()
                                && let Some(slot) = held.get_mut(deck.index())
                            {
                                *slot = grid;
                            }
                            if let Ok(mut held) = rates.lock()
                                && let Some(slot) = held.get_mut(deck.index())
                            {
                                *slot = rate;
                            }
                            if let Ok(mut held) = locks.lock()
                                && let Some(slot) = held.get_mut(deck.index())
                            {
                                *slot = lock;
                            }
                        }
                        Published::AnalysisLock { deck, enabled } => {
                            if let Some(slot) = statics.iter_mut().find(|entry| entry.deck == deck)
                            {
                                slot.analysis_lock = enabled;
                            }
                        }
                        Published::Grid { deck, grid } => {
                            if let Ok(mut held) = grids.lock()
                                && let Some(slot) = held.get_mut(deck.index())
                            {
                                *slot = Some(*grid.clone());
                            }
                            if let Some(slot) = statics.iter_mut().find(|entry| entry.deck == deck)
                            {
                                slot.grid = Some(*grid.clone());
                                if let Some(track) = slot.track.as_mut() {
                                    track.bpm = Some(grid.bpm());
                                    track.first_beat_frame = Some(grid.first_beat_frame());
                                    track.grid = Some(*grid);
                                }
                            }
                        }
                        Published::Analyzing { deck, analyzing } => {
                            if let Some(slot) = statics.iter_mut().find(|entry| entry.deck == deck)
                            {
                                slot.analyzing = analyzing;
                            }
                        }
                    }
                }
                // The tempo base of every deck, refreshed from what the decks say rather than from
                // load events: a client that reconnects to an engine whose decks are already loaded
                // sends no load, and an editor with no rate refuses every shift — the buttons look dead.
                if let Ok(mut held) = rates.lock() {
                    for entry in &statics {
                        if let Some(slot) = held.get_mut(entry.deck.index()) {
                            *slot = entry.source_rate;
                        }
                    }
                }
                if let Ok(mut held) = paths.lock() {
                    for entry in &statics {
                        if let Some(slot) = held.get_mut(entry.deck.index()) {
                            *slot = entry.path.clone();
                        }
                    }
                }
                revision += 1;
                published.fetch_add(1, Ordering::Relaxed);
                let mut snapshot = cells.read(&statics, rate, epoch, revision);
                for deck in &snapshot.decks {
                    if let Some(flag) = playing.get(deck.deck.index()) {
                        flag.store(deck.playing, Ordering::Relaxed);
                    }
                    // The graph has been seen with this deck standing still: the stop has landed, and a
                    // load is judged on the plain flag again.
                    if !deck.playing
                        && let Some(flag) = stopping.get(deck.deck.index())
                    {
                        flag.store(false, Ordering::Relaxed);
                    }
                }
                // The journal follows every deck at the state rate: what a deck is doing now decides
                // what the interval that just ended was worth (DJP-MEDIA-004).
                if let Ok(mut held) = journal.lock() {
                    for deck in &snapshot.decks {
                        let path = paths
                            .lock()
                            .ok()
                            .and_then(|slots| slots.get(deck.deck.index()).cloned())
                            .flatten();
                        held.observe(
                            deck.deck.index(),
                            deck.playing,
                            path.as_deref(),
                            snapshot.audio_clock_samples,
                            snapshot.sample_rate,
                            &print_of,
                        );
                    }
                    for record in held.take_finished() {
                        let _ = play_records.send(dj_engine::plays::record_of(&record));
                    }
                    snapshot.plays = held.recent();
                }
                snapshot.recent_window_ms = recent_window_ms;
                snapshot.frame_hz = frame_hz;
                let message = ServerMessage::Snapshot(Box::new(snapshot));

                if let Ok(mut list) = subscribers.lock() {
                    list.retain(|subscriber| subscriber.send(message.clone()).is_ok());
                }
            }
        });
    }

    // Files the collection asks to measure: a full decode each, so a worker of their own, never a
    // client thread and never the audio callback.
    let (file_requests, file_requests_rx) = std::sync::mpsc::channel::<AnalysisBatch>();
    let analysis_cancel = Arc::new(AtomicBool::new(false));
    let analysis_running = Arc::new(AtomicBool::new(false));
    {
        let store = store.clone();
        let cells = Arc::clone(&cells);
        let cancel = Arc::clone(&analysis_cancel);
        let running = Arc::clone(&analysis_running);
        std::thread::spawn(move || {
            while let Ok(job) = file_requests_rx.recv() {
                running.store(true, Ordering::Relaxed);
                cancel.store(false, Ordering::Relaxed);
                // A panic inside a batch used to kill this worker for good: `running` stayed true, the window stood
                // still and every later press queued behind a thread that no longer existed. The batch is healed
                // here, and the journal says what happened.
                let healed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    dj_engine::run_analysis(
                        job.paths,
                        Arc::clone(&cells),
                        dj_engine::AnalysisTarget {
                            store: store.clone(),
                            volume: volume().to_string(),
                            root: root().to_path_buf(),
                        },
                        Arc::clone(&cancel),
                        Arc::clone(&running),
                        job.parallel,
                    );
                }));
                if healed.is_err() {
                    dj_engine::log::error(
                        "the analysis batch panicked; the worker stays up and the next press is heard",
                    );
                    // The state must not stay `running` after a fault: the window would never close.
                    if let Some(mut state) = cells.analysis() {
                        state.running = false;
                        state.current = None;
                        cells.set_analysis(Some(state));
                    }
                    running.store(false, Ordering::Relaxed);
                }
            }
        });
    }

    // Client threads: they never touch a deck, only the channel.
    {
        let subscribers = Arc::clone(&subscribers);
        let cells = Arc::clone(&cells);
        let work = Workers {
            cells: Arc::clone(&cells),
            store: store.clone(),
            files: file_requests.clone(),
            analysis_cancel: Arc::clone(&analysis_cancel),
            analysis_running: Arc::clone(&analysis_running),
            loads: load_requests.clone(),
            analyses: analysis_requests.clone(),
            grids: grid_edits.clone(),
            locks: Arc::clone(&locks),
            paths: Arc::clone(&paths),
            published: deck_updates.clone(),
        };
        let stop_handle = Arc::clone(&stop);
        let accept_seen = Arc::clone(&seen_client);
        std::thread::spawn(move || {
            // The build, not just the version: two installs of one version look the same in a log, and «is this
            // the file I just installed?» is the first question when a fix seems not to have arrived.
            let exe = std::env::current_exe().unwrap_or_default();
            let built = std::fs::metadata(&exe)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_millis().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            dj_engine::log::info(format!(
                "engine {} from {} (file time {}) listening on {}",
                env!("CARGO_PKG_VERSION"),
                exe.display(),
                built,
                server.path().display()
            ));
            // Accept forever: a refused connection must not take the engine down with it.
            let mut accepted = 0;
            loop {
                let mut connection = match server.accept() {
                    Ok(connection) => connection,
                    Err(err) => {
                        dj_engine::log::info(format!("accept failed: {err}"));
                        continue;
                    }
                };
                accepted += 1;
                // Who connected, in the order they came: a shell that never appears here never reached the
                // engine, and that is the first thing to know when a window looks empty.
                dj_engine::log::info(format!("client #{accepted} connected"));
                if let Err(err) = BridgeServer::handshake(&mut connection, epoch) {
                    dj_engine::log::info(format!("client #{accepted}: handshake refused: {err}"));
                    continue;
                }
                dj_engine::log::info(format!("client #{accepted}: handshake done"));
                accept_seen.store(true, Ordering::Relaxed);
                let (outgoing, receiver) = std::sync::mpsc::channel::<ServerMessage>();
                // Greet with the counters as they are now; the publisher fills the rest within one
                // tick, so the client is never left without a snapshot.
                let _ = cells;
                let outgoing_clone = outgoing.clone();
                if let Ok(mut list) = subscribers.lock() {
                    list.push(outgoing);
                }
                let submissions = submissions.clone();
                let alerts = outgoing_clone;
                let work = work.clone();
                let stop = Arc::clone(&stop_handle);
                let flags = DeckFlags {
                    playing: Arc::clone(&playing),
                    stopping: Arc::clone(&stopping),
                };
                std::thread::spawn(move || {
                    serve_client(connection, receiver, alerts, submissions, work, stop, flags)
                });
            }
        });
    }

    // The engine leaves a grace period after its last client: closing the shell closes the application, and a
    // shell started again inside the window finds a warm engine. `HAULER_IDLE_EXIT_MS=0` switches it off, which
    // is what a check that shares one engine across runs needs.
    {
        let grace_ms: u64 = std::env::var("HAULER_IDLE_EXIT_MS")
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(10_000);
        if grace_ms > 0 {
            let grace = Duration::from_millis(grace_ms);
            let idle_stop = Arc::clone(&stop);
            let idle_subs = Arc::clone(&subscribers);
            let idle_seen = Arc::clone(&seen_client);
            dj_engine::log::info(format!(
                "idle exit: the engine leaves {} ms after its last client",
                grace_ms
            ));
            std::thread::spawn(move || {
                let mut idle_since: Option<std::time::Instant> = None;
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if idle_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let connected = idle_subs.lock().map(|list| list.len()).unwrap_or(0);
                    if connected > 0 || !idle_seen.load(Ordering::Relaxed) {
                        idle_since = None;
                        continue;
                    }
                    let since = *idle_since.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() >= grace {
                        dj_engine::log::info(format!(
                            "no client for {} ms: the engine leaves",
                            grace.as_millis()
                        ));
                        idle_stop.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            });
        }
    }

    // Re-measuring a tempo is another full decode, so it gets a thread of its own: the DJ asks for it
    // by hand, and the answer replaces the grid the deck and the file carry (B02).
    {
        let prepared = graph_loads.clone();
        let published_to = deck_updates.clone();
        let store = store.clone();
        std::thread::spawn(move || {
            while let Ok(request) = analysis_requests_rx.recv() {
                let deck = request.deck;
                // The screen says what is happening while it happens: a decode of a five-minute track
                // is long enough that a silent button reads as a button that did nothing.
                let started = std::time::Instant::now();
                let _ = published_to.send(Published::Analyzing {
                    deck,
                    analyzing: true,
                });
                let _ = prepared.send(GraphLoad::SetAnalyzing {
                    deck,
                    analyzing: true,
                });
                let path = std::path::PathBuf::from(&request.path);
                match dj_engine::beatmodel::analyse(&path) {
                    Some(found) => {
                        // An explicit request outranks what the file remembers: that is the whole
                        // point of the button — the stored grid was wrong.
                        let segments: Vec<dj_domain::BeatSegment> = found
                            .segments
                            .iter()
                            .map(|segment| dj_domain::BeatSegment {
                                frame: segment.first_beat_frame,
                                beat: segment.beat,
                                bpm: segment.bpm,
                            })
                            .collect();
                        let Some(grid) = dj_domain::BeatGrid::from_segments(segments)
                            .map(|grid| grid.with_beats_per_bar(found.metre))
                        else {
                            eprintln!("dj-engine: the measurement is not a grid");
                            continue;
                        };
                        if let Err(message) = dj_engine::meta::write_beatgrid(
                            &store,
                            volume(),
                            root(),
                            &path,
                            &grid,
                            true,
                        ) {
                            eprintln!("dj-engine: {message}");
                        }
                        // The publisher carries the wire: the deck's own numbers are for the graph,
                        // the description is what a client reads, and both have to change together.
                        let _ = published_to.send(Published::Grid {
                            deck,
                            grid: Box::new(grid.clone()),
                        });
                        let _ = prepared.send(GraphLoad::SetGrid {
                            deck,
                            grid: Box::new(grid),
                        });
                    }
                    None => eprintln!(
                        "dj-engine: no tempo found in {} — the grid is unchanged",
                        request.path
                    ),
                }
                // The answer is published at once, the badge lingers: a state that lasts a couple of
                // frames is not information, it is a flicker. The measurement is over either way.
                let shown = std::time::Duration::from_millis(400);
                if let Some(left) = shown.checked_sub(started.elapsed()) {
                    std::thread::sleep(left);
                }
                let _ = published_to.send(Published::Analyzing {
                    deck,
                    analyzing: false,
                });
                let _ = prepared.send(GraphLoad::SetAnalyzing {
                    deck,
                    analyzing: false,
                });
            }
        });
    }

    // Grid edits: no decode, but they write the file and must land at a block boundary in the deck, so
    // they get the same treatment as a measurement — a worker, not the callback, not the client thread.
    {
        let prepared = graph_loads.clone();
        let published_to = deck_updates.clone();
        let store = store.clone();
        let grids = Arc::clone(&grids);
        let rates = Arc::clone(&rates);
        let paths = Arc::clone(&paths);
        std::thread::spawn(move || {
            while let Ok(request) = grid_edits_rx.recv() {
                let deck = request.deck;
                let current = grids
                    .lock()
                    .ok()
                    .and_then(|held| held.get(deck.index()).cloned().flatten());
                // No grid is a state a track is in, not a reason to ignore the DJ: the edit that places
                // a beat or names a tempo makes the grid it needs (B02). A track nobody has measured
                // would otherwise have no way to a grid at all.
                let rate = rates
                    .lock()
                    .ok()
                    .and_then(|held| held.get(deck.index()).copied())
                    .unwrap_or(0);
                let Some(edited) = dj_engine::edit_grid(current.as_ref(), &request.edit, rate)
                else {
                    eprintln!("dj-engine: the edit does not make a grid; nothing changed");
                    continue;
                };
                // The grid in the map is published a snapshot late, and two edits arriving together —
                // a tapped tempo and the shift that puts it on the taps — would otherwise compose from
                // the same stale copy and the last one would win. The worker owns the grid it edits.
                if let Ok(mut held) = grids.lock()
                    && let Some(slot) = held.get_mut(deck.index())
                {
                    *slot = Some(edited.clone());
                }
                // The deck's own file first: a client that reconnected knows no path at all, and an
                // edit that named the wrong file would write the grid beside the wrong track.
                let path = std::path::PathBuf::from(
                    paths
                        .lock()
                        .ok()
                        .and_then(|held| held.get(deck.index()).cloned().flatten())
                        .unwrap_or_else(|| request.path.clone()),
                );
                if let Err(message) =
                    dj_engine::meta::write_beatgrid(&store, volume(), root(), &path, &edited, true)
                {
                    eprintln!("dj-engine: {message}");
                }
                let _ = published_to.send(Published::Grid {
                    deck,
                    grid: Box::new(edited.clone()),
                });
                // The lock is the DJ's switch and nothing else. A grid that is already saved is what
                // beats the next automatic pass, and it says so by existing; pinning the track here would
                // grey the Measure button the moment a DJ used it once.
                let _ = prepared.send(GraphLoad::SetGrid {
                    deck,
                    grid: Box::new(edited),
                });
            }
        });
    }

    // The loader: one thread, because a load is a second of work that must not touch the callback.
    let waveform_cache = dj_analysis::WaveformCache::new(peaks_directory());
    // The track cache builds in its own thread.
    //
    // A decode into memory is a whole second pass over the file. Left in the loader, it would make the
    // next load wait for it — a DJ loading two decks would watch the second one sit a file behind. Here
    // the deck starts on the stream and the entry arrives when it arrives.
    let (cache_requests, cache_work) = std::sync::mpsc::channel::<(std::path::PathBuf, DeckId)>();
    {
        let prepared = graph_loads.clone();
        std::thread::spawn(move || {
            // The budget is the owner's setting (`pcm_cache.whole_track_gb`): a gigabyte holds four
            // ten-minute tracks at 48 kHz, and a larger number only waits to be evicted.
            //
            // f32 and not i16: a cached track must play the same samples the stream played. Memory and
            // not disk: the point is a read path that cannot be late.
            let mut cache = if cache_mb == 0 {
                None
            } else {
                match dj_codec::PcmCache::new(
                    std::env::temp_dir().join("hauler-dj-pcm"),
                    dj_codec::CacheConfig {
                        enabled: true,
                        lossless: cache_lossless,
                        storage: dj_codec::CacheStorage::Memory,
                        max_bytes: cache_mb.saturating_mul(1024 * 1024),
                        sample_format: dj_codec::PcmSampleFormat::F32,
                    },
                ) {
                    Ok(cache) => Some(cache),
                    Err(error) => {
                        eprintln!("dj-engine: no track cache: {error}");
                        None
                    }
                }
            };
            match cache.as_ref() {
                Some(_) => eprintln!(
                    "dj-engine: track cache {cache_mb} MB in memory, lossless {}",
                    if cache_lossless { "on" } else { "off" }
                ),
                None => eprintln!("dj-engine: track cache off"),
            }
            while let Ok((path, deck)) = cache_work.recv() {
                let Some(store) = cache.as_mut() else {
                    continue;
                };
                if let Some((source, _key)) = dj_engine::cached_source(&path, store) {
                    let _ = prepared.send(GraphLoad::CacheReady { deck, path, source });
                }
            }
        });
    }
    {
        let prepared = graph_loads.clone();
        let updates = deck_updates.clone();
        let load_cells = Arc::clone(&cells);
        std::thread::spawn(move || {
            while let Ok(request) = load_requests_rx.recv() {
                dj_engine::log::info(format!(
                    "load arrived for {:?}: {}",
                    request.deck, request.path
                ));
                let _ = updates.send(Published::Deck(Box::new(DeckUpdate {
                    deck: request.deck,
                    static_info: DeckStatic {
                        deck: request.deck,
                        load: dj_domain::DeckLoad::Loading { progress: 0.0 },
                        track: None,
                        total_frames: None,
                        waveform: None,
                        auto_gain: 1.0,
                        auto_gain_db: None,
                        analyzing: false,
                        analysis_lock: false,
                        grid: None,
                        source_rate: 0,
                        path: Some(request.path.clone()),
                    },
                })));
                let path = std::path::PathBuf::from(&request.path);
                // Stream first, cache behind: a ten-minute file is two decodes, and the deck should
                // not wait for the second one to start playing. The entry is handed over at a block
                // boundary once it exists (GraphLoad::CacheReady).
                // What the file already says about its key saves a whole decode below: a track with a
                // stored key is not measured again.
                // One read of the record answers three of the four decodes a load would otherwise pay for: the
                // key, the loudness correction and the grid.
                let record = store
                    .load(volume(), root(), &path)
                    .ok()
                    .map(|loaded| loaded.meta);
                let known = dj_engine::KnownMeta {
                    key: record.as_ref().and_then(|meta| meta.key.clone()),
                    auto_gain_db: record
                        .as_ref()
                        .and_then(|meta| meta.auto_gain_db)
                        .map(|db| db as f32),
                    grid: record.as_ref().is_some_and(|meta| meta.beatgrid.is_some()),
                };
                match prepare_track_with_key(&path, rate, 2.0, auto_gain_share, None, &known).map(
                    |mut prepared| {
                        if let Some(problem) =
                            dj_engine::meta::restore(&mut prepared, &store, volume(), root())
                        {
                            eprintln!("dj-engine: {problem}");
                        }
                        // The grid is worth keeping: measuring it is a decode of the whole file, and the
                        // next load should not pay for it again. A stored grid is left alone by the write.
                        if let Some(grid) = prepared.grid.as_ref() {
                            match dj_engine::meta::write_beatgrid(
                                &store,
                                volume(),
                                root(),
                                &path,
                                grid,
                                false,
                            ) {
                                Ok(true) => {}
                                Ok(false) => {}
                                Err(message) => eprintln!("dj-engine: {message}"),
                            }
                        }
                        // Names for a collection list: the tags are read here anyway, and an import from
                        // before they travelled in the record has none of them. The same read gives the
                        // genres, and they go into the index the browser reads.
                        let mut index = dj_library::Index::open_default().ok();
                        dj_engine::meta::write_names(
                            &store,
                            volume(),
                            root(),
                            &path,
                            false,
                            index.as_mut(),
                        );
                        // The key is worth keeping for the same reason: measuring it is a decode of the
                        // whole file. A key already in the file is left alone by the write.
                        // The correction, for the same reason as the key: measuring it is a full decode of the
                        // file, and a track loaded twice should pay for it once.
                        if let Some(correction) = prepared.auto_gain_correction_db
                            && let Err(message) = dj_engine::meta::write_auto_gain(
                                &store,
                                volume(),
                                root(),
                                &path,
                                correction,
                                false,
                            )
                        {
                            eprintln!("dj-engine: {message}");
                        }
                        if let Some(key) =
                            prepared.track.as_ref().and_then(|track| track.key.clone())
                            && let Err(message) = dj_engine::meta::write_key(
                                &store,
                                volume(),
                                root(),
                                &path,
                                &key,
                                false,
                            )
                        {
                            eprintln!("dj-engine: {message}");
                        }
                        prepared
                    },
                ) {
                    Ok(mut prepared_track) => {
                        dj_engine::log::info(format!(
                            "load prepared {}: {} frames at {} Hz",
                            request.path,
                            prepared_track.static_info.total_frames.unwrap_or(0),
                            prepared_track.static_info.source_rate
                        ));
                        prepared_track.deck = request.deck;
                        prepared_track.static_info.deck = request.deck;
                        // Second decode of the file, off the audio path: this is why a load is a
                        // worker job and not something the callback can do.
                        match dj_engine::prepare_waveform(&path, &waveform_cache) {
                            Ok(waveform) => prepared_track.static_info.waveform = Some(waveform),
                            Err(message) => {
                                eprintln!("dj-engine: no waveform for {}: {message}", request.path)
                            }
                        }
                        // The attach goes first: whoever hears "Ready" may act on it at once, and
                        // a cue sent before the deck holds the track would belong to nothing.
                        let static_info = prepared_track.static_info.clone();
                        dj_engine::log::info(format!(
                            "load attaching {} to {:?}",
                            request.path, request.deck
                        ));
                        let _ = prepared.send(GraphLoad::Attach(Box::new(prepared_track)));
                        // Now the same file again, into memory. The deck is already playing from the
                        // stream, so this second decode costs the loader thread and nobody's ears.
                        // Ask for the entry; the cache thread answers with CacheReady when it has one.
                        let _ = cache_requests.send((path.clone(), request.deck));
                        // The word "ready" goes out with a number: until the graph has applied as many attaches
                        // as there were publications, the snapshot keeps saying `Loading`.
                        load_cells.note_publication(request.deck);
                        let _ = updates.send(Published::Deck(Box::new(DeckUpdate {
                            deck: request.deck,
                            static_info: static_info.clone(),
                        })));
                    }
                    Err(message) => {
                        // The same line the console gets, and it is the one that explains a deck standing
                        // still: a path the engine could not open says why.
                        dj_engine::log::info(format!("cannot load {}: {message}", request.path));
                        let _ = updates.send(Published::Deck(Box::new(DeckUpdate {
                            deck: request.deck,
                            static_info: DeckStatic {
                                deck: request.deck,
                                load: dj_domain::DeckLoad::Failed { message },
                                track: None,
                                total_frames: None,
                                waveform: None,
                                auto_gain: 1.0,
                                auto_gain_db: None,
                                analyzing: false,
                                analysis_lock: false,
                                grid: None,
                                source_rate: 0,
                                path: None,
                            },
                        })));
                    }
                }
            }
        });
    }

    // Reversed windows for streamed tracks: the deck asks from the audio callback, this thread does the
    // decoding and the reading, and the answer arrives at a block boundary like any other load.
    let (reverse_requests, reverse_work) = std::sync::mpsc::channel::<dj_engine::ReverseRequest>();
    {
        let prepared = graph_loads.clone();
        std::thread::spawn(move || {
            let mut decoder: Option<(std::path::PathBuf, dj_codec::StreamDecoder)> = None;
            while let Ok(request) = reverse_work.recv() {
                let fresh = !matches!(decoder.as_ref(), Some((path, _)) if path == &request.path);
                if fresh {
                    match dj_codec::StreamDecoder::open(&request.path) {
                        Ok(open) => decoder = Some((request.path.clone(), open)),
                        Err(error) => {
                            eprintln!("dj-engine: reversed window: {error}");
                            decoder = None;
                            continue;
                        }
                    }
                }
                let Some((_, open)) = decoder.as_mut() else {
                    continue;
                };
                if let Err(error) = open.seek(request.start) {
                    eprintln!("dj-engine: reversed window seek: {error}");
                    continue;
                }
                let frames = usize::try_from(request.frames).unwrap_or(0);
                let mut material = vec![0.0f32; frames * 2];
                let mut read = 0usize;
                while read < frames {
                    match open.read_frames(&mut material[read * 2..]) {
                        Ok(0) => break,
                        Ok(got) => read += got,
                        Err(_) => break,
                    }
                }
                if read == 0 {
                    continue;
                }
                material.truncate(read * 2);
                let _ = prepared.send(dj_engine::GraphLoad::ReverseWindow {
                    deck: request.deck,
                    path: request.path,
                    start: request.start,
                    material,
                });
            }
        });
    }

    let graph = EngineGraph::new(
        decks,
        mixer,
        submissions_rx,
        Arc::clone(&cells),
        block,
        graph_loads_rx,
    )
    .with_meta_sink(meta_edits)
    .with_reverse_worker(reverse_requests)
    .with_cache_budget(cache_mb.saturating_mul(1024 * 1024));

    // One bag of run parameters for both paths.
    let options = RunOptions {
        block,
        rate,
        seconds,
        state_hz,
        published: Arc::clone(&published),
    };

    let outcome = match (device, device_channels) {
        (Some(requested), Some(channels)) => {
            let outcome = run_device(graph, &requested, channels, &options, chosen_routes, &stop);
            remove_socket(&socket);
            outcome
        }
        _ => {
            let _ = null_sink;
            run_null(graph, &options, &stop);
            // The listener lives in the accept thread, which is parked in accept() and therefore
            // never runs its own destructor; the socket has to be removed here or the next engine
            // sees a live path with nobody behind it.
            remove_socket(&socket);
            Ok(())
        }
    };
    // The publisher closes the open play when it sees the stop flag; give it a couple of ticks to hand
    // that play to the writer, whose own exit waits for its queue.
    std::thread::sleep(Duration::from_millis(200));
    outcome
}

/// Ask for SIGINT and SIGTERM to set the stop flag instead of killing the process outright.
///
/// A hard kill would skip the clean shutdown: the stream would stop mid-block and the socket would
/// be left behind for the next engine to clean up.
/// Keep one thread whose signal mask never gets blocked by PipeWire.
///
/// PipeWire's thread loop blocks signals in the thread that creates it — ours — so SIGINT stays
/// pending there for ever and Ctrl-C does nothing, handler or not. A thread created before the
/// stream starts keeps the mask it was born with, and the kernel delivers the signal there, where
/// the handler sets the stop flag.
fn keep_a_signal_receiving_thread(stop: &Arc<AtomicBool>) {
    let stop = Arc::clone(stop);
    std::thread::Builder::new()
        .name("dj-signals".to_string())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        })
        .ok();
}

fn register_signals(stop: &Arc<AtomicBool>) -> Result<(), String> {
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(stop))
            .map_err(|err| format!("cannot install a signal handler: {err}"))?;
    }
    Ok(())
}

/// What a run needs beyond its graph: the block, the rate, how long to run, and how often to publish.
///
/// One struct rather than nine arguments: the two paths (device and null sink) share them, and the
/// list had already grown past what a call site can read.
struct RunOptions {
    block: usize,
    rate: u32,
    seconds: Option<f64>,
    state_hz: u32,
    published: Arc<std::sync::atomic::AtomicU64>,
}

/// The sockets the settings file named: the main output's pair and the headphones' pair.
///
/// A struct rather than two options in one parameter: a pair of optional pairs reads as noise at the call site, and
/// clippy rightly calls that type complex.
#[derive(Debug, Clone)]
struct ChosenRoutes {
    master: Option<(usize, usize)>,
    cue: Option<(usize, usize)>,
    /// Which arrangement the headphones are in: the cue's channels are a consequence of it.
    mode: dj_config::HeadphoneMode,
    /// The switch the window shows as "None".
    cue_enabled: bool,
    /// The interface the headphones are on, when the mode puts them on another one.
    cue_device: Option<String>,
}

/// The device path: the graph lives inside the callback, which is the only place it may run.
fn run_device(
    graph: EngineGraph,
    requested: &str,
    channels: usize,
    options: &RunOptions,
    pairs: ChosenRoutes,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    let RunOptions {
        block,
        rate,
        seconds,
        state_hz,
        published,
    } = options;
    let (block, rate, seconds, state_hz) = (*block, *rate, *seconds, *state_hz);
    // The DJ's own channels, where they fit: `audio.master` for the main output and `audio.headphones.pair` for the
    // cue. The plan refuses a pair that reaches past the device and keeps its own.
    // Which channels the cue gets is a decision, not a number: `auto` takes the first free pair of the main interface,
    // `same_device` the DJ's pair when it is free, `device` would need a second stream this build does not open.
    let master_pair = pairs.master.unwrap_or((0, 1));
    // On a second interface the cue does not go into this device at all: it leaves through the ring, and the other stream
    // plays it at its own rate. The two clocks are held together by `cue::CuePull`, not by a shared buffer size.
    let mut cue_ring: Option<std::sync::Arc<dj_audio_io::cue::CueRing>> = None;
    let mut cue_output: Option<dj_audio_io::CueOutput> = None;
    let mut cue_telemetry: Option<std::sync::Arc<dj_audio_io::cue::CueTelemetry>> = None;
    let cue = match pairs.mode {
        dj_config::HeadphoneMode::Auto => {
            dj_audio_io::cue_channels(channels, master_pair, None, pairs.cue_enabled)
        }
        dj_config::HeadphoneMode::SameDevice => {
            dj_audio_io::cue_channels(channels, master_pair, pairs.cue, pairs.cue_enabled)
        }
        dj_config::HeadphoneMode::Device => {
            let name = pairs.cue_device.clone().unwrap_or_default();
            if !pairs.cue_enabled || name.trim().is_empty() {
                dj_engine::log::info(
                    "monitoring is on another device but none is named: the cue stays off",
                );
                None
            } else {
                // What rate the other interface likes, not what this one asked for: they are separate clocks.
                let cue_rate = dj_audio_io::resolve_device(Some(&name))
                    .map(|shape| shape.sample_rate)
                    .unwrap_or(rate);
                let ring =
                    dj_audio_io::cue::CueRing::new(usize::try_from(rate.max(1)).unwrap_or(48_000));
                let pair = pairs.cue.unwrap_or((0, 1));
                let telemetry = std::sync::Arc::new(dj_audio_io::cue::CueTelemetry::default());
                match dj_audio_io::CueOutput::start(
                    &name,
                    cue_rate,
                    block,
                    pair,
                    dj_audio_io::cue::CuePull::new(std::sync::Arc::clone(&ring), rate, cue_rate)
                        .with_telemetry(std::sync::Arc::clone(&telemetry)),
                ) {
                    Ok(output) => {
                        dj_engine::log::info(format!(
                            "monitoring on {} at {cue_rate} Hz, cue on {}/{}, the main device at {rate} Hz",
                            output.device(),
                            pair.0,
                            pair.1
                        ));
                        cue_ring = Some(ring);
                        cue_output = Some(output);
                        cue_telemetry = Some(telemetry);
                        None
                    }
                    Err(error) => {
                        dj_engine::log::warn(format!(
                            "the headphone interface did not open: {error}"
                        ));
                        None
                    }
                }
            }
        }
    };
    match (pairs.mode, cue) {
        (dj_config::HeadphoneMode::Device, _) => {}
        (_, Some((left, right))) => {
            dj_engine::log::info(format!("monitoring on channels {left}/{right}"))
        }
        (_, None) => {
            dj_engine::log::info("no spare channels for monitoring: the cue is mixed but silent")
        }
    }
    let mut plan = dj_audio_io::RoutePlan::with_pairs(channels, pairs.master, cue);
    // The decision is final:  fills an empty cue with the plan.s own pair, which is right for a device with
    // four channels and wrong when the cue left for another interface, or when the DJ said None.
    plan.cue = cue;
    let config = dj_audio_io::LiveConfig {
        device: Some(requested.to_string()),
        sample_rate: rate,
        block_frames: block,
        seconds: f64::INFINITY,
        thresholds: dj_audio_io::Thresholds::default(),
    };

    let output =
        dj_audio_io::LiveOutput::start_with_cue(&config, plan, Box::new(graph), cue_ring.clone())
            .map_err(|err| format!("cannot open {requested:?}: {err}"))?;
    let report = output.snapshot();
    // What the backend actually gave: a request for 256 frames can come back as 480, and a rate the device does
    // not support is quietly replaced. Silence and crackle both start here.
    dj_engine::log::info(format!(
        "device opened: {:?}, {} channel(s), {} Hz, {} frames per callback",
        report.device, report.channels, report.sample_rate, report.frames_per_callback
    ));
    // A stream that opened is not a stream that runs: the callback may never be called, and then the deck plays to
    // nobody. Two seconds is enough to tell the two apart.
    let mut settled = output.snapshot();
    for _ in 0..20 {
        if settled.callbacks > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        settled = output.snapshot();
    }
    if settled.callbacks == 0 {
        dj_engine::log::error(
            "the device opened but no audio callback arrived in 2 s: the deck will be silent",
        );
    } else {
        dj_engine::log::info(format!(
            "device running: {} callbacks, {} frames per callback, {} Hz, late {}, errors {}",
            settled.callbacks,
            settled.frames_per_callback,
            settled.sample_rate,
            settled.late_callbacks,
            settled.errors
        ));
    }
    eprintln!(
        "dj-engine: playing on {:?} — {} channel(s), master {}/{} cue {}",
        report.device,
        report.channels,
        plan.master.0,
        plan.master.1,
        plan.cue
            .map(|(left, right)| format!("{left}/{right}"))
            .unwrap_or_else(|| "none".to_string())
    );

    // A bounded run is how the owner checks the device without listening for ever: the same
    // counters the probe reports, printed as JSON for the evidence directory.
    if let Some(seconds) = seconds {
        let report = output.finish(seconds);
        let json = serde_json::json!({
            "device": report.device,
            "channels": report.channels,
            "sample_rate": report.sample_rate,
            "block_frames_requested": report.block_frames_requested,
            "block_honoured": report.honoured_block(),
            "frames_per_callback": report.frames_per_callback,
            "callbacks": report.callbacks,
            "frames": report.frames,
            "audio_seconds": report.audio_seconds,
            "late_callbacks": report.late_callbacks,
            "early_callbacks": report.early_callbacks,
            "errors": report.errors,
            "interval_p50_ms": report.interval_p50_ms,
            "interval_p99_ms": report.interval_p99_ms,
            "interval_max_ms": report.interval_max_ms,
            "render_p50_us": report.render_p50_us,
            "render_p99_us": report.render_p99_us,
            "render_max_us": report.render_max_us,
            "render_budget": report.render_budget,
            "state_hz_requested": state_hz,
            "snapshots_published": published.load(Ordering::Relaxed),
            "stalled_blocks": report.stalled_blocks,
            "off_cpu_max_us": report.off_cpu_max_us,
            "peaks": report.peaks,
            "route": {
                "master": [plan.master.0, plan.master.1],
                "cue": plan.cue.map(|(left, right)| [left, right]),
            },
        });
        println!(
            "{}",
            serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_string())
        );
        // A run that ends on its own clock still has to say what the two clocks did.
        log_cue_link(cue_telemetry.as_ref(), rate);
        drop(cue_output);
        return Ok(());
    }

    // PipeWire's thread loop takes the signal when the stream starts, so the handlers are installed
    // again now that it exists; otherwise Ctrl-C reaches nobody and the engine runs until killed.
    register_signals(stop)?;

    // The engine owns the device for as long as it lives; the stream stops when this value drops.
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let report = output.snapshot();
    eprintln!(
        "dj-engine: stopping — {} callbacks, {} frames, late {}, render p99 {:.1} us",
        report.callbacks, report.frames, report.late_callbacks, report.render_p99_us
    );
    log_cue_link(cue_telemetry.as_ref(), rate);
    drop(cue_output);
    Ok(())
}

/// What the two clocks did with each other: the cushion, the lean of the read pointer, and how often the cue had to hold
/// a sample. A large or growing correction is a pair of crystals worth telling the DJ about.
fn log_cue_link(telemetry: Option<&std::sync::Arc<dj_audio_io::cue::CueTelemetry>>, rate: u32) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let stats = telemetry.read();
    let produced = telemetry.produced().max(1);
    // More than a percent of held frames is not a cushion problem: the other device is not keeping the same time at all,
    // and the DJ has to hear about it before the set rather than during it.
    if stats.starved * 100 > produced {
        dj_engine::log::warn(format!(
            "cue link: {} of {} frames were held — the headphone interface is not keeping time",
            stats.starved, produced
        ));
    }
    dj_engine::log::info(format!(
        "cue link: {:.0} ms of cushion, correction {:+.1} ppm, {} held, {} dropped, {} frames played",
        stats.fill as f64 * 1000.0 / f64::from(rate.max(1)),
        stats.correction * 1_000_000.0,
        stats.starved,
        stats.dropped,
        telemetry.produced()
    ));
}

/// The null path: same graph, same rules, paced by a clock instead of a sound card.
fn run_null(mut graph: EngineGraph, options: &RunOptions, stop: &Arc<AtomicBool>) {
    let RunOptions {
        block,
        rate,
        seconds,
        state_hz,
        published,
    } = options;
    let (block, rate, seconds, state_hz) = (*block, *rate, *seconds, *state_hz);
    let period = Duration::from_secs_f64(f64::from(block as u32) / f64::from(rate));
    let mut master = vec![0.0f32; block * 2];
    let mut cue = vec![0.0f32; block * 2];
    let started = Instant::now();
    let mut next_block = started;
    let mut callbacks: u64 = 0;
    let limit = seconds.map(|seconds| (seconds * f64::from(rate)) as u64);

    while !stop.load(Ordering::Relaxed) {
        graph.render_pair(&mut master, &mut cue, block);
        callbacks += 1;
        if let Some(limit) = limit
            && graph.clock() >= limit
        {
            let deck_peaks: Vec<f32> = graph
                .decks()
                .iter()
                .map(|deck| deck.levels().peak_left.max(deck.levels().peak_right))
                .collect();
            let json = serde_json::json!({
                "mode": "null_sink",
                "deck_peaks": deck_peaks,
                "callbacks": callbacks,
                "frames": graph.clock(),
                "audio_seconds": graph.clock() as f64 / f64::from(rate),
                "sample_rate": rate,
                "block_frames": block,
                // The rate the wire actually carried, next to the one that was asked for: the
                // figure is what the UI can project between, not what the flag promised.
                "state_hz_requested": state_hz,
                "snapshots_published": published.load(Ordering::Relaxed),
            });
            println!(
                "{}",
                serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_string())
            );
            return;
        }
        next_block += period;
        let now = Instant::now();
        if next_block > now {
            std::thread::sleep(next_block - now);
        } else {
            next_block = now;
        }
    }
}

/// Where waveform envelopes are cached: the user's cache directory, never the collection.
fn peaks_directory() -> PathBuf {
    let base = dj_paths::cache_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("hauler-dj").join("peaks")
}

/// Remove the socket file the accept thread still holds open. A pipe name has no file.
fn remove_socket(path: &std::path::Path) {
    let _ = dj_bridge::transport::remove_stale(path);
}

/// Where the engine listens by default: the platform decides, and `dj-paths` is the one place that knows.
///
/// A socket file on Linux, a named pipe on Windows. Spelling this out here instead was a real fault: on Windows
/// the engine still built an XDG path, while the shell connected to a pipe name, and the two never met.
fn default_socket() -> PathBuf {
    dj_paths::default_endpoint()
}

/// A request to edit the grid of the deck's track (DJP-SYNC-003).
#[derive(Debug, Clone)]
struct GridEditRequest {
    /// Deck whose grid is being edited.
    deck: dj_domain::DeckId,
    /// The file the deck holds.
    path: String,
    /// What the DJ asked for.
    edit: dj_domain::GridEdit,
}

/// The per-deck flags a client connection reads.
///
/// `playing` is refreshed by the publisher from the snapshot; `stopping` says a stop was accepted and
/// the graph has not been seen standing still yet. A load is refused for a deck that is playing, and
/// the second flag is what keeps that refusal from outliving the stop it belongs to.
#[derive(Clone)]
struct DeckFlags {
    /// True while the deck is playing, as last published.
    playing: Arc<Vec<AtomicBool>>,
    /// True between an accepted stop and the publish that shows the deck standing still.
    stopping: Arc<Vec<AtomicBool>>,
}

/// The background work a client connection may ask for: a decode, or a second measurement.
/// Whether this deck's grid is locked.
fn locked(locks: &Arc<Mutex<Vec<bool>>>, deck: dj_domain::DeckId) -> bool {
    locks
        .lock()
        .map(|held| held.get(deck.index()).copied().unwrap_or(false))
        .unwrap_or(false)
}

#[derive(Clone)]
struct Workers {
    /// The publisher's cells: the import worker publishes its progress through them.
    cells: Arc<StateCells>,
    /// The metadata store: the import worker writes the analysis of what it placed through it.
    store: dj_library::Store,
    /// Files to measure that are not on a deck (DJP-MEDIA-004).
    files: Sender<AnalysisBatch>,
    /// Stops the running batch between files.
    analysis_cancel: Arc<AtomicBool>,
    /// Whether a batch is being measured right now: a second press queues behind it, and the journal says so.
    analysis_running: Arc<AtomicBool>,
    /// Decodes a track and prepares a deck.
    loads: Sender<LoadRequest>,
    /// Measures a deck's tempo again (B02).
    analyses: Sender<AnalysisRequest>,
    /// Edits the grid of what a deck plays (DJP-SYNC-003).
    grids: Sender<GridEditRequest>,
    /// Which decks hold a locked grid: the command handler reads it, the grid worker keeps it current.
    locks: Arc<Mutex<Vec<bool>>>,
    /// The file each deck holds: a command that names no path is answered with this one, because the
    /// deck is what plays and a client that reconnected never loaded anything.
    paths: Arc<Mutex<Vec<Option<String>>>>,
    /// The channel the decks publish on: a lock written here reaches the snapshot through it.
    published: Sender<dj_engine::Published>,
}

/// Files to measure off the deck: a batch, and whether to spread it over threads.
#[derive(Debug, Clone)]
struct AnalysisBatch {
    /// The files to measure.
    paths: Vec<String>,
    /// Spread the batch over several workers.
    parallel: bool,
}

/// A request to measure a deck's tempo again (B02).
#[derive(Debug, Clone)]
struct AnalysisRequest {
    /// Deck whose grid will be replaced.
    deck: dj_domain::DeckId,
    /// The file the deck holds.
    path: String,
}

fn serve_client(
    connection: dj_bridge::Connection,
    outgoing: Receiver<ServerMessage>,
    alerts: Sender<ServerMessage>,
    submissions: Sender<Submission>,
    work: Workers,
    stop: Arc<AtomicBool>,
    flags: DeckFlags,
) {
    let cells = Arc::clone(&work.cells);
    let (mut reader, mut writer) = connection.split();

    // The writer only writes: it blocks on the channel, never polls with a deadline.
    std::thread::spawn(move || {
        for message in outgoing {
            if writer.send(&message).is_err() {
                return;
            }
        }
    });

    // The import this client started, if any: the answer channel and the stop flag. One batch at a
    // time, because a second batch would ask questions the DJ never sees.
    let mut import: Option<dj_engine::ImportJob> = None;

    // The reader only reads, so a quiet client cannot starve the snapshot stream.
    let alerts = alerts;
    // How many messages of each kind arrive, once a second: a hand on the platter sends a seek every block, and
    // a log line per message would bury everything else. The count tells the same story at trace level.
    let mut seen: std::collections::BTreeMap<&'static str, u32> = std::collections::BTreeMap::new();
    let mut counting_since = std::time::Instant::now();
    loop {
        if counting_since.elapsed() >= Duration::from_secs(1) && !seen.is_empty() {
            let parts: Vec<String> = seen
                .iter()
                .map(|(name, count)| format!("{name} x{count}"))
                .collect();
            dj_engine::log::trace(format!("client in 1 s: {}", parts.join(", ")));
            seen.clear();
            counting_since = std::time::Instant::now();
        }
        match reader.recv_client() {
            Ok(ClientMessage::Command(command)) => {
                let kind = dj_engine::log::command_name(&command.command).unwrap_or("deck");
                *seen.entry(kind).or_insert(0) += 1;
                // One line per command the DJ asked for; the ones a moving hand sends every block are named
                // `None` and stay out of the file.
                dj_engine::log::debug(format!(
                    "command #{}: {}",
                    command.id,
                    dj_engine::log::command_name(&command.command).unwrap_or("a deck command")
                ));
                // A load is answered here, not by the audio callback: it is `Accepted`, and the
                // deck becomes playable only when the state says `Ready`.
                match dj_engine::route(&command) {
                    dj_engine::Routed::Load(request) => {
                        // A deck that is playing refuses a load: cutting the track the room is
                        // dancing to is the one mistake a DJ never forgives. The playing flag comes
                        // from the published state, which is at most one publish interval old.
                        // A deck that is playing refuses a load: cutting the track the room is
                        // dancing to is the one mistake a DJ never forgives. The flag comes from the
                        // publisher, so it is at most one publish interval old.
                        let busy = flags
                            .playing
                            .get(request.deck.index())
                            .is_some_and(|flag| flag.load(Ordering::Relaxed))
                            && !flags
                                .stopping
                                .get(request.deck.index())
                                .is_some_and(|flag| flag.load(Ordering::Relaxed));
                        if busy {
                            let ack =
                                dj_domain::Ack::rejected(command.id, dj_domain::RejectReason::Busy);
                            if alerts.send(ServerMessage::Ack(ack)).is_err() {
                                return;
                            }
                            continue;
                        }
                        // The path is read before the request is handed over: the request is moved into the
                        // queue, and a line about a load that says nothing about which file is a line wasted.
                        let loading_path = request.path.clone();
                        let ack = match work.loads.send(request) {
                            Ok(()) => {
                                dj_engine::log::info(format!(
                                    "load #{} handed to the loader: {loading_path}",
                                    command.id
                                ));
                                dj_domain::Ack::accepted(command.id)
                            }
                            Err(_) => {
                                dj_engine::log::info(format!(
                                    "load #{} refused: the loader is gone",
                                    command.id
                                ));
                                dj_domain::Ack::rejected(
                                    command.id,
                                    dj_domain::RejectReason::InvalidValue,
                                )
                            }
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::Playlists { command_id, op } => {
                        // The playlists live in the index and their mirror on disk: both are written here,
                        // off the audio path, and the tree this answers with is published so the panel
                        // redraws (DJP-MEDIA-004).
                        let playlist_cells = Arc::clone(&cells);
                        let playlist_alerts = alerts.clone();
                        std::thread::spawn(move || {
                            let outcome = dj_engine::playlists::default_mirror()
                                .ok_or_else(|| "no data folder for the playlist mirror".to_string())
                                .and_then(|mirror| {
                                    dj_library::Index::open_default()
                                        .map_err(|error| error.to_string())
                                        .and_then(|mut index| {
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|elapsed| elapsed.as_millis() as i64)
                                                .unwrap_or(0);
                                            dj_engine::playlists::apply(
                                                &mut index, &mirror, &op, now,
                                            )
                                        })
                                });
                            let ack = match outcome {
                                Ok(tree) => {
                                    eprintln!(
                                        "dj-engine: playlists now hold {} folders, {} playlists",
                                        tree.folders.len(),
                                        tree.playlists.len()
                                    );
                                    playlist_cells.set_playlists(tree);
                                    dj_domain::Ack::applied(command_id, 0)
                                }
                                Err(message) => {
                                    eprintln!("dj-engine: playlists: {message}");
                                    dj_domain::Ack::rejected(
                                        command_id,
                                        dj_domain::RejectReason::InvalidValue,
                                    )
                                }
                            };
                            let _ = playlist_alerts.send(ServerMessage::Ack(ack));
                        });
                    }
                    dj_engine::Routed::PlaylistQuery { command_id, query } => {
                        let playlist_cells = Arc::clone(&cells);
                        let playlist_alerts = alerts.clone();
                        std::thread::spawn(move || {
                            // A reader, not a writer: the list of a playlist must not queue behind an import.
                            let outcome = dj_library::Index::reader_default()
                                .map_err(|error| error.to_string())
                                .and_then(|index| {
                                    dj_engine::playlists::page(&index, query.playlist, &query)
                                });
                            let ack = match outcome {
                                Ok(page) => {
                                    eprintln!(
                                        "dj-engine: playlist {} answered with {} rows",
                                        query.playlist, page.total
                                    );
                                    playlist_cells.set_playlist_page(page);
                                    dj_domain::Ack::applied(command_id, 0)
                                }
                                Err(message) => {
                                    eprintln!("dj-engine: playlists: {message}");
                                    dj_domain::Ack::rejected(
                                        command_id,
                                        dj_domain::RejectReason::InvalidValue,
                                    )
                                }
                            };
                            let _ = playlist_alerts.send(ServerMessage::Ack(ack));
                        });
                    }
                    dj_engine::Routed::Query { command_id, query } => {
                        // The index lives on disk: opening it runs the schema and its migrations, which is
                        // a write. That must not happen in the loop that acks deck commands — a DJ who
                        // opens the collection should not find his deck unresponsive. So the query gets
                        // its own thread, and the answer comes back the way every other answer does:
                        // through the cells and the alerts.
                        let query_cells = Arc::clone(&cells);
                        let query_alerts = alerts.clone();
                        std::thread::spawn(move || {
                            // The index is a cache over the records: a query that cannot open it is an
                            // empty answer, not a failure — the browser then shows what it walked itself.
                            if query.genres_only {
                                // The boxes ask the genre table itself: a count over a page of two
                                // hundred rows is a count over a page, not over the collection.
                                let folders: Vec<std::path::PathBuf> =
                                    query.roots.iter().map(std::path::PathBuf::from).collect();
                                let genres = dj_library::Index::reader_default()
                                    .and_then(|index| index.genre_list_in(&folders))
                                    .map(|list| {
                                        list.into_iter()
                                            .map(|(name, count)| dj_domain::GenreCount {
                                                name,
                                                count: usize::try_from(count).unwrap_or(usize::MAX),
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default();
                                dj_engine::log::info(format!(
                                    "genre question answered with {} genres over {:?}",
                                    genres.len(),
                                    folders
                                ));
                                query_cells.set_genres(genres);
                                let _ = query_alerts.send(ServerMessage::Ack(
                                    dj_domain::Ack::applied(command_id, 0),
                                ));
                                return;
                            }
                            let folders: Vec<std::path::PathBuf> =
                                query.roots.iter().map(std::path::PathBuf::from).collect();
                            let asked = dj_engine::page_query(&query);
                            // Reading the index is cheap and never takes a write lock: the importer and
                            // the walks are writing, and a question that queues behind them is a list that
                            // never appears.
                            let mut page = dj_library::Index::reader_default()
                                .and_then(|index| index.page(&asked))
                                .unwrap_or_default();
                            if page.total == 0 && !folders.is_empty() {
                                // Nothing for these folders, and the folders hold records: the cache is made
                                // from them. The whole index need not be empty — a database that holds rows
                                // under a folder the DJ no longer opens is still a database this root is
                                // missing from. A running import is the one thing to keep out of the way of:
                                // it writes the same file.
                                let importing = query_cells
                                    .import()
                                    .map(|state| state.running)
                                    .unwrap_or(false);
                                if !importing {
                                    match dj_library::Index::open_default() {
                                        Ok(mut index) => match index.fill_from(&folders) {
                                            Ok(report) => eprintln!(
                                                "dj-engine: the index had nothing for {} folders, filled \
                                                 {} rows",
                                                folders.len(),
                                                report.tracks
                                            ),
                                            Err(error) => eprintln!(
                                                "dj-engine: the index could not be filled: {error}"
                                            ),
                                        },
                                        Err(error) => eprintln!(
                                            "dj-engine: the index could not be opened to fill it: {error}"
                                        ),
                                    }
                                    if let Ok(index) = dj_library::Index::reader_default() {
                                        match index.page(&asked) {
                                            Ok(fresh) => page = fresh,
                                            // A page that cannot be read is not an empty collection: the
                                            // error has to be said out loud, or the list blames the folders.
                                            Err(error) => {
                                                eprintln!(
                                                    "dj-engine: the page could not be read: {error}"
                                                )
                                            }
                                        }
                                        if page.total == 0 {
                                            // Still nothing for these folders after a fill: the rows are in
                                            // the index and the question cannot see them, so the two are put
                                            // side by side — asked folders first, stored paths after.
                                            dj_engine::log::info(format!("asked {folders:?}"));
                                            if let Ok(paths) = index.sample_paths(3) {
                                                dj_engine::log::info(format!("stored {paths:?}"));
                                            }
                                        }
                                    }
                                }
                            }
                            let page = dj_domain::CollectionPage {
                                id: query.id,
                                total: page.total,
                                rows: page.rows.iter().map(dj_engine::collection_row).collect(),
                            };
                            // The question and the answer, side by side: a list that stays empty is either a
                            // question that never arrived or an answer with nothing in it.
                            dj_engine::log::info(format!(
                                "page question {} over {:?}: {} rows of {}",
                                query.id,
                                query.roots,
                                page.rows.len(),
                                page.total
                            ));
                            {
                                let sample: Vec<String> = page
                                    .rows
                                    .iter()
                                    .take(3)
                                    .map(|row| row.path.clone())
                                    .collect();
                                dj_engine::log::info(format!(
                                    "page {} gives {:?}",
                                    query.id, sample
                                ));
                            }
                            query_cells.set_collection(page);
                            let _ = query_alerts
                                .send(ServerMessage::Ack(dj_domain::Ack::applied(command_id, 0)));
                        });
                        continue;
                    }
                    dj_engine::Routed::Import {
                        command_id,
                        paths,
                        root,
                        parallel,
                    } => {
                        // One batch at a time: two workers would ask the DJ two questions at once.
                        if import
                            .as_ref()
                            .is_some_and(|job| job.running.load(Ordering::Relaxed))
                        {
                            let ack =
                                dj_domain::Ack::rejected(command_id, dj_domain::RejectReason::Busy);
                            if alerts.send(ServerMessage::Ack(ack)).is_err() {
                                return;
                            }
                            continue;
                        }
                        eprintln!("dj-engine: import asked for {} files", paths.len());
                        let (answers_tx, answers_rx) = std::sync::mpsc::channel();
                        let cancel = Arc::new(AtomicBool::new(false));
                        // The same flag reaches the analysis an import triggers: a file's measuring is the
                        // long part of a batch, and it must stop when the DJ says stop.
                        let analyze_cancel = Arc::clone(&cancel);
                        let running = Arc::new(AtomicBool::new(true));
                        import = Some(dj_engine::ImportJob {
                            answers: answers_tx,
                            cancel: Arc::clone(&cancel),
                            running: Arc::clone(&running),
                        });
                        let worker_cells = Arc::clone(&cells);
                        let request = dj_engine::ImportRequest {
                            paths,
                            root,
                            parallel,
                        };
                        let worker_store = work.store.clone();
                        std::thread::spawn(move || {
                            // The index is a cache: if it cannot be opened the import still runs, and
                            // the rebuild fills it later.
                            let mut index = dj_library::Index::open_default().ok();
                            dj_engine::run_import(
                                request,
                                worker_cells,
                                answers_rx,
                                cancel,
                                running,
                                std::sync::Arc::new(move |path: &std::path::Path| {
                                    // The flag is read between files, and a file's analysis is the long
                                    // part: a batch asked to stop must not finish the measuring it is in.
                                    if analyze_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                        eprintln!(
                                            "dj-engine: analysis skipped, the batch was stopped"
                                        );
                                        return;
                                    }
                                    // `self::root`, not `root`: the import request's collection root
                                    // shadows the function of that name in this arm.
                                    dj_engine::meta::analyze_file(
                                        &worker_store,
                                        volume(),
                                        self::root(),
                                        path,
                                        false,
                                    );
                                }),
                                index.as_mut(),
                            );
                        });
                        if alerts
                            .send(ServerMessage::Ack(dj_domain::Ack::accepted(command_id)))
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::Model { command_id, op } => {
                        // Модель — это сеть и диск, не аудио: команда принимается сразу, работа идёт
                        // на своём потоке, а состояние едет в снапшоте.
                        let mut status = dj_engine::beatmodel::installed_status();
                        status.busy = true;
                        status.error.clear();
                        cells.set_model(status);
                        if alerts
                            .send(ServerMessage::Ack(dj_domain::Ack::accepted(command_id)))
                            .is_err()
                        {
                            return;
                        }
                        let worker_cells = Arc::clone(&cells);
                        std::thread::spawn(move || {
                            let outcome = match &op {
                                dj_domain::ModelOp::Download { variant } => {
                                    dj_engine::beatmodel::download(variant)
                                }
                                dj_domain::ModelOp::Delete { variant } => {
                                    dj_engine::beatmodel::remove(variant)
                                }
                            };
                            match outcome {
                                Ok(status) => worker_cells.set_model(status),
                                Err(error) => {
                                    let mut status = dj_engine::beatmodel::installed_status();
                                    status.error = error;
                                    worker_cells.set_model(status);
                                }
                            }
                        });
                        continue;
                    }
                    dj_engine::Routed::Rescan { command_id, roots } => {
                        // The walk belongs to the engine: the browser must not touch the disks for a list.
                        // Every file is asked for what it knows — a record with a grid and a key is left
                        // alone, a file without one is measured and gets a record. Then the index is
                        // rebuilt from the records, which is where the genre counts come from.
                        let worker_cells = Arc::clone(&cells);
                        let worker_store = work.store.clone();
                        std::thread::spawn(move || {
                            let folders: Vec<std::path::PathBuf> =
                                roots.iter().map(std::path::PathBuf::from).collect();
                            let files = dj_library::index::audio_files_under(&folders);
                            let mut index = dj_library::Index::open_default().ok();
                            let mut state = dj_domain::ImportState {
                                total: files.len(),
                                done: 0,
                                placed: 0,
                                skipped: 0,
                                failed: 0,
                                running: true,
                                current: None,
                                question: None,
                            };
                            worker_cells.set_import(Some(state.clone()));
                            for file in &files {
                                state.current = Some(file.display().to_string());
                                worker_cells.set_import(Some(state.clone()));
                                // The tags are read again from the file — names and genres — because that is
                                // what "rescan" means: a record whose tags are stale makes the genre boxes
                                // and the columns lie.
                                let _ = dj_engine::meta::write_names(
                                    &worker_store,
                                    volume(),
                                    self::root(),
                                    file,
                                    true,
                                    index.as_mut(),
                                );
                                // `force = false`: a record that already has a grid and a key is not
                                // measured again — only what is missing is measured.
                                if dj_engine::meta::analyze_file(
                                    &worker_store,
                                    volume(),
                                    self::root(),
                                    file,
                                    false,
                                ) {
                                    state.placed += 1;
                                } else {
                                    state.skipped += 1;
                                }
                                state.done += 1;
                                worker_cells.set_import(Some(state.clone()));
                            }
                            // The records are the source: the index is rebuilt from them, so the genre
                            // counts and the list both follow what the files say now.
                            if let Some(mut index) = index {
                                let _ = index.rebuild_from(&folders);
                            }
                            state.running = false;
                            state.current = None;
                            worker_cells.set_import(Some(state));
                        });
                        if alerts
                            .send(ServerMessage::Ack(dj_domain::Ack::accepted(command_id)))
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::ImportCancel { command_id } => {
                        // An answer, not a gesture: a cancel with no batch to stop is refused, because a
                        // panel that shows "stopping" for a batch that already ended is lying.
                        let asked = import
                            .as_ref()
                            .map(|job| job.cancel.store(true, Ordering::Relaxed))
                            .is_some();
                        // A cancel that leaves no trace in the log cannot be argued with: either the
                        // command arrives and the batch ignores it, or it never arrives at all.
                        eprintln!("dj-engine: cancel asked (batch present: {asked})");
                        let ack = if asked {
                            dj_domain::Ack::accepted(command_id)
                        } else {
                            dj_domain::Ack::rejected(
                                command_id,
                                dj_domain::RejectReason::InvalidValue,
                            )
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::ImportAnswer { command_id, answer } => {
                        // An answer nobody asked for is refused: the DJ's click must not be applied to
                        // the next question, which he has not seen.
                        let sent = import
                            .as_ref()
                            .is_some_and(|job| job.answers.send(answer).is_ok());
                        let ack = if sent {
                            dj_domain::Ack::accepted(command_id)
                        } else {
                            dj_domain::Ack::rejected(
                                command_id,
                                dj_domain::RejectReason::InvalidValue,
                            )
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::Eject { deck } => {
                        // The graph clears the deck at its next block boundary; the loader owns no
                        // state for it, so nothing is queued here.
                        let (reply, answer) = std::sync::mpsc::channel();
                        if submissions
                            .send(Submission {
                                command: command.clone(),
                                reply,
                            })
                            .is_err()
                        {
                            return;
                        }
                        if let Ok(ack) = answer.recv_timeout(Duration::from_millis(500)) {
                            // The deck is emptied at the next block boundary, and the flag that refuses a
                            // load is refreshed from the published snapshot — for one publish it still
                            // says "playing", and a load sent right after the eject is refused for a
                            // state that no longer exists. An accepted eject is a deck with nothing in
                            // it: say so now, and let the publisher clear the mark once it sees the
                            // deck standing still.
                            if matches!(ack.status, dj_domain::AckStatus::Applied { .. }) {
                                if let Some(flag) = flags.playing.get(deck.index()) {
                                    flag.store(false, Ordering::Relaxed);
                                }
                                if let Some(flag) = flags.stopping.get(deck.index()) {
                                    flag.store(true, Ordering::Relaxed);
                                }
                            }
                            if alerts.send(ServerMessage::Ack(ack)).is_err() {
                                return;
                            }
                        }
                        continue;
                    }
                    dj_engine::Routed::SetAnalysisLock {
                        deck,
                        path,
                        enabled,
                    } => {
                        // Which file the lock belongs to: the deck's own first, then the path the client
                        // sent. A client that reconnected to a running engine names nothing, and a lock
                        // written beside "" was refused with "No such file or directory" (owner's report,
                        // 2026-11-19).
                        let deck_path = work
                            .paths
                            .lock()
                            .ok()
                            .and_then(|held| held.get(deck.index()).cloned().flatten());
                        let Some(target) =
                            dj_engine::meta::analysis_lock_target(deck_path.as_deref(), &path)
                        else {
                            eprintln!(
                                "dj-engine: the lock was not written: the deck holds no file and the command named none"
                            );
                            let ack = dj_domain::Ack::rejected(
                                command.id,
                                dj_domain::RejectReason::InvalidValue,
                            );
                            if alerts.send(ServerMessage::Ack(ack)).is_err() {
                                return;
                            }
                            continue;
                        };
                        // The record beside the track and its mirror first, then the collection: the flag
                        // travels with the file, and the list draws it in its own cell.
                        let store = dj_library::Store::with_default_mirror();
                        let written = dj_engine::meta::set_analysis_lock(
                            &store,
                            volume(),
                            root(),
                            &target,
                            enabled,
                        );
                        let ack = match written {
                            Ok(_) => {
                                // The row in the collection is drawn from the index, so a lock that is written
                                // beside the file and not into the index is a lock the DJ never sees. Both, and
                                // the journal says which of them failed.
                                match (
                                    store.load(volume(), root(), &target),
                                    dj_library::Index::open_default(),
                                ) {
                                    (Ok(loaded), Ok(mut index)) => {
                                        let row = dj_engine::meta::row_of(&loaded.meta, &target);
                                        match index.upsert(&row) {
                                            Ok(()) => dj_engine::log::info(format!(
                                                "lock {enabled} written for {} and into the index",
                                                target.display()
                                            )),
                                            Err(error) => dj_engine::log::error(format!(
                                                "lock {enabled} written for {} but the index refused the row: {error}",
                                                target.display()
                                            )),
                                        }
                                    }
                                    (Err(error), _) => dj_engine::log::error(format!(
                                        "lock {enabled} written for {} but the record did not load back: {error}",
                                        target.display()
                                    )),
                                    (_, Err(error)) => dj_engine::log::error(format!(
                                        "lock {enabled} written for {} but the index did not open: {error}",
                                        target.display()
                                    )),
                                }
                                let _ = work
                                    .published
                                    .send(dj_engine::Published::AnalysisLock { deck, enabled });
                                if let Ok(mut held) = work.locks.lock()
                                    && let Some(slot) = held.get_mut(deck.index())
                                {
                                    *slot = enabled;
                                }
                                dj_domain::Ack::accepted(command.id)
                            }
                            Err(error) => {
                                eprintln!("dj-engine: the lock was not written: {error}");
                                dj_domain::Ack::rejected(
                                    command.id,
                                    dj_domain::RejectReason::InvalidValue,
                                )
                            }
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::EditGrid { deck, path, edit } => {
                        if locked(&work.locks, deck) {
                            // A locked grid is the DJ's decision: the edit is refused, not undone later.
                            let ack = dj_domain::Ack::rejected(
                                command.id,
                                dj_domain::RejectReason::InvalidValue,
                            );
                            if alerts.send(ServerMessage::Ack(ack)).is_err() {
                                return;
                            }
                            continue;
                        }
                        // Like a measurement, answered at once and carried out where the file can be
                        // written: an edit is a decision about the track, and the deck shows it when the
                        // worker has written and published it.
                        let ack = match work.grids.send(GridEditRequest { deck, path, edit }) {
                            Ok(()) => dj_domain::Ack::accepted(command.id),
                            Err(_) => dj_domain::Ack::rejected(
                                command.id,
                                dj_domain::RejectReason::InvalidValue,
                            ),
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::AnalyzeFile { path } => {
                        // Answered at once: the measurement takes as long as a decode, and the new
                        // numbers reach the file and the next listing, not this snapshot.
                        let ack = match work.files.send(AnalysisBatch {
                            paths: vec![path],
                            parallel: false,
                        }) {
                            Ok(()) => dj_domain::Ack::accepted(command.id),
                            Err(_) => dj_domain::Ack::rejected(
                                command.id,
                                dj_domain::RejectReason::InvalidValue,
                            ),
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::AnalyzeFiles { paths, parallel } => {
                        // A batch is measured one file at a time: a second press waits for the first, and without a
                        // line of its own that wait looks like a hang (owner's report, Windows, 2026-11-20).
                        if work.analysis_running.load(Ordering::Relaxed) {
                            dj_engine::log::info(format!(
                                "analysis queued behind the batch in hand: {} file(s)",
                                paths.len()
                            ));
                        }
                        let ack = match work.files.send(AnalysisBatch { paths, parallel }) {
                            Ok(()) => dj_domain::Ack::accepted(command.id),
                            Err(_) => dj_domain::Ack::rejected(
                                command.id,
                                dj_domain::RejectReason::InvalidValue,
                            ),
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::AnalyzeStop => {
                        // Between files, never inside one: a measurement in hand is finished, and its
                        // answer is still written.
                        work.analysis_cancel.store(true, Ordering::Relaxed);
                        let ack = dj_domain::Ack::accepted(command.id);
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::Analyze { deck, path } => {
                        if locked(&work.locks, deck) {
                            let ack = dj_domain::Ack::rejected(
                                command.id,
                                dj_domain::RejectReason::InvalidValue,
                            );
                            if alerts.send(ServerMessage::Ack(ack)).is_err() {
                                return;
                            }
                            continue;
                        }
                        // Answered at once, not when the answer is ready: the measurement takes as long
                        // as a decode, and the state publishes the new grid when it lands.
                        let ack = match work.analyses.send(AnalysisRequest { deck, path }) {
                            Ok(()) => dj_domain::Ack::accepted(command.id),
                            Err(_) => dj_domain::Ack::rejected(
                                command.id,
                                dj_domain::RejectReason::InvalidValue,
                            ),
                        };
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                        continue;
                    }
                    dj_engine::Routed::Render => {}
                }
                let (reply, answer) = std::sync::mpsc::channel();
                // Read before the command is handed over: after the send it belongs to the graph.
                let stop_of = stopping_deck(&command.command);
                let start_of = starting_deck(&command.command);
                if submissions.send(Submission { command, reply }).is_err() {
                    return;
                }
                match answer.recv_timeout(Duration::from_millis(500)) {
                    Ok(ack) => {
                        // A deck that is playing refuses a load, and that refusal reads a flag the
                        // publisher refreshes at display rate. A stop is applied by the audio thread at
                        // the next block boundary, so between this ack and that block the flag still
                        // says "playing" — and a load sent right after a stop is refused as Busy. The
                        // DJ presses stop, then loads, and the load is dropped for a state that no
                        // longer exists. The command was accepted, so the deck is going to stop: say so
                        // now. The publisher corrects the flag either way, so a refusal cannot leak.
                        if matches!(ack.status, dj_domain::AckStatus::Applied { .. })
                            && let Some(deck) = stop_of
                        {
                            if let Some(flag) = flags.playing.get(deck.index()) {
                                flag.store(false, Ordering::Relaxed);
                            }
                            if let Some(flag) = flags.stopping.get(deck.index()) {
                                flag.store(true, Ordering::Relaxed);
                            }
                        }
                        // A deck told to play again is a deck a load would cut: the window closes here.
                        if matches!(ack.status, dj_domain::AckStatus::Applied { .. })
                            && let Some(deck) = start_of
                            && let Some(flag) = flags.stopping.get(deck.index())
                        {
                            flag.store(false, Ordering::Relaxed);
                        }
                        if alerts.send(ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        // An engine that is behind is not a client's fault: report the wait instead
                        // of dropping the command silently.
                        let _ = alerts.send(ServerMessage::Error {
                            message: "the engine did not acknowledge the command in time"
                                .to_string(),
                        });
                    }
                }
            }
            // A client can always stop the engine, whatever the signal mask looks like.
            Ok(ClientMessage::Quit) => {
                eprintln!("dj-engine: stop requested over the bridge");
                stop.store(true, Ordering::Relaxed);
                return;
            }
            Ok(_) => {}
            Err(_) => return,
        }
    }
}
