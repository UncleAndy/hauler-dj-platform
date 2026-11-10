//! Loops asked for by length, moving them (DJP-DECK-005), and jumps by beats (DJP-DECK-006).
//!
//! Both commands are decided by the engine against the deck's grid: the DJ asks for "four beats", not for
//! a pair of frames, and a deck without a grid has no four beats to give. The tests drive the real graph,
//! so what is checked is what a client would get — the ack, the loop in the snapshot, and the play head.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_deck::Deck;
use dj_domain::{
    Ack, AckStatus, CommandId, DeckId, DjCommand, RejectReason, ScheduledCommand, SourceFrame,
};
use dj_engine::{DeckStatic, EngineGraph, StateCells, Submission};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};
use std::sync::Arc;
use std::sync::mpsc::Sender;

const RATE: u32 = 48_000;
const BLOCK: usize = 256;
/// 120 BPM at 48 kHz: a beat every 24 000 frames, the first of the track at 4 800.
const BEAT_FRAMES: u64 = 24_000;
const ANCHOR: u64 = 4_800;

/// A silent source: the tests are about the grid and the loop region, not about the sound.
struct Silence {
    position: u64,
}

impl dj_deck::DeckSource for Silence {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn frames(&self) -> Option<u64> {
        Some(960_000)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let take = out.len() / 2;
        out[..take * 2].fill(0.0);
        self.position += take as u64;
        Ok(take)
    }
    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame;
        Ok(frame)
    }
}

fn deck_on_grid(with_grid: bool) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(RATE);
    deck.attach(Box::new(Silence { position: 0 }), None);
    if with_grid {
        deck.set_beat_grid(dj_domain::BeatGrid::single(120.0, ANCHOR).expect("a grid at 120 BPM"));
    }
    deck
}

fn graph_for(deck: Deck) -> (EngineGraph, Arc<StateCells>, Sender<Submission>) {
    let cells = Arc::new(StateCells::new());
    let (submissions, receiver) = std::sync::mpsc::channel();
    let (_loads, load_receiver) = std::sync::mpsc::channel();
    let mut mixer = Mixer::new(2);
    mixer.prepare(RATE, BLOCK);
    for index in 0..2 {
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
    let graph = EngineGraph::new(
        vec![deck],
        mixer,
        receiver,
        Arc::clone(&cells),
        BLOCK,
        load_receiver,
    );
    (graph, cells, submissions)
}

/// Send a command and render the block that applies it, then take its ack.
///
/// The ack comes out of the render loop: waiting for it before rendering is waiting for a block nobody
/// renders — the first version of this file did exactly that and hung on a timeout.
fn apply(
    graph: &mut EngineGraph,
    submissions: &Sender<Submission>,
    id: CommandId,
    command: DjCommand,
) -> Ack {
    let (reply, answer) = std::sync::mpsc::channel();
    submissions
        .send(Submission {
            command: ScheduledCommand {
                id,
                effective_sample: 0,
                command,
            },
            reply,
        })
        .expect("the graph takes commands");
    render_one(graph);
    answer
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("an ack from the render loop")
}

fn published(cells: &StateCells) -> dj_domain::Snapshot {
    let statics = [DeckStatic {
        deck: DeckId::A,
        load: dj_domain::DeckLoad::Ready,
        track: None,
        total_frames: Some(960_000),
        waveform: None,
        auto_gain: 1.0,
        auto_gain_db: None,
        analyzing: false,
        grid: None,
        source_rate: RATE,
        path: None,
    }];
    cells.read(&statics, RATE, 1, 1)
}

fn deck_state(cells: &StateCells) -> dj_domain::DeckSnapshot {
    published(cells).decks[0].clone()
}

fn render_one(graph: &mut EngineGraph) {
    let mut master = vec![0.0f32; BLOCK * 2];
    let mut cue = vec![0.0f32; BLOCK * 2];
    graph.render_pair(&mut master, &mut cue, BLOCK);
}

#[test]
fn a_nudge_reaches_the_deck_and_the_state_reports_it() {
    // The shell holds a key and expects the sound to move with it: the command is continuous, and the
    // snapshot has to carry the bend so a client that reconnects knows the hand is still on.
    let (mut graph, cells, submissions) = graph_for(deck_on_grid(true));
    render_one(&mut graph);
    let ack = apply(
        &mut graph,
        &submissions,
        1,
        DjCommand::Nudge {
            deck: DeckId::A,
            amount: 0.5,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "a nudge is an ordinary deck command, got {:?}",
        ack.status
    );
    render_one(&mut graph);
    let bent = deck_state(&cells).nudge;
    assert!(
        (bent - 0.5).abs() < 1e-6,
        "the state must say the deck is bent, got {bent}"
    );

    let ack = apply(
        &mut graph,
        &submissions,
        2,
        DjCommand::Nudge {
            deck: DeckId::A,
            amount: 0.0,
        },
    );
    assert!(matches!(ack.status, AckStatus::Applied { .. }));
    render_one(&mut graph);
    assert_eq!(
        deck_state(&cells).nudge,
        0.0,
        "letting go is a bend of zero"
    );
}

#[test]
fn a_beat_jump_is_decided_against_the_grid() {
    // The play head sits a third of a beat past the fourth: a jump of one beat must land on the fifth
    // beat, not a beat-length past where it stood. The state document is what the shell draws.
    let (mut graph, cells, submissions) = graph_for(deck_on_grid(true));
    render_one(&mut graph);
    let _ = apply(
        &mut graph,
        &submissions,
        1,
        DjCommand::SeekFrames {
            deck: DeckId::A,
            frame: SourceFrame(ANCHOR + 3 * BEAT_FRAMES + 7_000),
        },
    );
    let ack = apply(
        &mut graph,
        &submissions,
        2,
        DjCommand::BeatJump {
            deck: DeckId::A,
            beats: 1.0,
            quantize: true,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "a grid is there to obey, got {:?}",
        ack.status
    );
    render_one(&mut graph);
    assert_eq!(
        deck_state(&cells).source_frame.0,
        ANCHOR + 4 * BEAT_FRAMES,
        "the jump lands on the beat"
    );
}

#[test]
fn a_beat_jump_without_a_grid_is_refused() {
    // Nothing to land on: a refusal the shell can show beats a jump to nowhere.
    let (mut graph, _cells, submissions) = graph_for(deck_on_grid(false));
    render_one(&mut graph);
    let ack = apply(
        &mut graph,
        &submissions,
        1,
        DjCommand::BeatJump {
            deck: DeckId::A,
            beats: 4.0,
            quantize: true,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Rejected(RejectReason::InvalidValue)),
        "a quantized jump needs a grid, got {:?}",
        ack.status
    );
}

#[test]
fn four_beats_is_four_beats_of_this_track() {
    // The client asks for a length, the engine finds the beats: the start snaps to the nearest beat of
    // the grid (frame 4 800, not the play head's frame 0) and the end is four beats later. The play head
    // steps into the loop, because pressing "4" is a request to hear those four beats.
    let (mut graph, cells, submissions) = graph_for(deck_on_grid(true));
    render_one(&mut graph);
    let ack = apply(
        &mut graph,
        &submissions,
        1,
        DjCommand::LoopBeats {
            deck: DeckId::A,
            beats: 4,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "a grid is there to obey, got {:?}",
        ack.status
    );
    render_one(&mut graph);

    let state = deck_state(&cells);
    let region = state.loop_region.expect("the loop is in force");
    assert_eq!(region.start.0, ANCHOR, "the loop starts on a beat");
    assert_eq!(region.len(), 4 * BEAT_FRAMES, "four beats long");
    assert_eq!(state.source_frame.0, ANCHOR, "the play head is in the loop");
    // Every loop is the same loop: the one made of beats arms the mode exactly as a pad's would, and the
    // state document says so — the Loop button has nothing else to draw.
    assert!(state.loop_mode, "a loop put in force arms the mode");
}

#[test]
fn moving_the_loop_takes_the_play_head_with_it() {
    let (mut graph, cells, submissions) = graph_for(deck_on_grid(true));
    render_one(&mut graph);
    let _ = apply(
        &mut graph,
        &submissions,
        1,
        DjCommand::LoopBeats {
            deck: DeckId::A,
            beats: 2,
        },
    );
    let first = deck_state(&cells).loop_region.expect("a loop");
    // Where inside the loop the sound is before the move: the loop takes that place with it, so the
    // groove does not restart at the new beginning (and the head is never left outside the loop, which
    // played the beats between the old and the new region).
    let place = deck_state(&cells)
        .source_frame
        .0
        .saturating_sub(first.start.0);
    let ack = apply(
        &mut graph,
        &submissions,
        2,
        DjCommand::LoopMove {
            deck: DeckId::A,
            beats: 1,
        },
    );
    assert!(matches!(ack.status, AckStatus::Applied { .. }));
    let state = deck_state(&cells);
    let moved = state.loop_region.expect("the loop is still there");
    assert_eq!(moved.start.0, first.start.0 + BEAT_FRAMES, "one beat on");
    assert_eq!(moved.len(), first.len(), "the length is the loop's own");
    assert_eq!(
        state.source_frame.0,
        moved.start.0 + place,
        "the play head came along, at its own place in the loop"
    );
    assert!(
        moved.start.0 <= state.source_frame.0 && state.source_frame.0 < moved.end.0,
        "the head must be inside the moved loop"
    );
}

#[test]
fn a_deck_without_a_grid_refuses_a_loop_of_beats() {
    // No grid, no beats: inventing a length would be a loop of nothing in particular, and the DJ has to
    // know it did not happen — a rejected ack, not a silent no.
    let (mut graph, cells, submissions) = graph_for(deck_on_grid(false));
    render_one(&mut graph);
    let ack = apply(
        &mut graph,
        &submissions,
        1,
        DjCommand::LoopBeats {
            deck: DeckId::A,
            beats: 4,
        },
    );
    assert_eq!(
        ack.status,
        AckStatus::Rejected(RejectReason::InvalidValue),
        "a length loop without a grid must be refused"
    );
    render_one(&mut graph);
    assert!(deck_state(&cells).loop_region.is_none());
}
