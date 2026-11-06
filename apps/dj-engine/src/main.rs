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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dj_bridge::{BridgeServer, ClientMessage, ServerMessage};
use dj_deck::{Deck, DeckSource};
use dj_domain::DeckId;
use dj_engine::{
    DeckStatic, DeckUpdate, EngineGraph, GraphLoad, LoadRequest, StateCells, Submission,
    prepare_track,
};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const USAGE: &str = "\
dj-engine — the audio engine process

Usage:
  dj-engine [--socket PATH] [--null-sink] [--device SUBSTR] [--block N] [--rate HZ]
            [--seconds S] [--autoplay] [--state-hz N]
  dj-engine [--socket PATH] --stop      ask a running engine to shut down

Defaults: socket $XDG_RUNTIME_DIR/hauler-dj-engine.sock, null sink, 48 kHz, block 256.
--device SUBSTR opens the real output (substring of the backend name); --null-sink keeps a
generated tone and a clock instead, so the command path works anywhere, including CI.
On a device with four or more outputs master goes to the first pair and cue to the second
(ADR-0005).

--seconds S renders for S seconds, prints the timing report as JSON and exits: the same
counters the probe reports, for the evidence directory.
--autoplay starts every deck playing, so a bounded run actually carries signal. Without it the
decks stay paused and every level reads zero, which says nothing about the output path.
--state-hz N publishes state N times a second (default 20, from display.state_hz in the settings).
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
}

impl ToneSource {
    fn new(sample_rate: u32, frequency: f32, seconds: f64) -> Self {
        Self {
            position: 0,
            frames: (f64::from(sample_rate) * seconds) as u64,
            phase: 0.0,
            step: std::f32::consts::TAU * frequency / sample_rate as f32,
        }
    }
}

impl DeckSource for ToneSource {
    fn sample_rate(&self) -> u32 {
        48_000
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

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("dj-engine: {message}");
            std::process::ExitCode::from(2)
        }
    }
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
    // Settings first, so the flags default to them rather than to numbers scattered in the code.
    let defaults = dj_config::Config::default();
    let mut state_hz = defaults.display.state_hz;

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

    let device_channels = match &device {
        Some(requested) => Some(
            dj_audio_io::output_channels(Some(requested), 4)
                .map_err(|err| format!("cannot open {requested:?}: {err}"))?,
        ),
        None => None,
    };

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
        deck.attach(Box::new(ToneSource::new(rate, 440.0, 600.0)), None);
        deck.set_blocking(false);
        decks.push(deck);
    }
    let mut mixer = Mixer::new(decks.len());
    mixer.prepare(rate, block);
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
    let statics = decks
        .iter()
        .map(|deck| DeckStatic::from_snapshot(&deck.snapshot()))
        .collect::<Vec<_>>();
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));
    let (submissions, submissions_rx) = std::sync::mpsc::channel::<Submission>();
    // Loads never enter the audio callback: clients hand them to a loader thread, which hands a
    // prepared source back to the graph and a new description to the publisher.
    let (load_requests, load_requests_rx) = std::sync::mpsc::channel::<LoadRequest>();
    let (graph_loads, graph_loads_rx) = std::sync::mpsc::channel::<GraphLoad>();
    let (deck_updates, deck_updates_rx) = std::sync::mpsc::channel::<DeckUpdate>();

    // Ctrl-C must stop the engine. PipeWire's thread loop blocks signals in the thread that creates
    // it, so the flag and the receiving thread have to exist before the stream does.
    let stop = Arc::new(AtomicBool::new(false));
    register_signals(&stop)?;
    keep_a_signal_receiving_thread(&stop);

    // The publisher owns snapshot assembly: one allocation, at display rate, off the audio path.
    // The count is kept so a bounded run can report the rate it actually achieved: the setting is
    // only worth having if the wire carries what it promises.
    let published = Arc::new(std::sync::atomic::AtomicU64::new(0));
    {
        let cells = Arc::clone(&cells);
        let published = Arc::clone(&published);
        let subscribers = Arc::clone(&subscribers);
        // The state rate is a parameter (display.state_hz): twenty snapshots a second is the
        // default, and raising it trades IPC for less projection between snapshots.
        let publish_every = Duration::from_micros(1_000_000 / u64::from(state_hz));
        std::thread::spawn(move || {
            let mut revision: u64 = 0;
            let mut statics = statics;
            loop {
                std::thread::sleep(publish_every);
                // A load changes what the deck is; only this thread reads the description, so the
                // audio path never sees a lock.
                while let Ok(update) = deck_updates_rx.try_recv() {
                    if let Some(slot) = statics.iter_mut().find(|entry| entry.deck == update.deck) {
                        *slot = update.static_info;
                    }
                }
                revision += 1;
                published.fetch_add(1, Ordering::Relaxed);
                let snapshot = cells.read(&statics, rate, epoch, revision);
                let message = ServerMessage::Snapshot(Box::new(snapshot));

                if let Ok(mut list) = subscribers.lock() {
                    list.retain(|subscriber| subscriber.send(message.clone()).is_ok());
                }
            }
        });
    }

    // Client threads: they never touch a deck, only the channel.
    {
        let subscribers = Arc::clone(&subscribers);
        let cells = Arc::clone(&cells);
        let loads = load_requests.clone();
        let stop_handle = Arc::clone(&stop);
        std::thread::spawn(move || {
            // Accept forever: a refused connection must not take the engine down with it.
            loop {
                let mut connection = match server.accept() {
                    Ok(connection) => connection,
                    Err(err) => {
                        eprintln!("dj-engine: accept failed: {err}");
                        continue;
                    }
                };
                if let Err(err) = BridgeServer::handshake(&mut connection, epoch) {
                    eprintln!("dj-engine: handshake refused: {err}");
                    continue;
                }
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
                let loads = loads.clone();
                let stop = Arc::clone(&stop_handle);
                std::thread::spawn(move || {
                    serve_client(connection, receiver, alerts, submissions, loads, stop)
                });
            }
        });
    }

    // The loader: one thread, because a load is a second of work that must not touch the callback.
    let waveform_cache = dj_analysis::WaveformCache::new(peaks_directory());
    {
        let prepared = graph_loads.clone();
        let updates = deck_updates.clone();
        std::thread::spawn(move || {
            while let Ok(request) = load_requests_rx.recv() {
                let _ = updates.send(DeckUpdate {
                    deck: request.deck,
                    static_info: DeckStatic {
                        deck: request.deck,
                        load: dj_domain::DeckLoad::Loading { progress: 0.0 },
                        track: None,
                        total_frames: None,
                        waveform: None,
                    },
                });
                let path = std::path::PathBuf::from(&request.path);
                match prepare_track(&path, rate, 2.0) {
                    Ok(mut prepared_track) => {
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
                        let _ = updates.send(DeckUpdate {
                            deck: request.deck,
                            static_info: prepared_track.static_info.clone(),
                        });
                        let _ = prepared.send(GraphLoad::Attach(Box::new(prepared_track)));
                    }
                    Err(message) => {
                        eprintln!("dj-engine: cannot load {}: {message}", request.path);
                        let _ = updates.send(DeckUpdate {
                            deck: request.deck,
                            static_info: DeckStatic {
                                deck: request.deck,
                                load: dj_domain::DeckLoad::Failed { message },
                                track: None,
                                total_frames: None,
                                waveform: None,
                            },
                        });
                    }
                }
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
    );

    // One bag of run parameters for both paths.
    let options = RunOptions {
        block,
        rate,
        seconds,
        state_hz,
        published: Arc::clone(&published),
    };

    match (device, device_channels) {
        (Some(requested), Some(channels)) => {
            let outcome = run_device(graph, &requested, channels, &options, &stop);
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
    }
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

/// The device path: the graph lives inside the callback, which is the only place it may run.
fn run_device(
    graph: EngineGraph,
    requested: &str,
    channels: usize,
    options: &RunOptions,
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
    let plan = dj_audio_io::RoutePlan::for_channels(channels);
    let config = dj_audio_io::LiveConfig {
        device: Some(requested.to_string()),
        sample_rate: rate,
        block_frames: block,
        seconds: f64::INFINITY,
        thresholds: dj_audio_io::Thresholds::default(),
    };

    let output = dj_audio_io::LiveOutput::start(&config, plan, Box::new(graph))
        .map_err(|err| format!("cannot open {requested:?}: {err}"))?;
    let report = output.snapshot();
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
    Ok(())
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
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".cache")
        });
    base.join("hauler-dj").join("peaks")
}

/// Remove the socket file the accept thread still holds open.
fn remove_socket(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

fn default_socket() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(base).join("hauler-dj-engine.sock")
}

fn serve_client(
    connection: dj_bridge::Connection,
    outgoing: Receiver<ServerMessage>,
    alerts: Sender<ServerMessage>,
    submissions: Sender<Submission>,
    loads: Sender<LoadRequest>,
    stop: Arc<AtomicBool>,
) {
    let (mut reader, mut writer) = connection.split();

    // The writer only writes: it blocks on the channel, never polls with a deadline.
    std::thread::spawn(move || {
        for message in outgoing {
            if writer.send(&message).is_err() {
                return;
            }
        }
    });

    // The reader only reads, so a quiet client cannot starve the snapshot stream.
    let alerts = alerts;
    loop {
        match reader.recv_client() {
            Ok(ClientMessage::Command(command)) => {
                // A load is answered here, not by the audio callback: it is `Accepted`, and the
                // deck becomes playable only when the state says `Ready`.
                match dj_engine::route(&command) {
                    dj_engine::Routed::Load(request) => {
                        let ack = match loads.send(request) {
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
                        if let Ok(ack) = answer.recv_timeout(Duration::from_millis(500))
                            && alerts.send(ServerMessage::Ack(ack)).is_err()
                        {
                            return;
                        }
                        let _ = deck;
                        continue;
                    }
                    dj_engine::Routed::Render => {}
                }
                let (reply, answer) = std::sync::mpsc::channel();
                if submissions.send(Submission { command, reply }).is_err() {
                    return;
                }
                match answer.recv_timeout(Duration::from_millis(500)) {
                    Ok(ack) => {
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
