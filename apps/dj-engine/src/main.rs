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
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dj_bridge::{BridgeServer, ClientMessage, ServerMessage};
use dj_deck::{Deck, DeckSource};
use dj_domain::{Ack, DeckId, Snapshot};
use dj_engine::{apply, snapshot};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const USAGE: &str = "\
dj-engine — the audio engine process

Usage:
  dj-engine [--socket PATH] [--null-sink] [--device SUBSTR] [--block N] [--rate HZ]

Defaults: socket $XDG_RUNTIME_DIR/hauler-dj-engine.sock, null sink, 48 kHz, block 256.
The null sink generates a tone instead of opening a device, so the command path can be
exercised anywhere.

Exit codes: 0 stopped by a signal, 2 bad usage, 3 socket or device error.
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

/// What a client thread hands to the render loop.
struct Submission {
    command: dj_domain::ScheduledCommand,
    reply: Sender<Ack>,
}

/// State the render loop publishes and the client threads attach to.
#[derive(Default)]
struct Published {
    snapshot: Option<Snapshot>,
    subscribers: Vec<Sender<ServerMessage>>,
}

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
            other => return Err(format!("unknown option {other:?}")),
        }
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
    for id in [DeckId::A, DeckId::B] {
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
                assign: CrossAssign::Thru,
                cue: false,
            },
        );
    }

    let published = Arc::new(Mutex::new(Published::default()));
    let (submissions, submissions_rx) = std::sync::mpsc::channel::<Submission>();

    // Client threads: they never touch a deck, only the channel.
    {
        let published = Arc::clone(&published);
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
                if let Ok(mut state) = published.lock() {
                    if let Some(snapshot) = &state.snapshot {
                        let _ = outgoing.send(ServerMessage::Snapshot(Box::new(snapshot.clone())));
                    }
                    state.subscribers.push(outgoing);
                }
                let submissions = submissions.clone();
                std::thread::spawn(move || serve_client(connection, receiver, submissions));
            }
        });
    }

    if null_sink || device.is_none() {
        render_null(
            &mut decks,
            &mut mixer,
            rate,
            block,
            &published,
            &submissions_rx,
        );
        return Ok(());
    }

    // A real device needs the graph inside the callback; the owner of that path is A05/A06 next
    // step, so this binary refuses rather than pretending.
    Err(format!(
        "device output {device:?} is not wired into the engine yet; use --null-sink"
    ))
}

fn default_socket() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(base).join("hauler-dj-engine.sock")
}

fn serve_client(
    mut connection: dj_bridge::Connection,
    outgoing: Receiver<ServerMessage>,
    submissions: Sender<Submission>,
) {
    loop {
        match outgoing.recv_timeout(Duration::from_millis(5)) {
            Ok(message) => {
                if connection.send(&message).is_err() {
                    return;
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        match connection.recv() {
            Ok(ClientMessage::Command(command)) => {
                let (reply, answer) = std::sync::mpsc::channel();
                if submissions.send(Submission { command, reply }).is_err() {
                    return;
                }
                match answer.recv_timeout(Duration::from_secs(1)) {
                    Ok(ack) => {
                        if connection.send(&ServerMessage::Ack(ack)).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = connection.send(&ServerMessage::Error {
                            message: "the render loop did not answer".to_string(),
                        });
                        return;
                    }
                }
            }
            Ok(ClientMessage::Ping) => {
                if connection.send(&ServerMessage::Pong).is_err() {
                    return;
                }
            }
            Ok(ClientMessage::Subscribe) | Ok(ClientMessage::Hello { .. }) => {}
            Err(_) => return,
        }
    }
}

/// Render against a clock instead of a device.
fn render_null(
    decks: &mut [Deck],
    mixer: &mut Mixer,
    rate: u32,
    block: usize,
    published: &Arc<Mutex<Published>>,
    submissions: &Receiver<Submission>,
) {
    let period = Duration::from_secs_f64(f64::from(block as u32) / f64::from(rate));
    let mut clock: u64 = 0;
    let mut revision: u64 = 0;
    let mut scratch = vec![0.0f32; block * 2];
    let started = Instant::now();
    let mut next_block = started;
    // Snapshots go out at display rate, not at block rate: a UI cannot use 750 of them a second,
    // and flooding the socket would bury the acks a client is waiting for.
    let mut last_publish: Option<Instant> = None;
    let publish_every = Duration::from_millis(50);

    loop {
        // Commands first, so a command at this sample is applied at this sample.
        while let Ok(submission) = submissions.try_recv() {
            let ack = apply(decks, mixer, &submission.command, clock);
            let _ = submission.reply.send(ack);
        }

        mixer.begin_block(block);
        for (index, deck) in decks.iter_mut().enumerate() {
            deck.render(&mut scratch);
            mixer.mix_channel(index, &scratch);
        }
        mixer.finish_block();
        clock += block as u64;

        let now = Instant::now();
        // Publish at display rate. The first version compared against "the previous publish", which
        // is true on every block after the first: it sent ~750 snapshots a second, always left a
        // message queued for the client writer, and the reader never reached the commands waiting
        // on the socket. Acks were never produced.
        let due = last_publish.is_none_or(|last| now.duration_since(last) >= publish_every);
        if due {
            revision += 1;
            last_publish = Some(now);
        }
        if due && let Ok(mut state) = published.lock() {
            let snapshot = snapshot(decks, mixer, revision, clock, rate, 1);
            state.snapshot = Some(snapshot.clone());
            state.subscribers.retain(|subscriber| {
                subscriber
                    .send(ServerMessage::Snapshot(Box::new(snapshot.clone())))
                    .is_ok()
            });
        }

        next_block += period;
        let now = Instant::now();
        if next_block > now {
            std::thread::sleep(next_block - now);
        } else {
            // Keep the clock honest instead of spinning after a stall.
            next_block = now;
        }
    }
}
