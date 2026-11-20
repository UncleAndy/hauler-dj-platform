//! The launcher: one action starts the application, and it ends as one application.
//!
//! The engine is an **audio** engine, so it runs in the user's session, as a child of this process. A Windows
//! service cannot do that: session 0 has no access to the user's audio endpoints, and its `%APPDATA%` is not the
//! DJ's. So the engine is a child here, and the launcher is what makes the pair behave like one program:
//!
//! - no console window on Windows (the binary is built for the GUI subsystem when it is a release build);
//! - the engine does not outlive its launcher (Linux: `PR_SET_PDEATHSIG`; Windows: the engine's own idle exit);
//! - a second launch does not start a second engine — it finds the lock taken and only opens the shell;
//! - the engine leaves by itself a grace period after its last client, so closing the shell closes everything
//!   while a shell restarted inside that window is instant.
//!
//! `--engine-only` exists for the checks: it brings the engine up, waits for its endpoint, says `engine ready`
//! and leaves — and the engine then dies with it, which is exactly the property under test.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("dj-launcher: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("dj-launcher: {message}");
            ExitCode::FAILURE
        }
    }
}

/// What the launcher was asked to do.
struct Options {
    engine: Option<PathBuf>,
    ui: Option<PathBuf>,
    timeout: Duration,
    engine_only: bool,
    /// Keeps the launcher (and so its lock and its engine) alive for this long after the engine is ready:
    /// the single-instance case needs a first launcher that is still there when the second one asks.
    hold: Duration,
    verbose: bool,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            engine: None,
            ui: None,
            timeout: Duration::from_secs(20),
            engine_only: false,
            hold: Duration::ZERO,
            verbose: false,
        };
        let mut arguments = arguments.peekable();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--engine" => {
                    options.engine = Some(PathBuf::from(
                        arguments.next().ok_or("--engine needs a path")?,
                    ));
                }
                "--ui" => {
                    options.ui = Some(PathBuf::from(arguments.next().ok_or("--ui needs a path")?));
                }
                "--timeout-ms" => {
                    let value = arguments.next().ok_or("--timeout-ms needs a number")?;
                    let milliseconds: u64 = value
                        .parse()
                        .map_err(|_| format!("--timeout-ms does not read as a number: {value}"))?;
                    options.timeout = Duration::from_millis(milliseconds.max(200));
                }
                "--hold-ms" => {
                    let value = arguments.next().ok_or("--hold-ms needs a number")?;
                    let milliseconds: u64 = value
                        .parse()
                        .map_err(|_| format!("--hold-ms does not read as a number: {value}"))?;
                    options.hold = Duration::from_millis(milliseconds);
                }
                "--engine-only" => options.engine_only = true,
                "--verbose" => options.verbose = true,
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(options)
    }
}

fn run(options: &Options) -> Result<u8, String> {
    let folder = std::env::current_exe()
        .map_err(|error| format!("cannot find my own path: {error}"))?
        .parent()
        .map(Path::to_path_buf)
        .ok_or("my own path has no folder")?;
    let engine = options
        .engine
        .clone()
        .unwrap_or_else(|| folder.join(program("dj-engine")));
    let ui = options
        .ui
        .clone()
        .unwrap_or_else(|| folder.join(program("dj-ui")));
    if !engine.is_file() {
        return Err(format!(
            "no engine at {}: run the launcher from the folder it came in",
            engine.display()
        ));
    }

    let lock = InstanceLock::acquire(&folder);
    let mut engine_process = match &lock {
        InstanceLock::Held(_guard) => {
            say(options, &format!("engine: starting {}", engine.display()));
            let process = start_engine(&engine)?;
            if !wait_for_endpoint(options.timeout) {
                // The engine prints the reason for a failure to its own log; the launcher names the path.
                say(options, "engine: the endpoint never answered");
                return Err(format!(
                    "the engine did not come up within {:?}; its log is {}",
                    options.timeout,
                    log_hint()
                ));
            }
            say(options, "engine ready");
            Some(process)
        }
        InstanceLock::Taken => {
            // Another launcher is running: its engine is the engine, and this process only opens the shell.
            say(
                options,
                "engine: another launcher holds it, opening the shell only",
            );
            None
        }
    };

    if !options.hold.is_zero() {
        std::thread::sleep(options.hold);
    }

    if options.engine_only {
        // Dropping the child is the lifeline: on Linux it dies when this process does (PDEATHSIG), on Windows
        // the engine leaves by its own idle exit. Either way the check can see the engine go.
        drop(engine_process.take());
        return Ok(0);
    }

    if !ui.is_file() {
        return Err(format!(
            "no shell at {}: run the launcher from the folder it came in",
            ui.display()
        ));
    }
    say(options, &format!("shell: starting {}", ui.display()));
    let mut shell = Command::new(&ui)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| format!("the shell did not start: {error}"))?;
    let status = shell
        .wait()
        .map_err(|error| format!("the shell could not be waited for: {error}"))?;
    // The engine leaves by its own idle exit once the shell's connection is gone; nothing to kill here.
    drop(engine_process.take());
    Ok(status.code().unwrap_or(0).clamp(0, 255) as u8)
}

/// The engine, with the lifeline that makes it die with this process on Linux.
fn start_engine(engine: &Path) -> Result<Child, String> {
    let mut command = Command::new(engine);
    // An empty device name asks for the system's own output; the engine plays a generated tone without it, which
    // on a machine with no console looks exactly like "no sound but the meter is lit".
    command.arg("--device").arg("");
    command.stdin(Stdio::null());
    if !cfg!(windows) {
        // The engine's own log is where a fault on a machine we cannot see is found; keep it out of our stderr
        // only when nobody is reading it.
        command.stdout(Stdio::null());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        #[allow(unsafe_code)]
        // SAFETY: `pre_exec` runs between fork and exec; the only call is `prctl`, which is async-signal-safe
        // and touches nothing this process owns.
        unsafe {
            command.pre_exec(|| {
                rustix::process::set_parent_process_death_signal(Some(
                    rustix::process::Signal::TERM,
                ))
                .map_err(std::io::Error::from)
            });
        }
    }
    command
        .spawn()
        .map_err(|error| format!("the engine did not start: {error}"))
}

/// Waits until a client can actually talk to the engine, not merely until the file exists.
///
/// A socket file appears before the engine listens on it, and an endpoint file appears before the port is
/// bound: connecting is the only honest question.
fn wait_for_endpoint(timeout: Duration) -> bool {
    let endpoint = dj_paths::default_endpoint();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if endpoint.exists() && connect_once(&endpoint) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(unix)]
fn connect_once(endpoint: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(endpoint).is_ok()
}

#[cfg(windows)]
fn connect_once(endpoint: &Path) -> bool {
    // On Windows the endpoint file holds `127.0.0.1:port`, written once the listener is bound.
    let Ok(text) = std::fs::read_to_string(endpoint) else {
        return false;
    };
    text.trim()
        .parse::<std::net::SocketAddr>()
        .map(|address| {
            std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
        })
        .unwrap_or(false)
}

/// One launcher at a time, and the second one still opens the shell.
///
/// The lock is a file nobody else may open: `flock` on Linux, and on Windows the file is opened with a share
/// mode of zero, which makes a second open fail the same way. No named mutex, no FFI.
enum InstanceLock {
    Held(LockGuard),
    Taken,
}

impl InstanceLock {
    fn acquire(folder: &Path) -> Self {
        let file = lock_file(folder);
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match LockGuard::open(&file) {
            Some(guard) => Self::Held(guard),
            None => Self::Taken,
        }
    }
}

/// Where the lock lives: the runtime folder when there is one, else the data folder.
fn lock_file(_folder: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(dj_paths::data_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join("hauler-dj").join("launcher.lock")
}

/// The held lock. Dropping it releases the lock and leaves the file (removing it would race the next launcher).
struct LockGuard {
    /// Held for its lifetime: the lock lives exactly as long as this descriptor.
    _file: std::fs::File,
}

impl LockGuard {
    #[cfg(unix)]
    fn open(path: &Path) -> Option<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .ok()?;
        // `flock` is released by the kernel when the descriptor closes, so a killed launcher cannot leave a
        // stale lock behind.
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Some(Self { _file: file }),
            Err(_) => None,
        }
    }

    #[cfg(windows)]
    fn open(path: &Path) -> Option<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        // Share mode zero: a second open of the same file fails while this one is open, which is exactly the
        // named mutex without the FFI.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(path)
            .ok()?;
        Some(Self { _file: file })
    }

    #[cfg(not(any(unix, windows)))]
    fn open(_path: &Path) -> Option<Self> {
        None
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // The descriptor closes with the guard, and with it the lock; the file itself stays, because removing
        // it would race a launcher that has just opened it.
    }
}

/// `dj-engine` or `dj-engine.exe`, depending on the platform.
fn program(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Where the engine writes, for the one message that matters when it does not come up.
fn log_hint() -> String {
    dj_paths::data_dir()
        .map(|folder| {
            folder
                .join("hauler-dj")
                .join("logs")
                .join("dj-engine.log")
                .display()
                .to_string()
        })
        .unwrap_or_else(|| "the engine's log".to_string())
}

fn say(options: &Options, text: &str) {
    if options.verbose {
        println!("dj-launcher: {text}");
    }
}

/// A line from the shell's own output, for the `--engine-only` mode's sibling: nothing, for now.
#[allow(dead_code)]
fn drain(reader: impl BufRead) -> Option<String> {
    BufReader::new(reader).lines().next().and_then(Result::ok)
}
