//! The deck's effect reaches the engine: the patch is applied where the strip applies it, the switch works both ways,
//! and the snapshot says what the mixer has (owner's decision, 21.11).
//!
//! The graph applies commands at a block boundary, so every test queues first and renders after: a test that waited
//! for an answer without rendering would wait forever, which is exactly what the engine would do.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::Deck;
use dj_domain::{
    Ack, AckStatus, BeatGrid, DeckId, DjCommand, FxKind, FxPatch, ScheduledCommand, TrackMeta,
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

/// Render `blocks` callbacks and return the loudest sample the room carried.
fn render(graph: &mut EngineGraph, blocks: usize) -> f32 {
    let mut master = vec![0.0f32; BLOCK * 2];
    let mut cue = vec![0.0f32; BLOCK * 2];
    let mut loudest = 0.0f32;
    for _ in 0..blocks {
        // The prefetcher fills its window on a loader thread: without the pause the graph runs ahead of the file and
        // renders silence.
        std::thread::sleep(std::time::Duration::from_millis(6));
        graph.render_pair(&mut master, &mut cue, BLOCK);
        loudest = loudest.max(
            master
                .iter()
                .fold(0.0f32, |held, sample| held.max(sample.abs())),
        );
    }
    loudest
}

/// The patch as the shell reads it.
///
/// `read` builds a deck only when a static entry describes it, so the test hands it one: an empty list makes the
/// snapshot contain no decks at all, and "the patch did not come back" would be the test's own doing.
fn patch_of(cells: &StateCells, index: usize, deck: DeckId) -> FxPatch {
    let statics = [dj_engine::DeckStatic {
        deck,
        load: dj_domain::DeckLoad::Empty,
        track: None,
        total_frames: None,
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
    let snapshot = cells.read(&statics, RATE, 1, 1);
    snapshot
        .decks
        .get(index)
        .map(|deck| deck.fx)
        .unwrap_or_default()
}

#[test]
fn the_fx_patch_reaches_the_mixer() {
    let (mut graph, submissions, cells) = graph();
    let answers = queue(
        &submissions,
        1,
        DjCommand::SetDeckFx {
            deck: DeckId::A,
            patch: FxPatch {
                kind: FxKind::Bitcrusher,
                amount: 0.75,
                enabled: true,
            },
        },
    );
    let _ = render(&mut graph, 4);
    match answers.try_recv().expect("an answer for the patch") {
        Ack {
            status: AckStatus::Applied { .. },
            ..
        } => {}
        other => panic!("the patch was refused: {other:?}"),
    }
    let patch = patch_of(&cells, 0, DeckId::A);
    assert_eq!(patch.kind, FxKind::Bitcrusher);
    assert!(
        (patch.amount - 0.75).abs() < 1e-6,
        "amount came back as {}",
        patch.amount
    );
    assert!(patch.enabled, "the engine kept the effect out of the path");
}

#[test]
fn turning_it_off_gives_the_dry_deck_back() {
    let (mut graph, submissions, _cells) = graph();
    let playing = queue(
        &submissions,
        10,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = render(&mut graph, 6);
    let _ = playing.try_recv().expect("an answer for play");
    let on = queue(
        &submissions,
        1,
        DjCommand::SetDeckFx {
            deck: DeckId::A,
            patch: FxPatch {
                kind: FxKind::Bitcrusher,
                amount: 1.0,
                enabled: true,
            },
        },
    );
    let with_effect = render(&mut graph, 8);
    let _ = on.try_recv().expect("an answer for the switch");

    let off = queue(
        &submissions,
        2,
        DjCommand::SetDeckFx {
            deck: DeckId::A,
            patch: FxPatch {
                kind: FxKind::Bitcrusher,
                amount: 1.0,
                enabled: false,
            },
        },
    );
    // The switch fades over 20 ms, so the first blocks still carry a little of it.
    let _ = render(&mut graph, 12);
    let _ = off.try_recv().expect("an answer for the second switch");

    assert!(
        with_effect > 0.0,
        "the deck rendered silence with the effect on"
    );
    let dry = render(&mut graph, 8);
    assert!(
        dry > 0.0,
        "the deck went silent after the effect was switched out"
    );
}

#[test]
fn the_beat_reaches_units_that_live_on_it() {
    // 120 BPM at a ratio of one is half a second per beat; the mixer's echo should take that from the deck and not
    // from a number of its own.
    let seconds = Mixer::beat_seconds(120.0, 1.0).expect("a tempo of 120 is a beat");
    assert!(
        (seconds - 0.5).abs() < 1e-6,
        "120 BPM is {seconds} s per beat"
    );
    let faster =
        Mixer::beat_seconds(120.0, 2.0).expect("a deck at twice the speed still has a beat");
    assert!(
        (faster - 0.25).abs() < 1e-6,
        "a deck at 1.08 is not the point: {faster} s"
    );
    assert!(
        Mixer::beat_seconds(0.0, 1.0).is_none(),
        "a deck with no tempo has no beat"
    );
}
