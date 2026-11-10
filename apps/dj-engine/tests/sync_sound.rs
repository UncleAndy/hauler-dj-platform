//! Sound while synced: a follower dragged onto a beat that never moves goes silent (DJP-SYNC-002).
//!
//! The engine's own tests measure the numbers. This one measures the thing the numbers are for: two
//! decks on a real file, a master that stops, and whether the follower still reaches the master bus.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::Deck;
use dj_domain::{AckStatus, BeatGrid, DeckId, DjCommand};
use dj_engine::Submission;
use dj_engine::{EngineGraph, StateCells};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav")
}

fn deck(id: DeckId) -> Deck {
    let path = fixture();
    let prefetcher = Prefetcher::open(
        &path,
        PrefetchConfig {
            window_frames: 48_000,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open {}: {err}", path.display()));
    let mut deck = Deck::new(id);
    // Both decks read the same eight seconds of drums, so the grid is the same on both: what the sync
    // loop has to do is align them, not guess.
    let grid = BeatGrid::single(120.0, 0).expect("a grid of one tempo");
    deck.attach(
        Box::new(prefetcher),
        Some(dj_domain::TrackMeta {
            title: String::from("drums"),
            artist: String::new(),
            duration_frames: Some(48_000 * 8),
            bpm: Some(120.0),
            first_beat_frame: Some(0),
            grid: Some(grid),
            key: None,
        }),
    );
    deck.set_blocking(false);
    deck.set_gain(1.0);
    deck
}

fn graph() -> (EngineGraph, Sender<Submission>, Arc<StateCells>) {
    let decks = vec![deck(DeckId::A), deck(DeckId::B)];
    let mut mixer = dj_mixer::Mixer::new(2);
    mixer.prepare(RATE, BLOCK);
    for index in 0..2 {
        mixer.set_channel(
            index,
            dj_mixer::ChannelSettings {
                trim_db: 0.0,
                fader: 1.0,
                assign: dj_mixer::CrossAssign::Thru,
                cue: false,
            },
        );
    }
    let cells = Arc::new(StateCells::new());
    let (submissions, receiver) = std::sync::mpsc::channel();
    let (_loads, load_receiver) = std::sync::mpsc::channel();
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

/// Queue a command. The graph reads the queue at the start of a block, so nothing is applied until the
/// next render — a test that waited for the ack before rendering would wait forever.
fn send(
    submissions: &Sender<Submission>,
    id: u64,
    command: DjCommand,
) -> std::sync::mpsc::Receiver<dj_domain::Ack> {
    let (reply, answers) = std::sync::mpsc::channel();
    submissions
        .send(Submission {
            command: dj_domain::ScheduledCommand {
                id,
                effective_sample: 0,
                command,
            },
            reply,
        })
        .unwrap_or_else(|err| panic!("{err}"));
    answers
}

/// Render `blocks` blocks and return the loudest sample that reached the master bus.
fn loudest(graph: &mut EngineGraph, blocks: usize) -> f32 {
    let mut master = vec![0.0f32; BLOCK * 2];
    let mut cue = vec![0.0f32; BLOCK * 2];
    let mut peak = 0.0f32;
    for _ in 0..blocks {
        graph.render_pair(&mut master, &mut cue, BLOCK);
        for sample in &master {
            peak = peak.max(sample.abs());
        }
    }
    peak
}

#[test]
fn a_follower_still_sounds_when_its_master_stops() {
    let (mut graph, submissions, _cells) = graph();
    let _ack1 = send(
        &submissions,
        1,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ack2 = send(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::B,
            enabled: true,
        },
    );
    let _ack3 = send(&submissions, 3, DjCommand::Master { deck: DeckId::A });
    let _ack4 = send(
        &submissions,
        4,
        DjCommand::Sync {
            deck: DeckId::B,
            mode: dj_domain::SyncMode::Beat,
        },
    );

    // The key-locked stretch stage spends its delay before it has a sample to give: twenty blocks.
    let warm = loudest(&mut graph, 40);
    for (name, answers) in [
        ("play A", _ack1),
        ("play B", _ack2),
        ("master", _ack3),
        ("sync", _ack4),
    ] {
        let ack = answers
            .try_recv()
            .unwrap_or_else(|err| panic!("{name}: no ack: {err}"));
        assert!(
            matches!(ack.status, AckStatus::Applied { .. }),
            "{name}: {:?}",
            ack.status
        );
    }
    assert!(
        warm > 0.01,
        "both decks should be audible while both play: {warm}"
    );
    let before = graph.decks()[1].position();

    // The master stops. Its beat stands still, so anything that keeps jumping onto it asks for a seek
    // four times a second — and a deck asked to seek that often never gets its material read.
    let _ack5 = send(
        &submissions,
        5,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: false,
        },
    );
    let after_stop = loudest(&mut graph, 50);
    let travelled = graph.decks()[1].position().saturating_sub(before);
    let expected = RATE as u64 * 50 * BLOCK as u64 / RATE as u64;
    println!(
        "half a second after the master stopped: peak {after_stop:.4}, follower travelled {travelled} of {expected} frames"
    );
    assert!(
        after_stop > 0.01,
        "the follower must still be heard with the master stopped, peak was {after_stop}"
    );
    assert!(
        travelled > expected / 2,
        "the follower must keep playing, it moved {travelled} of {expected} frames"
    );
}

#[test]
fn a_follower_takes_the_master_s_beat_without_stopping_the_sound() {
    // The same file on both decks, but the follower starts a quarter of a beat late: sync has to move it
    // onto the beat, and the move must not cost the sound.
    let (mut graph, submissions, cells) = graph();
    let _ack1 = send(
        &submissions,
        1,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ack2 = send(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::B,
            enabled: true,
        },
    );
    let _ack5 = send(
        &submissions,
        5,
        DjCommand::SeekFrames {
            deck: DeckId::B,
            frame: dj_domain::SourceFrame(12_000),
        },
    );
    let _ack3 = send(&submissions, 3, DjCommand::Master { deck: DeckId::A });
    let _ack4 = send(
        &submissions,
        4,
        DjCommand::Sync {
            deck: DeckId::B,
            mode: dj_domain::SyncMode::Beat,
        },
    );
    let _warm = loudest(&mut graph, 40);
    let after = loudest(&mut graph, 40);
    // The jump lands while the time-stretch stage is still refilling its window, so the distance it
    // reports right after is not the distance it holds: what matters is where it stands once settled.
    let mut settled_phase = f64::INFINITY;
    let mut loudest_after = after;
    for _ in 0..25 {
        loudest_after = loudest_after.max(loudest(&mut graph, 8));
        let statics: Vec<dj_engine::DeckStatic> = graph
            .decks()
            .iter()
            .map(|deck| dj_engine::DeckStatic::from_snapshot(&deck.snapshot()))
            .collect();
        let state = cells.read(&statics, RATE, 1, 1);
        settled_phase = state
            .decks
            .iter()
            .find(|deck| deck.deck == DeckId::B)
            .and_then(|deck| deck.phase_error_ms)
            .unwrap_or(settled_phase);
        if settled_phase.abs() < 2.0 {
            break;
        }
    }
    println!(
        "after the jump: peak {loudest_after:.4}, settled {settled_phase:.3} ms from the master's beat"
    );
    assert!(
        loudest_after > 0.01,
        "the follower is on the beat and silent: {loudest_after}"
    );
    assert!(
        settled_phase.abs() < 2.0,
        "the follower should settle on the master's beat, it stands {settled_phase:.3} ms away"
    );
}

#[test]
fn the_engine_notices_a_master_that_stops() {
    // Guard for the test above: if the graph kept treating a paused deck as running, the gate that
    // stops the jumps would never be reached and the test would pass for the wrong reason.
    let started = Instant::now();
    let (mut graph, submissions, _cells) = graph();
    let _ack1 = send(
        &submissions,
        1,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = loudest(&mut graph, 4);
    assert!(graph.decks()[0].is_playing());
    let _ack2 = send(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: false,
        },
    );
    // A command is read at the start of a block, so the graph has to render one before the stop is real.
    let _ = loudest(&mut graph, 2);
    assert!(!graph.decks()[0].is_playing(), "the graph took the stop");
    println!("started in {:?}", started.elapsed());
}
