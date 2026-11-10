//! Four decks: a file on any of the four has to reach the master.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::Deck;
use dj_domain::{BeatGrid, DeckId, TrackMeta};
use dj_domain::{DjCommand, ScheduledCommand};
use dj_engine::Submission;
use dj_engine::{EngineGraph, StateCells};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

const IDS: [DeckId; 4] = [DeckId::A, DeckId::B, DeckId::C, DeckId::D];

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
        }),
    );
    deck.set_blocking(false);
    deck.set_gain(1.0);
    deck
}

/// Four decks, four channels, the sides the shell gives them; `playing` is the only deck with a file,
/// so whatever reaches the master came from that one channel.
fn graph_with_only(playing: Option<DeckId>) -> (EngineGraph, Sender<Submission>, Arc<StateCells>) {
    let decks: Vec<Deck> = IDS
        .iter()
        .map(|id| {
            if Some(*id) == playing {
                loaded(*id)
            } else {
                Deck::new(*id)
            }
        })
        .collect();
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

/// Queue a command; the graph reads its queue at the start of a block.
fn send(submissions: &Sender<Submission>, id: u64, command: DjCommand) {
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
        .unwrap_or_else(|err| panic!("{err}"));
    let _ = answers.recv_timeout(std::time::Duration::from_secs(1));
}

/// Render a few blocks and hand back the loudest sample the master carried.
fn render(graph: &mut EngineGraph, blocks: usize) -> f32 {
    let mut master = vec![0.0_f32; BLOCK * 2];
    let mut cue = vec![0.0_f32; BLOCK * 2];
    let mut loudest = 0.0_f32;
    for _ in 0..blocks {
        std::thread::sleep(std::time::Duration::from_millis(6));
        graph.render_pair(&mut master, &mut cue, BLOCK);
        for sample in &master {
            loudest = loudest.max(sample.abs());
        }
    }
    loudest
}

/// The four decks the shell shows are four channels of one mixer, and a file on any of them has to
/// reach the master. This walks the whole chain four times, once per deck, with nothing else playing.
#[test]
fn every_one_of_four_decks_reaches_the_master() {
    for id in IDS {
        let (mut graph, submissions, cells) = graph_with_only(Some(id));
        send(
            &submissions,
            1,
            DjCommand::Play {
                deck: id,
                enabled: true,
            },
        );
        let _ = render(&mut graph, 30);
        let heard = render(&mut graph, 60);
        println!("deck {id:?}: master {heard:.4}");
        assert!(
            heard > 0.01,
            "deck {id:?} plays, but nothing of it reaches the master"
        );
        let _ = cells;
    }
}

/// And all four at once: four deck objects, four channels, one master that carries the sum.
#[test]
fn four_decks_play_into_one_master() {
    let (mut graph, submissions, _cells) = {
        // Same shape as `graph_with_only`, but every deck gets the file.
        let decks: Vec<Deck> = IDS.iter().copied().map(loaded).collect();
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
        let cells = Arc::new(StateCells::new());
        let (submissions, receiver): (Sender<Submission>, Receiver<Submission>) = channel();
        let (_loads, load_receiver) = channel();
        (
            EngineGraph::new(
                decks,
                mixer,
                receiver,
                Arc::clone(&cells),
                BLOCK,
                load_receiver,
            ),
            submissions,
            cells,
        )
    };
    for (index, id) in IDS.iter().enumerate() {
        send(
            &submissions,
            100 + index as u64,
            DjCommand::Play {
                deck: *id,
                enabled: true,
            },
        );
    }
    let _ = render(&mut graph, 30);
    let heard = render(&mut graph, 60);
    println!("four decks: master {heard:.4}");
    assert!(heard > 0.01, "four decks play, and the master is silent");
}
