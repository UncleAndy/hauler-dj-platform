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
    let statics = decks
        .iter()
        .map(|deck| DeckStatic::from_snapshot(&deck.snapshot()))
        .collect::<Vec<_>>();
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));
    let (submissions, submissions_rx) = std::sync::mpsc::channel::<Submission>();
    // Requests to measure a tempo again; the worker below owns the answers.
    let (analysis_requests, analysis_requests_rx) = std::sync::mpsc::channel::<AnalysisRequest>();
    // Requests to edit a grid. The grid in force lives with the deck; the publisher hands a copy to
    // this map on every change, and the edit worker is the only one who reads it.
    let (grid_edits, grid_edits_rx) = std::sync::mpsc::channel::<GridEditRequest>();
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
                    match update {
                        Published::Deck(update) => {
                            let deck = update.deck;
                            let grid = update.static_info.grid.clone();
                            let rate = update.static_info.source_rate;
                            if let Some(slot) = statics.iter_mut().find(|entry| entry.deck == deck)
                            {
                                *slot = update.static_info;
                            }
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
                let snapshot = cells.read(&statics, rate, epoch, revision);
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
        let work = Workers {
            loads: load_requests.clone(),
            analyses: analysis_requests.clone(),
            grids: grid_edits.clone(),
        };
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
                match dj_analysis::tempo::analyse(&path) {
                    Ok(Some(found)) => {
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
                    Ok(None) => eprintln!(
                        "dj-engine: no tempo found in {} — the grid is unchanged",
                        request.path
                    ),
                    Err(message) => eprintln!("dj-engine: {message}"),
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
                let Some(current) = current else {
                    eprintln!(
                        "dj-engine: no grid on deck {:?} to edit — the track has none yet",
                        deck
                    );
                    continue;
                };
                let rate = rates
                    .lock()
                    .ok()
                    .and_then(|held| held.get(deck.index()).copied())
                    .unwrap_or(0);
                let Some(edited) = dj_engine::edit_grid(&current, &request.edit, rate) else {
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
        std::thread::spawn(move || {
            while let Ok(request) = load_requests_rx.recv() {
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
                let known_key = store
                    .load(volume(), root(), &path)
                    .ok()
                    .and_then(|loaded| loaded.meta.key);
                match prepare_track_with_key(
                    &path,
                    rate,
                    2.0,
                    auto_gain_share,
                    None,
                    known_key.as_deref(),
                )
                .map(|mut prepared| {
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
                    // The key is worth keeping for the same reason: measuring it is a decode of the
                    // whole file. A key already in the file is left alone by the write.
                    if let Some(key) = prepared.track.as_ref().and_then(|track| track.key.clone())
                        && let Err(message) =
                            dj_engine::meta::write_key(&store, volume(), root(), &path, &key, false)
                    {
                        eprintln!("dj-engine: {message}");
                    }
                    prepared
                }) {
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
                        // The attach goes first: whoever hears "Ready" may act on it at once, and
                        // a cue sent before the deck holds the track would belong to nothing.
                        let static_info = prepared_track.static_info.clone();
                        let _ = prepared.send(GraphLoad::Attach(Box::new(prepared_track)));
                        // Now the same file again, into memory. The deck is already playing from the
                        // stream, so this second decode costs the loader thread and nobody's ears.
                        // Ask for the entry; the cache thread answers with CacheReady when it has one.
                        let _ = cache_requests.send((path.clone(), request.deck));
                        let _ = updates.send(Published::Deck(Box::new(DeckUpdate {
                            deck: request.deck,
                            static_info: static_info.clone(),
                        })));
                    }
                    Err(message) => {
                        eprintln!("dj-engine: cannot load {}: {message}", request.path);
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
#[derive(Clone)]
struct Workers {
    /// Decodes a track and prepares a deck.
    loads: Sender<LoadRequest>,
    /// Measures a deck's tempo again (B02).
    analyses: Sender<AnalysisRequest>,
    /// Edits the grid of what a deck plays (DJP-SYNC-003).
    grids: Sender<GridEditRequest>,
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
                        let ack = match work.loads.send(request) {
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
                    dj_engine::Routed::EditGrid { deck, path, edit } => {
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
                    dj_engine::Routed::Analyze { deck, path } => {
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
