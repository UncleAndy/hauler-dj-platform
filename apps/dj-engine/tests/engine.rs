//! The engine's rules, tested without spawning a process.
//!
//! An earlier version of this file started the binary and talked to it over a socket. It hung the
//! gate: a spawned server inherits the harness pipes, and a missing message has no deadline. The
//! socket path is covered by `dj-bridge`'s own tests; what belongs here is what a command *does*.

use dj_deck::Deck;
use dj_domain::{
    AckStatus, CueAction, DeckId, DjCommand, RejectReason, ScheduledCommand, SourceFrame,
};
use dj_engine::{apply, snapshot};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

fn deck_with_tone(id: DeckId, rate: u32) -> Deck {
    struct Tone;
    impl dj_deck::DeckSource for Tone {
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn frames(&self) -> Option<u64> {
            Some(480_000)
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
    let mut deck = Deck::new(id);
    deck.attach(Box::new(Tone), None);
    deck.set_blocking(false);
    deck.set_gain(1.0);
    let _ = rate;
    deck
}

fn mixer_for(rate: u32) -> Mixer {
    let mut mixer = Mixer::new(2);
    mixer.prepare(rate, 256);
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

#[test]
fn play_is_applied_at_a_device_sample_and_visible_in_the_state() {
    let mut decks = vec![
        deck_with_tone(DeckId::A, 48_000),
        deck_with_tone(DeckId::B, 48_000),
    ];
    let mut mixer = mixer_for(48_000);

    let ack = apply(
        &mut decks,
        &mut mixer,
        &command(
            1,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true,
            },
        ),
        4_096,
    );
    assert_eq!(ack.command_id, 1);
    assert!(
        matches!(ack.status, AckStatus::Applied { at_sample } if at_sample >= 4_096),
        "a command is acknowledged as applied, not merely queued: {:?}",
        ack.status
    );

    let state = snapshot(&decks, &mixer, 1, 4_096, 48_000, 7);
    let deck_a = state
        .decks
        .iter()
        .find(|deck| deck.deck == DeckId::A)
        .unwrap_or_else(|| panic!("deck A must be in the snapshot"));
    assert!(deck_a.playing, "the state must reflect the ack");
    assert_eq!(state.protocol, dj_domain::STATE_PROTOCOL);
    assert_eq!(state.audio_clock_samples, 4_096);
}

#[test]
fn stop_is_applied_too() {
    let mut decks = vec![
        deck_with_tone(DeckId::A, 48_000),
        deck_with_tone(DeckId::B, 48_000),
    ];
    let mut mixer = mixer_for(48_000);
    apply(
        &mut decks,
        &mut mixer,
        &command(
            1,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true,
            },
        ),
        0,
    );
    let ack = apply(
        &mut decks,
        &mut mixer,
        &command(
            2,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: false,
            },
        ),
        512,
    );
    assert!(matches!(ack.status, AckStatus::Applied { .. }));
    let state = snapshot(&decks, &mixer, 2, 512, 48_000, 7);
    let deck_a = state
        .decks
        .iter()
        .find(|deck| deck.deck == DeckId::A)
        .unwrap_or_else(|| panic!("deck A"));
    assert!(!deck_a.playing);
}

#[test]
fn tempo_is_refused_by_name_because_resampling_does_not_exist_yet() {
    let mut decks = vec![
        deck_with_tone(DeckId::A, 48_000),
        deck_with_tone(DeckId::B, 48_000),
    ];
    let mut mixer = mixer_for(48_000);
    let ack = apply(
        &mut decks,
        &mut mixer,
        &command(
            3,
            DjCommand::SetTempoRatio {
                deck: DeckId::A,
                ratio: 1.05,
            },
        ),
        0,
    );
    assert!(
        matches!(ack.status, AckStatus::Rejected(RejectReason::NotSupported)),
        "an unimplemented command must be refused explicitly, got {:?}",
        ack.status
    );
}

#[test]
fn a_command_for_a_deck_that_does_not_exist_is_invalid_not_silent() {
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);
    let ack = apply(
        &mut decks,
        &mut mixer,
        &command(
            4,
            DjCommand::Play {
                deck: DeckId::C,
                enabled: true,
            },
        ),
        0,
    );
    assert!(matches!(
        ack.status,
        AckStatus::Rejected(RejectReason::InvalidValue)
    ));
}

#[test]
fn a_past_deadline_is_clamped_to_now_rather_than_dropped() {
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);
    let mut late = command(
        5,
        DjCommand::SeekFrames {
            deck: DeckId::A,
            frame: SourceFrame(1_000),
        },
    );
    late.effective_sample = 10; // long past, the clock is already at 5 000
    let ack = apply(&mut decks, &mut mixer, &late, 5_000);
    match ack.status {
        AckStatus::Applied { at_sample } => assert_eq!(
            at_sample, 5_000,
            "a command that asked for the past is applied now, not silently discarded"
        ),
        other => panic!("expected the command to be applied, got {other:?}"),
    }
}

#[test]
fn hotcues_round_trip_through_the_command_path() {
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);

    let stored = apply(
        &mut decks,
        &mut mixer,
        &command(
            6,
            DjCommand::Cue {
                deck: DeckId::A,
                action: CueAction::Set(3),
            },
        ),
        0,
    );
    assert!(matches!(stored.status, AckStatus::Applied { .. }));

    let state = snapshot(&decks, &mixer, 1, 0, 48_000, 7);
    assert!(
        state.decks[0].hotcues[3].is_some(),
        "the stored hotcue must appear in the state"
    );

    let jumped = apply(
        &mut decks,
        &mut mixer,
        &command(
            7,
            DjCommand::Cue {
                deck: DeckId::A,
                action: CueAction::Jump(3),
            },
        ),
        0,
    );
    assert!(matches!(jumped.status, AckStatus::Applied { .. }));

    let empty = apply(
        &mut decks,
        &mut mixer,
        &command(
            8,
            DjCommand::Cue {
                deck: DeckId::A,
                action: CueAction::Jump(5),
            },
        ),
        0,
    );
    assert!(
        matches!(
            empty.status,
            AckStatus::Rejected(RejectReason::InvalidValue)
        ),
        "jumping to an empty slot is refused, not ignored"
    );
}
