//! The metronome: a click on every beat of the grid (DJP-SYNC-003).
//!
//! These run the real graph, and the track is silent except for marks placed at known frames: the only
//! sounds in the master are the metronome's clicks and those marks. The marks are the material's own
//! clock — a deck primes its scratch and its tempo stage delays the sound, so the master's first frame is
//! not source frame zero, and the beats are compared with the material rather than with the graph.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_deck::Deck;
use dj_domain::{Ack, CommandId, DeckId, DjCommand, ScheduledCommand};
use dj_engine::{DeckStatic, EngineGraph, StateCells, Submission};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};
use std::sync::Arc;
use std::sync::mpsc::Sender;

const RATE: u32 = 48_000;
const BLOCK: usize = 256;
/// 120 BPM at 48 kHz: a beat every 24 000 frames, the first of the track at 4 800.
const BEAT_FRAMES: usize = 24_000;
const ANCHOR: usize = 4_800;
/// Length of one click, in frames: 20 ms at 48 kHz.
const CLICK_FRAMES: usize = 960;

/// A silent track with marks: the material's own clock, in the master.
struct Marks {
    at: u64,
    next: u64,
    step: u64,
}

impl dj_deck::DeckSource for Marks {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn frames(&self) -> Option<u64> {
        Some(960_000)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let take = out.len() / 2;
        out[..take * 2].fill(0.0);
        let from = self.next;
        let to = from + take as u64;
        let mut mark = self.at;
        while mark < to {
            if mark >= from {
                let at = (mark - from) as usize;
                for frame in at..(at + 4).min(take) {
                    out[frame * 2] = 0.9;
                    out[frame * 2 + 1] = 0.9;
                }
            }
            mark += self.step;
        }
        self.at = mark;
        self.next = to;
        Ok(take)
    }
    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.next = frame;
        Ok(frame)
    }
}

/// A deck playing a silent track that marks every `step` frames from `first`, with a grid on its beats.
///
/// The marks sit half a beat away from the beats so a mark never hides a click.
fn marked_deck() -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.attach(
        Box::new(Marks {
            at: (ANCHOR + BEAT_FRAMES / 2) as u64,
            next: 0,
            step: BEAT_FRAMES as u64,
        }),
        Some(dj_domain::TrackMeta {
            title: String::from("Marks"),
            artist: String::new(),
            duration_frames: Some(960_000),
            bpm: None,
            first_beat_frame: None,
            grid: None,
            key: None,
        }),
    );
    deck.set_blocking(false);
    deck.set_gain(1.0);
    deck.set_beat_grid(
        dj_domain::BeatGrid::single(120.0, ANCHOR as u64)
            .expect("a grid")
            .with_beats_per_bar(4),
    );
    deck
}

fn mixer_for(rate: u32) -> Mixer {
    let mut mixer = Mixer::new(2);
    mixer.prepare(rate, BLOCK);
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
    mixer
}

fn graph_for(deck: Deck) -> (EngineGraph, Arc<StateCells>, Sender<Submission>) {
    let cells = Arc::new(StateCells::new());
    let (submissions, receiver) = std::sync::mpsc::channel();
    let (_loads, load_receiver) = std::sync::mpsc::channel();
    let graph = EngineGraph::new(
        vec![deck],
        mixer_for(RATE),
        receiver,
        Arc::clone(&cells),
        BLOCK,
        load_receiver,
    );
    (graph, cells, submissions)
}

/// Post a command and hand back where its ack will arrive.
///
/// The ack comes out of the render loop, so it is read after a block, never before: waiting for it here
/// would wait for a block that nobody renders.
fn post(
    submissions: &Sender<Submission>,
    id: CommandId,
    command: DjCommand,
) -> std::sync::mpsc::Receiver<Ack> {
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
    answer
}

fn applied(answer: std::sync::mpsc::Receiver<Ack>) -> Ack {
    answer
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("an ack from the render loop")
}

/// The state document a client would receive, with one deck in it.
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

/// Render `frames` of master, block by block, and return it.
fn render(graph: &mut EngineGraph, frames: usize) -> Vec<f32> {
    let mut master = vec![0.0f32; frames * 2];
    let mut cue = vec![0.0f32; frames * 2];
    let mut written = 0;
    while written < frames {
        let count = (frames - written).min(BLOCK);
        graph.render_pair(
            &mut master[written * 2..(written + count) * 2],
            &mut cue[written * 2..(written + count) * 2],
            count,
        );
        written += count;
    }
    master
}

/// Where the loud marks are: they are the audio's own clock, four frames each.
fn marks(master: &[f32]) -> Vec<usize> {
    let frames = master.len() / 2;
    let mut found = Vec::new();
    let mut frame = 0usize;
    while frame < frames {
        if master[frame * 2].abs() > 0.5 {
            found.push(frame);
            frame += CLICK_FRAMES;
        } else {
            frame += 1;
        }
    }
    found
}

/// Where the clicks start, the marks left out.
///
/// A mark is four frames of a loud impulse, and the limiter smears its edges through the band a click
/// lives in: without keeping clear of them, every mark would count as a click of its own.
fn clicks(master: &[f32], marks: &[usize]) -> Vec<usize> {
    let frames = master.len() / 2;
    let mut found = Vec::new();
    let mut frame = 0usize;
    while frame < frames {
        let level = master[frame * 2].abs();
        // A click is a tone, so it crosses zero twice a period: after one is found, the next is looked
        // for a click length later, which is far inside the beat at any tempo a DJ plays.
        if level > 0.02
            && marks
                .iter()
                .all(|mark| frame.abs_diff(*mark) > CLICK_FRAMES)
        {
            found.push(frame);
            frame += CLICK_FRAMES;
        } else {
            frame += 1;
        }
    }
    found
}

/// Play with the metronome on and hand back the master.
fn metronome_on(graph: &mut EngineGraph, cells: &StateCells, frames: usize) -> Vec<f32> {
    let _ = cells;
    render(graph, frames)
}

#[test]
fn a_click_lands_on_the_beat_the_material_is_at() {
    // A mark sits half a beat after each beat, so the click has to be one half beat before the mark.
    // This is the material's own clock: a deck primes its scratch, and the tempo stage delays the sound,
    // so counting from the master's first frame would ask the wrong question.
    let (mut graph, cells, submissions) = graph_for(marked_deck());
    let click = post(
        &submissions,
        1,
        DjCommand::Metronome {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let play = post(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let master = metronome_on(&mut graph, &cells, 96_000);
    assert!(
        matches!(applied(click).status, dj_domain::AckStatus::Applied { .. }),
        "the metronome is applied"
    );
    assert!(
        matches!(applied(play).status, dj_domain::AckStatus::Applied { .. }),
        "the deck plays"
    );

    let found_marks = marks(&master);
    let found_clicks = clicks(&master, &found_marks);
    assert!(found_marks.len() >= 3, "three marks: {:?}", found_marks);
    assert!(found_clicks.len() >= 3, "three clicks: {:?}", found_clicks);
    // The beat length, as the material plays it: the distance between two marks.
    let beat = found_marks[1] - found_marks[0];
    for (index, mark) in found_marks.iter().take(3).enumerate() {
        let want = *mark as i64 - (beat / 2) as i64;
        let error = (found_clicks[index] as i64 - want).abs();
        assert!(
            error <= 32,
            "click {index} starts at {} and the beat of the material is at {want}: {error} frames apart \
             (beat {beat})",
            found_clicks[index]
        );
    }
}

#[test]
fn the_clicks_keep_the_materials_beat() {
    // The gap between two clicks is the gap between two beats of the material — no rounding creeping in
    // block by block, and no drift against the track.
    let (mut graph, cells, submissions) = graph_for(marked_deck());
    let _ = post(
        &submissions,
        1,
        DjCommand::Metronome {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = post(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let master = metronome_on(&mut graph, &cells, 96_000);
    let found_marks = marks(&master);
    let found_clicks = clicks(&master, &found_marks);
    assert!(
        found_marks.len() >= 4 && found_clicks.len() >= 4,
        "three beats of marks and clicks: {:?} and {:?}",
        found_marks,
        found_clicks
    );
    // Both lists run one entry per beat, so the gaps line up beat for beat.
    let beats: Vec<i64> = found_marks
        .windows(2)
        .map(|pair| pair[1] as i64 - pair[0] as i64)
        .collect();
    let gaps: Vec<i64> = found_clicks
        .windows(2)
        .map(|pair| pair[1] as i64 - pair[0] as i64)
        .collect();
    for index in 0..gaps.len().min(beats.len()).min(3) {
        let (gap, want) = (gaps[index], beats[index]);
        assert!(
            (gap - want).abs() <= 2,
            "clicks {index} and {} are {gap} frames apart, and the material's beats are {want}",
            index + 1
        );
    }
}

#[test]
fn the_first_beat_of_the_bar_is_accented() {
    // A bar of four: the accent is a fifth above the beat, so the click on beat one changes sign more
    // often than the one on beat two. Counting crossings needs no spectrum and no guess.
    let (mut graph, cells, submissions) = graph_for(marked_deck());
    let _ = post(
        &submissions,
        1,
        DjCommand::Metronome {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = post(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let master = metronome_on(&mut graph, &cells, 96_000);
    let found_marks = marks(&master);
    let found = clicks(&master, &found_marks);
    assert!(found.len() >= 4, "four clicks: {:?}", found);
    let crossings = |from: usize| {
        let mut count = 0usize;
        let mut previous = 0.0f32;
        for frame in from..from + 480 {
            let value = master[frame * 2];
            if (value >= 0.0) != (previous >= 0.0) && previous != 0.0 {
                count += 1;
            }
            previous = value;
        }
        count
    };
    // Ten milliseconds of each click: 1 000 Hz gives ten periods, 1 500 Hz fifteen. A click rendering
    // starts inside a bar, so the first two clicks are beat one and beat two of that bar.
    let accent = crossings(found[0]);
    let plain = crossings(found[1]);
    assert!(
        accent > plain + 2,
        "the accent of the bar changes sign {accent} times against {plain} on a plain beat"
    );
}

#[test]
fn a_metronome_that_is_off_clicks_nowhere() {
    // The control arm: the same silence, the same grid, the same transport — and nothing to hear, which
    // is what makes the tests above about the metronome rather than about the render loop.
    let (mut graph, cells, submissions) = graph_for(marked_deck());
    let _ = post(
        &submissions,
        1,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let master = render(&mut graph, 96_000);
    let _ = cells;
    let found_marks = marks(&master);
    let found = clicks(&master, &found_marks);
    assert!(
        found.is_empty(),
        "silence without the metronome, but clicks were found at {found:?}"
    );
}

#[test]
fn the_state_says_which_deck_clicks_and_how_many_times() {
    // The flag alone would only prove the command arrived; the count proves the click reached the audio
    // path — the one thing a check cannot hear.
    let (mut graph, cells, submissions) = graph_for(marked_deck());
    let _ = post(
        &submissions,
        1,
        DjCommand::Metronome {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = post(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = render(&mut graph, 96_000);
    let state = published(&cells);
    let deck = state.decks.first().expect("one deck");
    assert!(deck.metronome, "the flag is published");
    assert!(
        deck.clicks >= 3,
        "three clicks were played, the state counts {}",
        deck.clicks
    );
}

#[test]
fn turning_the_click_off_stops_it() {
    let (mut graph, cells, submissions) = graph_for(marked_deck());
    let _ = post(
        &submissions,
        1,
        DjCommand::Metronome {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = post(
        &submissions,
        2,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    );
    let _ = render(&mut graph, 48_000);
    let before = published(&cells).decks[0].clicks;
    let _ = post(
        &submissions,
        3,
        DjCommand::Metronome {
            deck: DeckId::A,
            enabled: false,
        },
    );
    let _ = render(&mut graph, 96_000);
    let after = published(&cells).decks[0].clicks;
    assert!(
        after <= before + 1,
        "the click stopped: {before} clicks before the switch and {after} two seconds after it"
    );
}
