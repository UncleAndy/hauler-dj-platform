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
use dj_domain::{Ack, DeckId};
use dj_engine::{DeckStatic, StateCells, apply};
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

    // The render thread writes scalars here; nothing in the audio path allocates or locks.
    let cells = Arc::new(StateCells::new());
    let statics = Arc::new(
        decks
            .iter()
            .map(|deck| DeckStatic::from_snapshot(&deck.snapshot()))
            .collect::<Vec<_>>(),
    );
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));
    let (submissions, submissions_rx) = std::sync::mpsc::channel::<Submission>();

    // The publisher owns snapshot assembly: one allocation, at display rate, off the audio path.
    {
        let cells = Arc::clone(&cells);
        let statics = Arc::clone(&statics);
        let subscribers = Arc::clone(&subscribers);
        let publish_every = Duration::from_millis(50);
        std::thread::spawn(move || {
            let mut revision: u64 = 0;
            loop {
                std::thread::sleep(publish_every);
                revision += 1;
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
        let statics = Arc::clone(&statics);
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
                // Greet with the state as it is now, then join the fan-out list.
                let first = cells.read(&statics, rate, epoch, 0);
                let _ = outgoing.send(ServerMessage::Snapshot(Box::new(first)));
                let outgoing_clone = outgoing.clone();
                if let Ok(mut list) = subscribers.lock() {
                    list.push(outgoing);
                }
                let submissions = submissions.clone();
                let alerts = outgoing_clone;
                std::thread::spawn(move || serve_client(connection, receiver, alerts, submissions));
            }
        });
    }

    if null_sink || device.is_none() {
        render_null(&mut decks, &mut mixer, rate, block, &cells, &submissions_rx);
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
    connection: dj_bridge::Connection,
    outgoing: Receiver<ServerMessage>,
    alerts: Sender<ServerMessage>,
    submissions: Sender<Submission>,
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
            Ok(_) => {}
            Err(_) => return,
        }
    }
}

fn render_null(
    decks: &mut [Deck],
    mixer: &mut Mixer,
    rate: u32,
    block: usize,
    cells: &Arc<StateCells>,
    submissions: &Receiver<Submission>,
) {
    let period = Duration::from_secs_f64(f64::from(block as u32) / f64::from(rate));
    let mut clock: u64 = 0;
    let mut scratch = vec![0.0f32; block * 2];
    let started = Instant::now();
    let mut next_block = started;

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

        // Atomic stores only: the audio path does not build a snapshot, take a lock, or allocate.
        cells.publish(decks, mixer, clock);

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
