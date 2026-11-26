//! Конец трека: движок решает один раз, все клиенты рисуют один флаг (просьба владельца, 25.11).
//!
//! Правило здесь и проверяется, а не в трёх клиентах: петля и пауза снимают предупреждение, а «за сколько секунд»
//! задаётся настройкой и переводится в кадры по частоте самого файла.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use dj_deck::Deck;
use dj_domain::{
    Ack, AckStatus, BeatGrid, CommandId, DeckId, DjCommand, ScheduledCommand, SourceFrame,
};
use dj_engine::{DeckStatic, EngineGraph, StateCells, Submission};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;
/// The fixture: eight seconds at 48 kHz.
const TOTAL: u64 = 8 * 48_000;

/// A silent source of the fixture's length: the warning is about frames left, not about the sound.
struct Silence {
    position: u64,
}

impl dj_deck::DeckSource for Silence {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn frames(&self) -> Option<u64> {
        Some(TOTAL)
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

fn deck() -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(RATE);
    deck.attach(Box::new(Silence { position: 0 }), None);
    deck.set_beat_grid(BeatGrid::single(120.0, 0).expect("a grid of one tempo"));
    deck
}

fn graph_for(deck: Deck) -> (EngineGraph, Arc<StateCells>, Sender<Submission>) {
    let cells = Arc::new(StateCells::new());
    let (submissions, receiver) = channel();
    let (_loads, load_receiver) = channel();
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

fn render_one(graph: &mut EngineGraph) {
    let mut master = vec![0.0f32; BLOCK * 2];
    let mut cue = vec![0.0f32; BLOCK * 2];
    graph.render_pair(&mut master, &mut cue, BLOCK);
}

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
        total_frames: Some(TOTAL),
        waveform: None,
        auto_gain: 1.0,
        auto_gain_db: None,
        auto_gain_correction_db: None,
        analyzing: false,
        analysis_lock: false,
        grid: None,
        source_rate: RATE,
        path: None,
    }];
    cells.read(&statics, RATE, 1, 1)
}

fn warning(cells: &StateCells) -> bool {
    published(cells).decks[0].end_warning
}

/// A deck playing at `seconds` before the end, with the warning set.
fn playing_near_the_end(
    warning_seconds: f32,
    seconds_left: u64,
) -> (EngineGraph, Arc<StateCells>, Sender<Submission>) {
    let (mut graph, cells, submissions) = graph_for(deck());
    cells.set_end_warning_seconds(warning_seconds);
    render_one(&mut graph);
    let _ = apply(
        &mut graph,
        &submissions,
        1,
        DjCommand::SeekFrames {
            deck: DeckId::A,
            frame: SourceFrame(TOTAL - seconds_left * u64::from(RATE)),
        },
    );
    let ack = apply(
        &mut graph,
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "the deck refused to play: {:?}",
        ack.status
    );
    (graph, cells, submissions)
}

#[test]
fn the_last_seconds_of_a_playing_deck_are_a_warning() {
    let (_graph, cells, _submissions) = playing_near_the_end(5.0, 3);
    assert!(
        warning(&cells),
        "three seconds from the end with a five second warning is not warned"
    );
}

#[test]
fn a_deck_far_from_its_end_is_not_warned() {
    let (_graph, cells, _submissions) = playing_near_the_end(2.0, 6);
    assert!(
        !warning(&cells),
        "six seconds from the end with a two second warning is warned"
    );
}

#[test]
fn a_warning_of_zero_seconds_is_off() {
    let (_graph, cells, _submissions) = playing_near_the_end(0.0, 1);
    assert!(
        !warning(&cells),
        "the warning is off, and the deck warns anyway"
    );
}

#[test]
fn a_deck_standing_in_a_loop_is_not_warned() {
    let (mut graph, cells, submissions) = playing_near_the_end(5.0, 3);
    assert!(warning(&cells), "the deck started warned");
    let ack = apply(
        &mut graph,
        &submissions,
        3,
        DjCommand::LoopBeats {
            deck: DeckId::A,
            beats: 1,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "the loop was refused: {:?}",
        ack.status
    );
    render_one(&mut graph);
    assert!(
        !warning(&cells),
        "a deck repeating a loop has no end to warn about"
    );
    // And out of the loop the warning is back: it is the loop that silences it, not a latch.
    let ack = apply(
        &mut graph,
        &submissions,
        4,
        DjCommand::SetLoopMode {
            deck: DeckId::A,
            enabled: false,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "the loop mode was refused: {:?}",
        ack.status
    );
    render_one(&mut graph);
    assert!(
        warning(&cells),
        "the warning did not come back with the loop"
    );
}

#[test]
fn a_paused_deck_is_not_warned() {
    let (mut graph, cells, submissions) = playing_near_the_end(5.0, 3);
    let ack = apply(
        &mut graph,
        &submissions,
        5,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: false,
        },
    );
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "the deck refused to stop: {:?}",
        ack.status
    );
    render_one(&mut graph);
    assert!(
        !warning(&cells),
        "a paused deck is not going to reach its end"
    );
}
