//! The wire format, pinned byte for byte.
//!
//! `command-v1`/`state-v1` are consumed by a separate process (ADR-003), so field names, the
//! externally-tagged enum shape and the units are a contract, not an implementation detail.
//! These tests fail the moment serde's output changes shape — renaming a field then requires
//! bumping the protocol version, which is exactly the conversation we want to force.

use dj_domain::{
    Ack, CommandId, CrossfaderCurve, DeckId, DeckLoad, DeckSnapshot, DjCommand, EqBand, EqGains,
    FrameRange, Levels, RejectReason, STATE_PROTOCOL, ScheduledCommand, Snapshot, SourceFrame,
    TrackMeta,
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
fn a_fader_is_a_position_and_not_a_gain() {
    let command = DjCommand::SetChannelFader {
        deck: DeckId::C,
        position: 0.5,
    };
    assert_eq!(
        serde_json::to_string(&command).unwrap_or_default(),
        r#"{"SetChannelFader":{"deck":"C","position":0.5}}"#
    );
}

#[test]
fn an_eq_knob_names_its_band_and_the_deck_it_belongs_to() {
    let command = DjCommand::SetChannelEq {
        deck: DeckId::B,
        band: EqBand::Mid,
        gain_db: -4.5,
    };
    assert_eq!(
        serde_json::to_string(&command).unwrap_or_default(),
        r#"{"SetChannelEq":{"deck":"B","band":"mid","gain_db":-4.5}}"#
    );
}

#[test]
fn a_snapshot_from_before_the_eq_still_reads_as_flat() {
    // A client of the previous build must keep working: the field is defaulted, and three bands at
    // unity is what a channel without an EQ is doing.
    let json = r#"{"deck":"A","load":"Empty","playing":false,"source_frame":0,
        "remaining_frames":null,"position_ratio":null,"track":null,"loop_region":null,
        "hotcues":[null,null,null,null,null,null,null,null],
        "levels":{"peak_left":0.0,"peak_right":0.0},"linear_gain":1.0,"tempo_ratio":1.0}"#;
    let deck: DeckSnapshot = serde_json::from_str(json).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(deck.eq, EqGains::FLAT);
    assert!(deck.eq.is_flat());
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
        crossfader_curve: CrossfaderCurve::Smooth,
        master: dj_domain::Levels::default(),
        master_gain: 1.0,
        decks: vec![DeckSnapshot {
            waveform: None,
            deck: DeckId::A,
            load: DeckLoad::Ready,
            playing: true,
            source_frame: SourceFrame(4_800),
            remaining_frames: Some(120_000),
            position_ratio: Some(0.0384),
            track: Some(TrackMeta {
                title: "Track".to_string(),
                artist: "Artist".to_string(),
                duration_frames: Some(124_800),
                bpm: Some(128.0),
                key: Some("8A".to_string()),
            }),
            loop_region: None,
            hotcues: vec![
                Some(SourceFrame(1_000)),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            // A named and coloured pad: the wire format covers the whole record, not only the frame.
            cue_labels: vec![
                Some("drop".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            cue_colors: vec![
                Some("#5fb85f".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            levels: Levels {
                peak_left: 0.5,
                peak_right: 0.25,
            },
            linear_gain: 1.0,
            tempo_ratio: 1.0,
            fader: 0.85,
            eq: EqGains {
                low_db: 3.0,
                mid_db: -4.5,
                high_db: 0.0,
            },
        }],
    };
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap_or_default(),
        concat!(
            r#"{"protocol":"state-v1","revision":3,"epoch":1,"audio_clock_samples":4800,"#,
            r#""sample_rate":48000,"crossfader":0.25,"crossfader_curve":"smooth","#,
            r#""master":{"peak_left":0.0,"peak_right":0.0},"master_gain":1.0,"#,
            r#""decks":[{"deck":"A","#,
            r#""load":"Ready","playing":true,"source_frame":4800,"remaining_frames":120000,"#,
            r#""position_ratio":0.0384,"track":{"title":"Track","artist":"Artist","#,
            r#""duration_frames":124800,"bpm":128.0,"key":"8A"},"loop_region":null,"#,
            r#""hotcues":[1000,null,null,null,null,null,null,null],"#,
            r#""cue_labels":["drop",null,null,null,null,null,null,null],"#,
            r##""cue_colors":["#5fb85f",null,null,null,null,null,null,null],"##,
            r#""levels":{"peak_left":0.5,"peak_right":0.25},"#,
            r#""linear_gain":1.0,"tempo_ratio":1.0,"fader":0.85,"#,
            r#""eq":{"low_db":3.0,"mid_db":-4.5,"high_db":0.0}}]}"#
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
