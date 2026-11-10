//! S2: the engine follows a master deck, measured on the decks themselves (DJP-SYNC-001/002).
//!
//! These tests call `sync_step` — the function the audio callback calls every few blocks — so what is
//! measured here is the code that runs in the engine, not a copy of its arithmetic.
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dj_deck::Deck;
use dj_domain::{AckStatus, BeatGrid, DeckId, DjCommand, ScheduledCommand, SyncMode, TrackMeta};
use dj_engine::{SyncMemory, apply, sync_step};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const RATE: u32 = 48_000;
/// The engine's sync step: eight blocks of 256 frames at 48 kHz, which is 42.7 ms.
const STEP_SECONDS: f64 = 8.0 * 256.0 / 48_000.0;

/// A deck playing a tone, with a grid, so it has a beat to follow and a tempo to match.
///
/// `seeks` counts what the deck asked its source for: a jump is the sync loop's only reason to seek, so
/// the counter is how a test tells "one correction" from "a seek every step".
fn grid_track(
    bpm: f64,
    first_beat: u64,
    seeks: Arc<AtomicU32>,
) -> (Box<dyn dj_deck::DeckSource>, TrackMeta) {
    struct Tone {
        seeks: Arc<AtomicU32>,
    }
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
            self.seeks.fetch_add(1, Ordering::Relaxed);
            Ok(frame)
        }
        fn request_seek(&mut self, frame: u64) -> Result<u64, String> {
            self.seeks.fetch_add(1, Ordering::Relaxed);
            Ok(frame)
        }
    }
    let grid = BeatGrid::single(bpm, first_beat).expect("a grid of one tempo");
    let meta = TrackMeta {
        title: String::from("Tone"),
        artist: String::new(),
        duration_frames: Some(48_000 * 600),
        bpm: Some(bpm),
        first_beat_frame: Some(first_beat),
        grid: Some(grid),
        key: None,
    };
    (Box::new(Tone { seeks }), meta)
}

fn deck_with_grid(id: DeckId, bpm: f64, first_beat: u64, seeks: Arc<AtomicU32>) -> Deck {
    let (source, meta) = grid_track(bpm, first_beat, seeks);
    let mut deck = Deck::new(id);
    deck.attach(source, Some(meta));
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
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);

    // Two seconds of control ticks: the trim allows four percent a second, and the move is 6.67 %.
    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
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
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    let master_seeks = Arc::clone(&seeks);
    // Both running: a beat that stands still is not a beat to move onto, and a deck that is not
    // playing is not chasing one.
    decks[0].play();
    decks[1].play();
    // A quarter of a beat ahead of the master: 6000 frames at 120 BPM and 48 kHz.
    decks[1].seek(6_000);
    assert!((phase_of(&decks[1]) - 0.25).abs() < 1e-9);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let before = master_seeks.load(Ordering::Relaxed);

    let mut memory = SyncMemory::default();
    sync_step(&mut decks, &mut memory, STEP_SECONDS);

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
    assert_eq!(
        master_seeks.load(Ordering::Relaxed) - before,
        1,
        "one correction is one seek"
    );
}

#[test]
fn a_stopped_master_sets_the_tempo_but_never_drags_the_beat_back() {
    // The defect this test exists for: a paused master stands still, so a follower pulled onto its beat
    // is pulled back every step. Four seeks a second leave a deck reading from disk permanently silent
    // — the sound never arrives because the buffer is dropped before it can be played.
    let seeks = Arc::new(AtomicU32::new(0));
    let master_seeks = Arc::clone(&seeks);
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    // A is the master and it is paused; B follows.
    decks[1].seek(6_000);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let before = master_seeks.load(Ordering::Relaxed);

    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    let made = master_seeks.load(Ordering::Relaxed) - before;
    println!(
        "stopped master: {made} seeks over fifty steps, follower ratio {:.6}",
        decks[1].tempo_ratio()
    );
    assert_eq!(made, 0, "a beat that does not move is not a beat to follow");
    assert!(
        (decks[1].tempo_ratio() - 128.0 / 120.0).abs() < 1e-6,
        "the tempo is still the master's: {} ",
        decks[1].tempo_ratio()
    );
}

#[test]
fn one_jump_lands_before_another_is_asked_for() {
    // A jump is a seek, and a seek on a deck reading from disk is silence until the decoder catches up.
    // Asking again inside that window keeps the deck silent for as long as the loop runs.
    let seeks = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&seeks);
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    decks[0].play();
    decks[1].play();
    decks[1].seek(6_000);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let before = counter.load(Ordering::Relaxed);

    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    let made = counter.load(Ordering::Relaxed) - before;
    println!("jumps over fifty steps: {made}");
    assert_eq!(
        made, 1,
        "the first jump stands the deck on the beat; the rest hold it there"
    );
}

#[test]
fn sync_off_leaves_the_follower_alone() {
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    let mut mixer = mixer();
    // A master but no follower: nothing is asked of B.
    apply(
        &mut decks,
        &mut mixer,
        &command(1, DjCommand::Master { deck: DeckId::A }),
        0,
    );
    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    assert!(
        (decks[1].tempo_ratio() - 1.0).abs() < 1e-12,
        "B kept its speed"
    );
    assert_eq!(decks[1].position(), 0, "B was not moved");
}

#[test]
fn a_tempo_the_fader_cannot_reach_is_matched_anyway() {
    // 150 against 120 needs 25 %, and the fader allows 16. The rule was the opposite until the owner
    // said otherwise on 2026-11-10: a sync is the deck's own business, and a tempo the fader cannot
    // reach is still a tempo the DJ asked for. What the old rule left behind — a deck playing at the
    // speed of the track before it — was a wrong tempo nobody could see.
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 150.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    let ratio = decks[1].tempo_ratio();
    println!("beyond the fader: ratio {ratio:.9}, wanted 1.25");
    assert!(
        (ratio - 1.25).abs() < 1e-6,
        "the follower should reach 1.25 whatever the fader allows, it runs at {ratio:.9}"
    );
}

#[test]
fn one_deck_leads_at_a_time() {
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
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
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    decks[0].play();
    decks[1].play();
    decks[1].seek(6_000);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
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
    let seeks = Arc::new(AtomicU32::new(0));
    let mut deck = deck_with_grid(DeckId::B, 120.0, 0, seeks);
    deck.set_sync(SyncMode::Tempo);
    assert_eq!(deck.snapshot().sync, SyncMode::Tempo);
    deck.set_sync(SyncMode::Off);
    assert_eq!(deck.snapshot().sync, SyncMode::Off);
}

#[test]
fn a_stopped_follower_is_not_dragged_backwards() {
    // The owner's report: with the master playing, a stopped follower's play head crept backwards in
    // steps, in time with the phase meter. Every step the beat of a playing master moves on, and a jump
    // onto it pulls the stopped deck back by that much — the play head is dragged through the track by
    // a distance nobody asked for.
    let seeks = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&seeks);
    let mut decks = vec![
        deck_with_grid(DeckId::A, 120.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    decks[0].play();
    decks[1].seek(48_000);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let before = counter.load(Ordering::Relaxed);
    let paused_at = decks[1].position();

    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    let made = counter.load(Ordering::Relaxed) - before;
    println!(
        "stopped follower: {made} seeks, position {paused_at} -> {}",
        decks[1].position()
    );
    assert_eq!(made, 0, "a stopped deck is not chasing a beat");
    assert_eq!(
        decks[1].position(),
        paused_at,
        "the play head of a stopped deck stays where the hand left it"
    );
    assert!(
        (decks[1].tempo_ratio() - 1.0).abs() < 1e-9,
        "the tempo still follows: {}",
        decks[1].tempo_ratio()
    );
}

#[test]
fn a_started_follower_is_aligned_on_the_first_step() {
    // And the other side of the same rule: the moment it plays, it is placed on the beat.
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 120.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    decks[0].play();
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    // A second of standing still: nothing is jumped.
    for _ in 0..25 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    let aligned = decks[1].position();
    decks[1].set_tempo(1.0);
    decks[1].play();
    decks[1].seek(aligned + 9_000);
    sync_step(&mut decks, &mut memory, STEP_SECONDS);
    let apart_ms = (phase_of(&decks[1]) - phase_of(&decks[0])).abs() * 500.0;
    println!(
        "after the start: leader {} frames, follower {} frames, {apart_ms:.3} ms apart",
        decks[0].position(),
        decks[1].position()
    );
    assert!(
        apart_ms < 1.0,
        "a started follower is placed on the beat: {apart_ms:.3} ms"
    );
}

#[test]
fn the_bar_phase_tells_a_beat_behind_from_aligned() {
    // A beat-level phase says nothing about which beat: a follower a whole beat behind is "on the beat"
    // and a quarter of the bar away. The bar number is what the meter draws.
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 120.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    decks[0].play();
    decks[1].play();
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    // Aligned on the same beat: the bar reads zero.
    sync_step(&mut decks, &mut memory, STEP_SECONDS);
    let state = dj_engine::snapshot(&decks, &mixer, 1, 0, RATE, 1);
    let aligned = state
        .decks
        .iter()
        .find(|deck| deck.deck == DeckId::B)
        .expect("B");
    let aligned_bar = aligned.bar_error_beats.expect("a bar error is published");
    println!(
        "aligned: bar {aligned_bar:.4}, phase {:?} ms",
        aligned.phase_error_ms
    );
    assert!(
        aligned_bar.abs() < 0.02,
        "aligned reads zero: {aligned_bar:.4}"
    );

    // One beat (24000 frames at 120 BPM) further on: a quarter of the bar, and still on the beat.
    let here = decks[1].position();
    decks[1].seek(here + 24_000);
    let state = dj_engine::snapshot(&decks, &mixer, 1, 0, RATE, 1);
    let behind = state
        .decks
        .iter()
        .find(|deck| deck.deck == DeckId::B)
        .expect("B");
    let bar = behind.bar_error_beats.expect("a bar error is published");
    println!(
        "one beat along: bar {bar:.4}, phase {:?} ms",
        behind.phase_error_ms
    );
    assert!(
        (bar.abs() - 1.0).abs() < 0.02,
        "a whole beat along is a quarter of the bar: {bar:.4}"
    );
}

#[test]
fn putting_the_master_down_stops_following_and_leaves_the_tempo() {
    // The owner's rule: when the master goes, the mode goes with it and the deck plays on exactly as it
    // was — the tempo a hand gave it is a decision, not a state to undo.
    let seeks = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&seeks);
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    decks[0].play();
    decks[1].play();
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    let mut block = vec![0.0f32; 256 * 2];
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
        // A playing deck approaches a tempo by gliding over the blocks it renders, the way the engine
        // does: a step without sound would leave the speed where the glide started.
        for _ in 0..8 {
            decks[1].render(&mut block);
        }
    }
    let trimmed = decks[1].tempo_ratio();
    // The glide approaches its target rather than stamping it: within a tenth of a percent is "on the
    // master's tempo" for a deck that has been following for two seconds.
    assert!(
        (trimmed - 128.0 / 120.0).abs() < 1e-3,
        "the follower was on the master's tempo: {trimmed:.9}"
    );

    let ack = apply(&mut decks, &mut mixer, &command(3, DjCommand::MasterOff), 0);
    assert!(
        matches!(ack.status, AckStatus::Applied { .. }),
        "{:?}",
        ack.status
    );
    assert!(!decks[0].master() && !decks[1].master(), "nobody leads");
    assert_eq!(
        decks[1].sync(),
        SyncMode::Off,
        "the mode goes with the leader"
    );
    assert_eq!(
        decks[1].tempo_ratio(),
        trimmed,
        "the tempo stays where the hand left it"
    );

    // And no further step touches the deck: no trims, no jumps.
    let position = decks[1].position();
    let before = counter.load(Ordering::Relaxed);
    for _ in 0..25 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    println!(
        "after the master went: {made} seeks, ratio {:.9}, head {position} -> {}",
        decks[1].tempo_ratio(),
        decks[1].position(),
        made = counter.load(Ordering::Relaxed) - before
    );
    assert_eq!(
        counter.load(Ordering::Relaxed) - before,
        0,
        "nothing to chase"
    );
    assert_eq!(decks[1].position(), position, "the head stays where it was");
    assert_eq!(decks[1].tempo_ratio(), trimmed, "and so does the speed");
}

#[test]
fn a_lone_deck_does_not_lead_itself() {
    // Before this rule a single playing deck picked itself as master, so sync did nothing at all while
    // looking like it should: the follower sat on the beat of a leader that was itself.
    let seeks = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&seeks);
    let mut decks = vec![deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks))];
    decks[0].play();
    decks[0].set_sync(SyncMode::Beat);
    let frames = decks[0].position();
    let before = counter.load(Ordering::Relaxed);
    let mut memory = SyncMemory::default();
    for _ in 0..25 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    assert_eq!(counter.load(Ordering::Relaxed) - before, 0);
    assert_eq!(decks[0].position(), frames);
    assert!((decks[0].tempo_ratio() - 1.0).abs() < 1e-12);
}

#[test]
fn a_stopped_follower_still_says_where_its_head_stands() {
    // A deck that is not playing has no position in time, but it has a place: the distance from its head
    // to the master's beat is what a hand asks before pressing play. So it is published, and it moves
    // with the master while the head itself stays where it was left.
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 120.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    decks[0].play();
    decks[1].seek(48_000);
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    sync_step(&mut decks, &mut memory, STEP_SECONDS);

    let head = decks[1].position();
    let read = |decks: &[Deck]| {
        let state = dj_engine::snapshot(decks, &mixer, 1, 0, RATE, 1);
        state
            .decks
            .iter()
            .find(|deck| deck.deck == DeckId::B)
            .and_then(|deck| deck.phase_error_ms)
            .expect("a stopped follower still says where it stands")
    };
    let first = read(&decks);
    // The master moves a quarter of a beat on: 6000 frames at 120 BPM.
    let master_head = decks[0].position();
    decks[0].seek(master_head + 6_000);
    let second = read(&decks);
    println!(
        "stopped follower: {first:.3} ms then {second:.3} ms, head {head} -> {}",
        decks[1].position()
    );
    assert!(
        (second - first).abs() > 100.0,
        "the distance moves with the master: {first:.3} then {second:.3}"
    );
    assert_eq!(decks[1].position(), head, "and the head itself stays put");
}

/// A deck follows a tempo, not a file: a new track under a deck that is already following has its own
/// BPM, and keeping the pitch of the track before it leaves the mix at the wrong speed without saying so.
#[test]
fn a_new_track_under_a_following_deck_takes_the_master_tempo() {
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    let before = decks[1].tempo_ratio();
    assert!(
        (before - 128.0 / 120.0).abs() < 1e-6,
        "the follower started on the master's tempo, it runs at {before:.9}"
    );

    // The hand loads a slower track into the following deck and sync stays on.
    let (source, meta) = grid_track(100.0, 0, Arc::clone(&seeks));
    decks[1].attach(source, Some(meta));
    for _ in 0..50 {
        sync_step(&mut decks, &mut memory, STEP_SECONDS);
    }
    let after = decks[1].tempo_ratio();
    println!(
        "new track: ratio {before:.9} -> {after:.9}, wanted {:.9}",
        128.0 / 100.0
    );
    assert!(
        (after - 128.0 / 100.0).abs() < 1e-6,
        "the new track should run at the master's tempo, it runs at {after:.9}"
    );
}

/// Pressing sync means the tempo changes now: a deck that spends the next seconds climbing into the
/// mix is a deck that is not following yet.
#[test]
fn pressing_sync_takes_the_tempo_at_once() {
    let seeks = Arc::new(AtomicU32::new(0));
    let mut decks = vec![
        deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
        deck_with_grid(DeckId::B, 120.0, 0, Arc::clone(&seeks)),
    ];
    let mut mixer = mixer();
    lock_b_to_a(&mut decks, &mut mixer);
    let mut memory = SyncMemory::default();
    // The mode comes on with the press; the first step after it is the press.
    sync_step(&mut decks, &mut memory, STEP_SECONDS);
    let ratio = decks[1].tempo_ratio();
    println!("after the press: ratio {ratio:.9}");
    assert!(
        (ratio - 128.0 / 120.0).abs() < 1e-6,
        "one step after the press the deck should be on the master's tempo, it runs at {ratio:.9}"
    );
}

/// A follower stretched by 28 % is still a follower: the phase it holds is the phase a follower
/// stretched by 6 % holds. The stretch is a multiplier on the rate, not a licence for drift.
#[test]
fn a_stretched_follower_holds_the_beat_as_tightly_as_a_near_one() {
    for (file_bpm, label) in [(120.0, "6 %"), (100.0, "28 %")] {
        let seeks = Arc::new(AtomicU32::new(0));
        let mut decks = vec![
            deck_with_grid(DeckId::A, 128.0, 0, Arc::clone(&seeks)),
            deck_with_grid(DeckId::B, file_bpm, 0, Arc::clone(&seeks)),
        ];
        let mut mixer = mixer();
        lock_b_to_a(&mut decks, &mut mixer);
        let mut memory = SyncMemory::default();
        let mut worst: f64 = 0.0;
        for _ in 0..400 {
            sync_step(&mut decks, &mut memory, STEP_SECONDS);
            let mut apart = (phase_of(&decks[1]) - phase_of(&decks[0])).rem_euclid(1.0);
            if apart > 0.5 {
                apart -= 1.0;
            }
            let ms = (apart * 60_000.0 / 128.0).abs();
            worst = worst.max(ms);
        }
        println!(
            "{label}: ratio {:.9}, worst phase {worst:.4} ms over 400 steps",
            decks[1].tempo_ratio()
        );
        assert!(
            worst <= 2.0,
            "{label}: a follower held {worst:.3} ms off the beat over four hundred steps"
        );
    }
}
