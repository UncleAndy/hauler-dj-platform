//! Monitoring reaches the engine: a cued deck is heard in the headphone bus, the switch works both ways, and the two
//! knobs come back in the snapshot (owner's decisions, 20.11).
//!
//! The graph applies commands at a block boundary, so every test queues its commands first and then renders: a test
//! that waited for an answer without rendering would wait forever, which is exactly what the engine would do.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::Deck;
use dj_domain::{
    Ack, AckStatus, BeatGrid, DeckId, DjCommand, RejectReason, ScheduledCommand, TrackMeta,
};
use dj_engine::{EngineGraph, StateCells, Submission};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

const IDS: [DeckId; 2] = [DeckId::A, DeckId::B];

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav")
}

fn loaded(id: DeckId) -> Deck {
    let path = fixture();
    let prefetcher = Prefetcher::open(
        &path,
        PrefetchConfig {
            window_frames: 48_000,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open {}: {err}", path.display()));
    let grid = BeatGrid::single(120.0, 0).expect("a grid of one tempo");
    let mut deck = Deck::new(id);
    deck.attach(
        Box::new(prefetcher),
        Some(TrackMeta {
            title: String::from("drums"),
            artist: String::new(),
            duration_frames: Some(48_000 * 8),
            bpm: Some(120.0),
            first_beat_frame: Some(0),
            grid: Some(grid),
            key: None,
            analysis_lock: false,
        }),
    );
    deck.set_blocking(false);
    deck.set_gain(1.0);
    deck
}

/// Two decks with the same file; the crossfader is fully to the left, so only deck A can reach the room.
fn graph() -> (EngineGraph, Sender<Submission>, Arc<StateCells>) {
    let decks: Vec<Deck> = IDS.iter().map(|id| loaded(*id)).collect();
    let mut mixer = Mixer::new(IDS.len());
    mixer.prepare(RATE, BLOCK);
    for index in 0..IDS.len() {
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
    dj_engine::assign_crossfader_sides(&mut mixer, &IDS);
    mixer.set_crossfader(0.0);
    let cells = Arc::new(StateCells::new());
    let (submissions, receiver): (Sender<Submission>, Receiver<Submission>) = channel();
    let (_loads, load_receiver) = channel();
    let graph = EngineGraph::new(
        decks,
        mixer,
        receiver,
        Arc::clone(&cells),
        BLOCK,
        load_receiver,
    );
    (graph, submissions, cells)
}

/// Queue a command; the answer arrives when the graph next renders.
fn queue(submissions: &Sender<Submission>, id: u64, command: DjCommand) -> Receiver<Ack> {
    let (reply, answers) = channel();
    submissions
        .send(Submission {
            command: ScheduledCommand {
                id,
                effective_sample: 0,
                command,
            },
            reply,
        })
        .expect("the graph is listening");
    answers
}

/// Render `blocks` callbacks and return the loudest sample each bus carried across all of them.
///
/// Not the last block: at 120 BPM a bar is 2 s, and 42 ms of window is easily the gap between two hits.
fn render(graph: &mut EngineGraph, blocks: usize) -> (f32, f32) {
    let mut master = vec![0.0f32; BLOCK * 2];
    let mut cue = vec![0.0f32; BLOCK * 2];
    let mut last = (0.0f32, 0.0f32);
    for _ in 0..blocks {
        // The prefetcher fills its window on a loader thread: without the sleep the graph runs ahead of the file and
        // renders silence, which is what the first version of this test measured.
        std::thread::sleep(std::time::Duration::from_millis(6));
        graph.render_pair(&mut master, &mut cue, BLOCK);
        last = (
            last.0.max(
                master
                    .iter()
                    .fold(0.0f32, |held, sample| held.max(sample.abs())),
            ),
            last.1.max(
                cue.iter()
                    .fold(0.0f32, |held, sample| held.max(sample.abs())),
            ),
        );
    }
    last
}

fn applied(answers: &Receiver<Ack>, what: &str) -> Ack {
    let ack = answers
        .try_recv()
        .unwrap_or_else(|_| panic!("no answer for {what}"));
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "{what}: {:?}",
        ack.status
    );
    ack
}

#[test]
fn a_cued_deck_is_heard_in_the_headphones_and_not_in_the_room() {
    let (mut graph, submissions, _cells) = graph();
    let mut plays = Vec::new();
    for deck in IDS {
        plays.push(queue(
            &submissions,
            deck.index() as u64 + 1,
            DjCommand::Play {
                deck,
                enabled: true,
            },
        ));
    }
    let cue_on = queue(
        &submissions,
        10,
        DjCommand::SetChannelCue {
            deck: DeckId::B,
            on: true,
        },
    );

    // Warm-up: the prefetcher needs its window before the file reaches the mixer.
    let _ = render(&mut graph, 40);
    let (master, cue) = render(&mut graph, 8);
    for (index, answers) in plays.iter().enumerate() {
        applied(
            answers,
            if index == 0 {
                "deck A plays"
            } else {
                "deck B plays"
            },
        );
    }
    applied(&cue_on, "deck B goes into the headphones");
    assert!(
        master > 0.001,
        "deck A reaches the room, not silence: {master}"
    );
    assert!(
        cue > 0.001,
        "deck B is heard in the headphones, not silence: {cue}"
    );

    // And the switch works the other way: taking the deck out of the headphones silences the cue bus.
    let cue_off = queue(
        &submissions,
        11,
        DjCommand::SetChannelCue {
            deck: DeckId::B,
            on: false,
        },
    );
    // The lookahead line still holds what was cued a moment ago: four blocks let it drain before the measurement,
    // so a tight threshold measures the switch and not the delay.
    let _ = render(&mut graph, 4);
    let (master_after, cue_after) = render(&mut graph, 8);
    applied(&cue_off, "deck B leaves the headphones");
    assert!(
        cue_after < 1e-4,
        "with nothing cued the headphones are silent, not {cue_after}"
    );
    assert!(
        master_after > 0.001,
        "the room still hears deck A: {master_after}"
    );
}

#[test]
fn the_two_headphone_knobs_come_back_in_the_snapshot() {
    let (mut graph, submissions, cells) = graph();
    let gain = queue(&submissions, 1, DjCommand::SetCueGain { linear_gain: 0.5 });
    let mix = queue(&submissions, 2, DjCommand::SetCueMix { mix: 0.25 });
    render(&mut graph, 2);
    applied(&gain, "the headphone volume is applied");
    applied(&mix, "the proportion is applied");
    // The snapshot the shell reads carries them through the published cells.
    let snapshot = cells.read(&[], RATE, 1, 1);
    assert_eq!(
        snapshot.headphones.gain, 0.5,
        "headphone volume in the snapshot"
    );
    assert_eq!(
        snapshot.headphones.mix, 0.25,
        "the proportion in the snapshot"
    );
}

#[test]
fn a_nan_headphone_value_is_refused() {
    let (mut graph, submissions, _cells) = graph();
    let gain = queue(
        &submissions,
        1,
        DjCommand::SetCueGain {
            linear_gain: f32::NAN,
        },
    );
    let mix = queue(&submissions, 2, DjCommand::SetCueMix { mix: f32::NAN });
    render(&mut graph, 2);
    for (answers, what) in [
        (&gain, "a NaN headphone volume"),
        (&mix, "a NaN proportion"),
    ] {
        let ack = answers
            .try_recv()
            .unwrap_or_else(|_| panic!("no answer for {what}"));
        assert_eq!(
            ack.status,
            AckStatus::Rejected(RejectReason::InvalidValue),
            "{what} is refused"
        );
    }
}
