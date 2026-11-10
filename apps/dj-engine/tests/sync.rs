//! S2: the engine follows a master deck, measured on the decks themselves (DJP-SYNC-001/002).
//!
//! These tests call `sync_step` — the function the audio callback calls every few blocks — so what is
//! measured here is the code that runs in the engine, not a copy of its arithmetic.
#![allow(clippy::expect_used)]

use dj_deck::Deck;
use dj_domain::{
    AckStatus, BeatGrid, DeckId, DjCommand, ScheduledCommand, SourceFrame, SyncMode, TrackMeta,
};
use dj_engine::{apply, sync_step};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const RATE: u32 = 48_000;
/// The engine's sync step: eight blocks of 256 frames at 48 kHz, which is 42.7 ms.
const STEP_SECONDS: f64 = 8.0 * 256.0 / 48_000.0;

/// A deck playing a tone, with a grid, so it has a beat to follow and a tempo to match.
fn deck_with_grid(id: DeckId, bpm: f64, first_beat: u64) -> Deck {
    struct Tone;
    impl dj_deck::DeckSource for Tone {
        fn sample_rate(&self) -> u32 {
            RATE
        }
        fn frames(&self) -> Option<u64> {
            Some(48_000 * 600)
        }
        fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
            let take = out.len() / 2;
            out[..take * 2].fill(0.25);
            Ok(take)
        }
        fn seek(&mut self, frame: u64) -> Result<u64, String> {
            Ok(frame)
        }
    }
    let grid = BeatGrid::single(bpm, first_beat).expect("a grid of one tempo");
    let mut deck = Deck::new(id);
    deck.attach(
        Box::new(Tone),
        Some(TrackMeta {
            title: String::from("Tone"),
            artist: String::new(),
            duration_frames: Some(48_000 * 600),
            bpm: Some(bpm),
            first_beat_frame: Some(first_beat),
            grid: Some(grid),
            key: None,
        }),
    );
    deck.set_blocking(false);
    deck.set_gain(1.0);
    deck
}

fn mixer() -> Mixer {
    let mut mixer = Mixer::new(2);
    mixer.prepare(RATE, 256);
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

fn command(id: u64, command: DjCommand) -> ScheduledCommand {
    ScheduledCommand {
        id,
        effective_sample: 0,
        command,
    }
}

/// Make A the master and B follow its beat, through the protocol the shell uses.
fn lock_b_to_a(decks: &mut [Deck], mixer: &mut Mixer) {
    let master = apply(
        decks,
        mixer,
        &command(1, DjCommand::Master { deck: DeckId::A }),
        0,
    );
    assert!(
        matches!(master.status, AckStatus::Applied { .. }),
        "{:?}",
        master.status
    );
    let sync = apply(
        decks,
        mixer,
        &command(
            2,
            DjCommand::Sync {
                deck: DeckId::B,
                mode: SyncMode::Beat,
            },
        ),
        0,
    );
    assert!(matches!(sync.status, AckStatus::Applied { .. }));
}

fn phase_of(deck: &Deck) -> f64 {
    let grid = deck.beat_grid().expect("the deck has a grid");
    let beat = grid.beat_at(deck.position(), f64::from(RATE));
    beat - beat.floor()
}

#[test]
fn a_follower_takes_the_master_s_tempo() {
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0),
        deck_with_grid(DeckId::B, 120.0, 0),
    ];
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);

    // Two seconds of control ticks: the trim allows four percent a second, and the move is 6.67 %.
    for _ in 0..50 {
        sync_step(&mut decks, STEP_SECONDS);
    }
    let ratio = decks[1].tempo_ratio();
    println!("follower ratio after two seconds: {ratio:.9}");
    assert!(
        (ratio - 128.0 / 120.0).abs() < 1e-6,
        "the follower should run at the master's tempo, it runs at {ratio:.9}"
    );
    // And the master was not touched by its own followers.
    assert!((decks[0].tempo_ratio() - 1.0).abs() < 1e-12);
}

#[test]
fn beat_sync_puts_the_follower_on_the_master_s_beat() {
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0),
        deck_with_grid(DeckId::B, 120.0, 0),
    ];
    // A quarter of a beat ahead of the master: 6000 frames at 120 BPM and 48 kHz.
    decks[1].seek(6_000);
    assert!((phase_of(&decks[1]) - 0.25).abs() < 1e-9);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);

    sync_step(&mut decks, STEP_SECONDS);

    let master = phase_of(&decks[0]);
    let follower = phase_of(&decks[1]);
    let apart_ms = (follower - master) * 60_000.0 / 128.0;
    println!(
        "beat sync: follower {} frames, phase {follower:.6} against {master:.6}, {apart_ms:.4} ms",
        decks[1].position()
    );
    assert!(
        apart_ms.abs() < 1.0,
        "after one step the follower stands {apart_ms:.3} ms from the master"
    );
}

#[test]
fn sync_off_leaves_the_follower_alone() {
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0),
        deck_with_grid(DeckId::B, 120.0, 0),
    ];
    let mut mixer = mixer();
    // A master but no follower: nothing is asked of B.
    apply(
        &mut decks,
        &mut mixer,
        &command(1, DjCommand::Master { deck: DeckId::A }),
        0,
    );
    for _ in 0..50 {
        sync_step(&mut decks, STEP_SECONDS);
    }
    assert!(
        (decks[1].tempo_ratio() - 1.0).abs() < 1e-12,
        "B kept its speed"
    );
    assert_eq!(decks[1].position(), 0, "B was not moved");
}

#[test]
fn a_tempo_the_fader_cannot_reach_leaves_the_follower_at_its_own_speed() {
    // 150 against 120 needs 25 %, and the fader allows 16: the deck keeps its speed rather than
    // stopping at the edge, and the phase meter is left to show the distance.
    let mut decks = vec![
        deck_with_grid(DeckId::A, 150.0, 0),
        deck_with_grid(DeckId::B, 120.0, 0),
    ];
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    for _ in 0..50 {
        sync_step(&mut decks, STEP_SECONDS);
    }
    assert!((decks[1].tempo_ratio() - 1.0).abs() < 1e-12);
}

#[test]
fn one_deck_leads_at_a_time() {
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0),
        deck_with_grid(DeckId::B, 120.0, 0),
    ];
    let mut mixer = mixer();
    apply(
        &mut decks,
        &mut mixer,
        &command(1, DjCommand::Master { deck: DeckId::A }),
        0,
    );
    assert!(decks[0].master() && !decks[1].master());
    apply(
        &mut decks,
        &mut mixer,
        &command(2, DjCommand::Master { deck: DeckId::B }),
        0,
    );
    assert!(!decks[0].master(), "A gave the flag up");
    assert!(decks[1].master(), "B leads now");
}

#[test]
fn the_snapshot_says_who_leads_and_how_far_apart_the_beats_stand() {
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0),
        deck_with_grid(DeckId::B, 120.0, 0),
    ];
    decks[1].seek(6_000);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    for _ in 0..50 {
        sync_step(&mut decks, STEP_SECONDS);
    }

    let state = dj_engine::snapshot(&decks, &mixer, 1, 0, RATE, 1);
    let a = state
        .decks
        .iter()
        .find(|deck| deck.deck == DeckId::A)
        .expect("A");
    let b = state
        .decks
        .iter()
        .find(|deck| deck.deck == DeckId::B)
        .expect("B");
    assert!(a.master, "the snapshot names the master");
    assert_eq!(b.sync, SyncMode::Beat);
    assert!(
        b.phase_error_ms.is_some(),
        "a following deck publishes its distance from the master's beat"
    );
    println!(
        "snapshot: A master, B sync {:?}, phase {:?} ms, B at {:?}",
        b.sync, b.phase_error_ms, b.source_frame
    );
    assert_eq!(a.sync, SyncMode::Off);
    assert_eq!(a.phase_error_ms, None);
}

#[test]
fn the_deck_state_survives_a_command_round_trip() {
    // The mode is a deck's own state, so it comes back in the deck's snapshot even when nothing else
    // changed: a button that draws a mode the deck forgot would be a lit button over silence.
    let mut deck = deck_with_grid(DeckId::B, 120.0, 0);
    deck.set_sync(SyncMode::Tempo);
    assert_eq!(deck.snapshot().sync, SyncMode::Tempo);
    deck.set_sync(SyncMode::Off);
    assert_eq!(deck.snapshot().sync, SyncMode::Off);
    let _ = SourceFrame(0);
}
