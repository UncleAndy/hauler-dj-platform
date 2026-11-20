// The engine's own log: a file next to the collection, so a machine without a console still has evidence.
//
// Levels are chosen by `HAULER_LOG` (`error`, `warn`, `info`, `debug`, `trace`; `info` when unset). The launcher on
// Windows sets `trace`, because a DJ who reports "it does not work" cannot also be asked to reproduce it with a
// debugger attached. Every line goes to stderr as well, so a terminal that is there sees the same thing.
//
// The file is rotated at a fixed size: a session can run for hours at trace, and a log that fills the disk is its
// own incident.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

/// How much detail the log keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Something failed.
    Error = 0,
    /// Something was refused, fell back, or is missing.
    Warn = 1,
    /// The ordinary story: commands, clients, loads.
    Info = 2,
    /// The details worth having when something is wrong: paths, counts, timings.
    Debug = 3,
    /// Everything, including what runs every block.
    Trace = 4,
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

/// Read the level from the environment once, at startup.
/// A panic is a fault, and a fault that prints into a console nobody sees is a fault without a trace. Everything
/// the engine panics on — a worker thread, a scope, the audio path — is written into the same journal as the rest,
/// with the place it happened.
pub fn capture_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let place = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown place".to_string());
        let message = if let Some(text) = info.payload().downcast_ref::<&str>() {
            (*text).to_string()
        } else if let Some(text) = info.payload().downcast_ref::<String>() {
            text.clone()
        } else {
            "a panic without a message".to_string()
        };
        error(format!("panic at {place}: {message}"));
        previous(info);
    }));
}

pub fn init() {
    let level = match std::env::var("HAULER_LOG")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "error" => Level::Error,
        "warn" | "warning" => Level::Warn,
        "debug" => Level::Debug,
        "trace" => Level::Trace,
        _ => Level::Info,
    };
    LEVEL.store(level as u8, Ordering::Relaxed);
}

fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

/// Where the engine writes its log.
pub fn log_path() -> Option<std::path::PathBuf> {
    dj_paths::data_dir().map(|base| base.join("hauler-dj").join("logs").join("dj-engine.log"))
}

/// Rotate the file when it grows past this: one previous file is kept, and the old one is dropped.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

fn write(level: Level, message: &str) {
    if !enabled(level) {
        return;
    }
    let tag = match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN",
        Level::Info => "info",
        Level::Debug => "debug",
        Level::Trace => "trace",
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    let line = format!("{stamp} {tag} dj-engine: {message}");
    eprintln!("{line}");
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > MAX_BYTES
    {
        let previous = path.with_extension("log.1");
        let _ = std::fs::rename(&path, &previous);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// One line at `info`: the ordinary story.
pub fn info(message: impl AsRef<str>) {
    write(Level::Info, message.as_ref());
}

/// One line at `warn`: refused, fell back, or missing.
pub fn warn(message: impl AsRef<str>) {
    write(Level::Warn, message.as_ref());
}

/// One line at `error`.
pub fn error(message: impl AsRef<str>) {
    write(Level::Error, message.as_ref());
}

/// One line at `debug`: details worth having when something is wrong.
pub fn debug(message: impl AsRef<str>) {
    write(Level::Debug, message.as_ref());
}

/// One line at `trace`: everything, including what runs every block.
pub fn trace(message: impl AsRef<str>) {
    write(Level::Trace, message.as_ref());
}

/// The environment the engine woke up in: everything a reader needs to tell one installation from another.
///
/// No secrets are read here: these are folders and switches, not credentials.
pub fn startup_report(args: &[String]) {
    let exe = std::env::current_exe().unwrap_or_default();
    let built = std::fs::metadata(&exe)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    info(format!(
        "engine {} from {} (file time {built})",
        env!("CARGO_PKG_VERSION"),
        exe.display()
    ));
    info(format!(
        "host: {} {}, pid {}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::process::id()
    ));
    info(format!(
        "args: {}",
        if args.is_empty() {
            "(none)".to_string()
        } else {
            args.iter()
                .map(|arg| format!("{arg:?}"))
                .collect::<Vec<_>>()
                .join(" ")
        }
    ));
    if let Ok(cwd) = std::env::current_dir() {
        info(format!("working directory: {}", cwd.display()));
    }
    for name in [
        "APPDATA",
        "LOCALAPPDATA",
        "USERPROFILE",
        "USERNAME",
        "HOME",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        "ORT_DYLIB_PATH",
        "HAULER_LOG",
        "TMP",
        "TEMP",
    ] {
        if let Ok(value) = std::env::var(name) {
            info(format!("env {name}={value}"));
        }
    }
    for (label, path) in [
        ("data", dj_paths::data_dir()),
        ("config", dj_paths::config_dir()),
        ("cache", dj_paths::cache_dir()),
    ] {
        match path {
            Some(path) => info(format!("{label} directory: {}", path.display())),
            None => warn(format!("{label} directory: unknown on this system")),
        }
    }
    info(format!(
        "endpoint by default: {}",
        dj_paths::default_endpoint().display()
    ));
    if let Some(collection) = dj_paths::collection_root() {
        info(format!("collection by default: {}", collection.display()));
    }
    if let Some(path) = log_path() {
        info(format!("log file: {}", path.display()));
    }
}

/// Which outputs the system offers, and which one the engine took.
///
/// A silent app is often a wrong device, and the list answers the question the console cannot: what was there to
/// choose from.
pub fn output_report() {
    let names = match dj_audio_io::output_devices() {
        Ok(names) => names,
        Err(error) => {
            warn(format!("output devices could not be listed: {error}"));
            return;
        }
    };
    if names.is_empty() {
        warn("the system offers no output device");
        return;
    }
    for (index, name) in names.iter().enumerate() {
        info(format!("output device {index}: {name}"));
    }
}

/// The name of a command worth a line in the log, or `None` for the one the deck sends while a hand moves.
///
/// A scratch or a seek arrives every block; a load or a page question arrives when the DJ asks for something. The
/// log is for the second kind, and a log that fills with the first is a log nobody reads.
pub fn command_name(command: &dj_domain::DjCommand) -> Option<&'static str> {
    use dj_domain::DjCommand;
    match command {
        DjCommand::Load { .. } => Some("Load"),
        DjCommand::Eject { .. } => Some("Eject"),
        DjCommand::Import { .. } => Some("Import"),
        DjCommand::Rescan { .. } => Some("Rescan"),
        DjCommand::ImportCancel => Some("ImportCancel"),
        DjCommand::ImportAnswer { .. } => Some("ImportAnswer"),
        DjCommand::AnalyzeTrack { .. } => Some("AnalyzeTrack"),
        DjCommand::AnalyzeTracks { .. } => Some("AnalyzeTracks"),
        DjCommand::AnalyzeCancel => Some("AnalyzeCancel"),
        DjCommand::AnalyzeTempo { .. } => Some("AnalyzeTempo"),
        DjCommand::SetAnalysisLock { .. } => Some("SetAnalysisLock"),
        DjCommand::EditGrid { .. } => Some("EditGrid"),
        DjCommand::Model { .. } => Some("Model"),
        DjCommand::QueryCollection { .. } => Some("QueryCollection"),
        _ => None,
    }
}
