//! The wire format, pinned byte for byte.
//!
//! `command-v1`/`state-v1` are consumed by a separate process (ADR-003), so field names, the
//! externally-tagged enum shape and the units are a contract, not an implementation detail.
//! These tests fail the moment serde's output changes shape — renaming a field then requires
//! bumping the protocol version, which is exactly the conversation we want to force.

use dj_domain::{
    Ack, CommandId, DeckId, DeckSnapshot, DjCommand, FrameRange, RejectReason, STATE_PROTOCOL,
    ScheduledCommand, Snapshot, SourceFrame,
};

fn play() -> DjCommand {
    DjCommand::Play {
        deck: DeckId::A,
        enabled: true,
    }
}

#[test]
fn a_transport_command_is_tagged_with_its_variant_name() {
    assert_eq!(
        serde_json::to_string(&play()).unwrap_or_default(),
        r#"{"Play":{"deck":"A","enabled":true}}"#
    );
}

#[test]
fn a_continuous_control_names_its_deck_and_units() {
    let gain = DjCommand::SetChannelGain {
        deck: DeckId::B,
        linear_gain: 0.5,
    };
    assert_eq!(
        serde_json::to_string(&gain).unwrap_or_default(),
        r#"{"SetChannelGain":{"deck":"B","linear_gain":0.5}}"#
    );
}

#[test]
fn a_loop_region_is_a_pair_of_source_frames() {
    let command = DjCommand::SetLoop {
        deck: DeckId::A,
        region: FrameRange::new(1_000, 2_000),
    };
    assert_eq!(
        serde_json::to_string(&command).unwrap_or_default(),
        r#"{"SetLoop":{"deck":"A","region":{"start":1000,"end":2000}}}"#
    );
}

#[test]
fn a_scheduled_command_carries_id_sample_and_command() {
    let scheduled = ScheduledCommand {
        id: 7,
        effective_sample: 5_333,
        command: play(),
    };
    assert_eq!(
        serde_json::to_string(&scheduled).unwrap_or_default(),
        r#"{"id":7,"effective_sample":5333,"command":{"Play":{"deck":"A","enabled":true}}}"#
    );
}

#[test]
fn acks_report_applied_sample_and_rejection_reason() {
    assert_eq!(
        serde_json::to_string(&Ack::applied(7, 5_333)).unwrap_or_default(),
        r#"{"command_id":7,"status":{"Applied":{"at_sample":5333}}}"#
    );
    assert_eq!(
        serde_json::to_string(&Ack::rejected(8, RejectReason::QueueFull)).unwrap_or_default(),
        r#"{"command_id":8,"status":{"Rejected":"QueueFull"}}"#
    );
}

#[test]
fn a_snapshot_is_self_describing_enough_to_be_refused() {
    let snapshot = Snapshot {
        protocol: STATE_PROTOCOL.to_string(),
        revision: 3,
        epoch: 1,
        audio_clock_samples: 4_800,
        sample_rate: 48_000,
        crossfader: 0.25,
        decks: vec![DeckSnapshot {
            deck: DeckId::A,
            playing: true,
            source_frame: SourceFrame(4_800),
            linear_gain: 1.0,
            tempo_ratio: 1.0,
        }],
    };
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap_or_default(),
        concat!(
            r#"{"protocol":"state-v1","revision":3,"epoch":1,"audio_clock_samples":4800,"#,
            r#""sample_rate":48000,"crossfader":0.25,"decks":[{"deck":"A","playing":true,"#,
            r#""source_frame":4800,"linear_gain":1.0,"tempo_ratio":1.0}]}"#
        )
    );
}

#[test]
fn the_pinned_strings_parse_back_into_the_same_values() {
    let json = r#"{"id":9,"effective_sample":256,"command":{"Play":{"deck":"C","enabled":false}}}"#;
    let scheduled: ScheduledCommand =
        serde_json::from_str(json).unwrap_or_else(|err| panic!("parse: {err}"));
    assert_eq!(scheduled.id as CommandId, 9);
    assert_eq!(scheduled.effective_sample, 256);
    assert_eq!(
        scheduled.command,
        DjCommand::Play {
            deck: DeckId::C,
            enabled: false
        }
    );
}

#[test]
fn an_unknown_variant_is_rejected_instead_of_ignored() {
    let json = r#"{"Play":true}"#;
    assert!(
        serde_json::from_str::<DjCommand>(json).is_err(),
        "a malformed command must not silently deserialise"
    );
}
