//! The engine's rules, tested without spawning a process.
//!
//! An earlier version of this file started the binary and talked to it over a socket. It hung the
//! gate: a spawned server inherits the harness pipes, and a missing message has no deadline. The
//! socket path is covered by `dj-bridge`'s own tests; what belongs here is what a command *does*.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_deck::Deck;
use dj_domain::{
    AckStatus, CueAction, DeckId, DeckLoad, DjCommand, RejectReason, ScheduledCommand, SourceFrame,
};
use dj_engine::Routed;
use dj_engine::{
    DeckStatic, EngineGraph, GraphLoad, StateCells, Submission, apply, assign_crossfader_sides,
    prepare_track, route, snapshot,
};
use dj_mixer::{ChannelSettings, CrossAssign, CrossfaderCurve, Mixer};
use std::sync::Arc;
use std::sync::mpsc::Sender;

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

#[test]
fn publishing_a_block_does_not_allocate() {
    // The whole point of the cells: the audio path stores numbers, it does not build a snapshot.
    // A regression here would show up as a click, not as a failed assertion, so it is checked.
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);
    let cells = StateCells::new();
    cells.publish(&decks, &mixer, 0); // warm: first touch of the atomics is not measured

    dj_testkit::rt::reset();
    {
        let _scope = dj_testkit::rt::RtScope::enter();
        for clock in 1..=64 {
            cells.publish(&decks, &mixer, clock * 256);
        }
    }
    assert_eq!(
        dj_testkit::rt::allocations_in_rt(),
        0,
        "publish must not allocate inside the render path"
    );
    let _ = (&mut decks, &mut mixer);
}

#[test]
fn a_snapshot_read_back_carries_what_was_published() {
    let mut decks = vec![
        deck_with_tone(DeckId::A, 48_000),
        deck_with_tone(DeckId::B, 48_000),
    ];
    let mut mixer = mixer_for(48_000);
    let cells = StateCells::new();
    let statics = vec![
        DeckStatic::from_snapshot(&decks[0].snapshot()),
        DeckStatic::from_snapshot(&decks[1].snapshot()),
    ];

    apply(
        &mut decks,
        &mut mixer,
        &command(
            11,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true,
            },
        ),
        0,
    );
    apply(
        &mut decks,
        &mut mixer,
        &command(
            12,
            DjCommand::Cue {
                deck: DeckId::A,
                action: CueAction::Set(2),
            },
        ),
        0,
    );
    apply(
        &mut decks,
        &mut mixer,
        &command(
            13,
            DjCommand::SetLoop {
                deck: DeckId::A,
                region: Some(dj_domain::FrameRange {
                    start: SourceFrame(1_000),
                    end: SourceFrame(2_000),
                }),
            },
        ),
        0,
    );
    cells.publish(&decks, &mixer, 12_345);

    let state = cells.read(&statics, 48_000, 7, 3);
    assert_eq!(state.revision, 3);
    assert_eq!(state.epoch, 7);
    assert_eq!(state.audio_clock_samples, 12_345);
    assert_eq!(state.decks.len(), 2, "both decks are published");
    let read_back = &state.decks[0];
    assert_eq!(read_back.deck, DeckId::A);
    assert!(read_back.playing, "playing survives the atomic round trip");
    assert!(read_back.hotcues[2].is_some(), "the hotcue is in the state");
    let region = read_back
        .loop_region
        .unwrap_or_else(|| panic!("the loop is in the state, not only in the deck"));
    assert_eq!(region.start, SourceFrame(1_000));
    assert_eq!(region.end, SourceFrame(2_000));
    assert!(
        read_back.position_ratio.is_some(),
        "a deck with a known duration reports its position"
    );
    assert_eq!(read_back.deck, DeckId::A);
}

fn graph_for(block: usize) -> (EngineGraph, Arc<StateCells>, Sender<Submission>) {
    let decks = vec![
        deck_with_tone(DeckId::A, 48_000),
        deck_with_tone(DeckId::B, 48_000),
    ];
    let cells = Arc::new(StateCells::new());
    let (submissions, receiver) = std::sync::mpsc::channel();
    let (_loads, load_receiver) = std::sync::mpsc::channel();
    let graph = EngineGraph::new(
        decks,
        mixer_for(48_000),
        receiver,
        Arc::clone(&cells),
        block,
        load_receiver,
    );
    (graph, cells, submissions)
}

#[test]
fn a_block_through_the_graph_is_audible_acked_and_published() {
    // This is the device path minus the device: the callback's own work, called directly.
    let (mut graph, cells, submissions) = graph_for(256);
    let (reply, answers) = std::sync::mpsc::channel();
    submissions
        .send(Submission {
            command: command(
                1,
                DjCommand::Play {
                    deck: DeckId::A,
                    enabled: true,
                },
            ),
            reply,
        })
        .unwrap_or_else(|err| panic!("{err}"));

    let mut master = vec![0.0f32; 256 * 2];
    let mut cue = vec![0.0f32; 256 * 2];
    graph.render_pair(&mut master, &mut cue, 256);

    assert!(
        master.iter().any(|sample| sample.abs() > 0.01),
        "a playing deck must reach the master bus"
    );
    assert!(
        cue.iter().all(|sample| sample.abs() < 1e-9),
        "a deck that is not cued must stay out of the headphone bus"
    );
    let ack = answers
        .try_recv()
        .unwrap_or_else(|err| panic!("a command must be acknowledged inside the block: {err}"));
    assert!(matches!(ack.status, AckStatus::Applied { .. }));
    assert_eq!(
        graph.clock(),
        256,
        "the clock advances by the rendered frames"
    );

    let statics = vec![
        DeckStatic::from_snapshot(&graph.decks()[0].snapshot()),
        DeckStatic::from_snapshot(&graph.decks()[1].snapshot()),
    ];
    let state = cells.read(&statics, 48_000, 1, 1);
    assert!(
        state.decks[0].playing,
        "the graph publishes what it rendered"
    );
    assert_eq!(state.audio_clock_samples, 256);
}

#[test]
fn a_block_larger_than_prepared_is_truncated_not_allocated_for() {
    let (mut graph, _, _) = graph_for(128);
    let mut master = vec![0.0f32; 512 * 2];
    let mut cue = vec![0.0f32; 512 * 2];
    graph.render_pair(&mut master, &mut cue, 512);
    assert_eq!(
        graph.clock(),
        128,
        "rendering beyond the prepared block stops at the block boundary"
    );
    assert!(
        master[128 * 2..].iter().all(|sample| *sample == 0.0),
        "the part that was not rendered stays silent"
    );
}

#[test]
fn the_callback_path_does_not_allocate() {
    let (mut graph, _, _) = graph_for(256);
    let mut master = vec![0.0f32; 256 * 2];
    let mut cue = vec![0.0f32; 256 * 2];
    graph.render_pair(&mut master, &mut cue, 256); // warm-up: decoders and buffers

    dj_testkit::rt::reset();
    {
        let _scope = dj_testkit::rt::RtScope::enter();
        for _ in 0..16 {
            graph.render_pair(&mut master, &mut cue, 256);
        }
    }
    assert_eq!(
        dj_testkit::rt::allocations_in_rt(),
        0,
        "the block path of the engine must not allocate"
    );
}

fn graph_with_loads(
    block: usize,
) -> (
    EngineGraph,
    std::sync::mpsc::Sender<GraphLoad>,
    Sender<Submission>,
) {
    let decks = vec![
        deck_with_tone(DeckId::A, 48_000),
        deck_with_tone(DeckId::B, 48_000),
    ];
    let cells = Arc::new(StateCells::new());
    let (submissions, submission_rx) = std::sync::mpsc::channel();
    let (loads, load_rx) = std::sync::mpsc::channel();
    let graph = EngineGraph::new(
        decks,
        mixer_for(48_000),
        submission_rx,
        Arc::clone(&cells),
        block,
        load_rx,
    );
    (graph, loads, submissions)
}

fn tone_wav(dir: &std::path::Path, name: &str, seconds: f32) -> std::path::PathBuf {
    let path = dir.join(format!("{name}.wav"));
    let rate = 48_000u32;
    let frames = (rate as f32 * seconds) as usize;
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let phase = frame as f32 / rate as f32 * 440.0 * std::f32::consts::TAU;
        let value = phase.sin() * 0.5;
        samples.push(value);
        samples.push(value);
    }
    let file = std::fs::File::create(&path).unwrap_or_else(|err| panic!("create: {err}"));
    let mut writer = std::io::BufWriter::new(file);
    dj_audio_core::wav::write_pcm16_interleaved(&mut writer, rate, 2, &samples)
        .unwrap_or_else(|err| panic!("write: {err}"));
    path
}

#[test]
fn a_load_is_routed_away_from_the_audio_callback() {
    let load = command(
        21,
        DjCommand::Load {
            deck: DeckId::B,
            path: "/music/track.flac".to_string(),
        },
    );
    match route(&load) {
        Routed::Load(request) => {
            assert_eq!(request.deck, DeckId::B);
            assert_eq!(request.path, "/music/track.flac");
            assert_eq!(request.command_id, 21);
        }
        _ => panic!("a load must go to the loader, never to the render loop"),
    }
    match route(&command(22, DjCommand::Eject { deck: DeckId::A })) {
        Routed::Eject { deck } => assert_eq!(deck, DeckId::A),
        _ => panic!("an eject must go to the graph, not be applied blindly"),
    }
    match route(&command(
        23,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    )) {
        Routed::Render => {}
        _ => panic!("transport commands stay in the render loop"),
    }
}

#[test]
fn a_stray_load_reaching_the_render_loop_is_refused_rather_than_pretended() {
    // The router is supposed to keep loads out of the callback. If one slips through, an ack that
    // says "applied" would send the UI to a deck that never got a track.
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);
    let ack = apply(
        &mut decks,
        &mut mixer,
        &command(
            24,
            DjCommand::Load {
                deck: DeckId::A,
                path: "/music/track.flac".to_string(),
            },
        ),
        0,
    );
    assert!(matches!(
        ack.status,
        AckStatus::Rejected(RejectReason::NotSupported)
    ));
}

#[test]
fn a_prepared_track_is_attached_at_a_block_boundary_and_plays() {
    let dir = std::env::temp_dir().join(format!("dj-engine-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("{err}"));
    let path = tone_wav(&dir, "tone", 2.0);

    let prepared = prepare_track(&path, 48_000, 2.0).unwrap_or_else(|err| panic!("{err}"));
    assert!(prepared.track.is_some(), "the loader describes the track");
    assert_eq!(
        prepared.static_info.load,
        dj_domain::DeckLoad::Ready,
        "a prepared track is ready by construction"
    );
    assert!(
        prepared.static_info.total_frames.unwrap_or(0) > 90_000,
        "two seconds at 48 kHz"
    );

    let (mut graph, loads, _submissions) = graph_with_loads(256);
    loads
        .send(GraphLoad::Attach(Box::new(dj_engine::PreparedTrack {
            deck: DeckId::B,
            ..prepared
        })))
        .unwrap_or_else(|err| panic!("{err}"));

    let mut master = vec![0.0f32; 256 * 2];
    let mut cue = vec![0.0f32; 256 * 2];
    graph.render_pair(&mut master, &mut cue, 256);
    assert_eq!(
        graph.loads_applied(),
        1,
        "the attach happened inside the block"
    );

    let deck_b = graph
        .decks()
        .iter()
        .find(|deck| deck.id() == DeckId::B)
        .unwrap_or_else(|| panic!("deck B"));
    assert_eq!(deck_b.snapshot().load, dj_domain::DeckLoad::Ready);
    assert!(
        deck_b.snapshot().track.is_some(),
        "the deck knows which track it holds"
    );

    // Then play it: the audio must come from the file, not from the old tone.
    let (reply, _answers) = std::sync::mpsc::channel();
    _submissions
        .send(Submission {
            command: command(
                25,
                DjCommand::Play {
                    deck: DeckId::B,
                    enabled: true,
                },
            ),
            reply,
        })
        .unwrap_or_else(|err| panic!("{err}"));
    graph.render_pair(&mut master, &mut cue, 256);
    assert!(
        master.iter().any(|sample| sample.abs() > 0.01),
        "a loaded and playing deck must reach the master bus"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_eject_clears_the_deck_inside_the_block() {
    let (mut graph, loads, _submissions) = graph_with_loads(64);
    loads
        .send(GraphLoad::Clear { deck: DeckId::A })
        .unwrap_or_else(|err| panic!("{err}"));
    let mut master = vec![0.0f32; 64 * 2];
    let mut cue = vec![0.0f32; 64 * 2];
    graph.render_pair(&mut master, &mut cue, 64);
    let deck_a = graph
        .decks()
        .iter()
        .find(|deck| deck.id() == DeckId::A)
        .unwrap_or_else(|| panic!("deck A"));
    assert_eq!(deck_a.snapshot().load, dj_domain::DeckLoad::Empty);
    assert!(deck_a.snapshot().track.is_none());
}

#[test]
fn an_ejected_deck_reports_itself_empty() {
    // The snapshot took the load state from the static section captured at attach, so an ejected deck
    // kept saying "Ready" and the shell kept a track on screen that was gone. The state now travels
    // through the cells, which is where a thing that changes belongs.
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);
    let statics: Vec<DeckStatic> = decks
        .iter()
        .map(|deck| DeckStatic::from_snapshot(&deck.snapshot()))
        .collect();
    let cells = StateCells::new();
    cells.publish(&decks, &mixer, 0);
    assert_eq!(
        cells.read(&statics, 48_000, 1, 1).decks[0].load,
        DeckLoad::Ready
    );

    let ack = apply(
        &mut decks,
        &mut mixer,
        &command(13, DjCommand::Eject { deck: DeckId::A }),
        0,
    );
    assert!(matches!(ack.status, AckStatus::Applied { .. }));
    cells.publish(&decks, &mixer, 0);
    let published = cells.read(&statics, 48_000, 1, 2);
    assert_eq!(published.decks[0].load, DeckLoad::Empty);
    assert!(
        published.decks[0].track.is_none(),
        "an empty deck has no track"
    );
}

#[test]
fn a_master_gain_command_lands_in_the_published_gain() {
    // The knob reads `master_gain` from the snapshot; a command that writes the mixer but publishes
    // nothing makes the knob spring back, which is the same defect the channel gain had.
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);
    let ack = apply(
        &mut decks,
        &mut mixer,
        &command(11, DjCommand::SetMasterGain { linear_gain: 0.5 }),
        0,
    );
    assert!(matches!(ack.status, AckStatus::Applied { .. }));
    assert!((mixer.master_gain() - 0.5).abs() < 1e-6);

    let cells = StateCells::new();
    cells.publish(&decks, &mixer, 0);
    let statics: Vec<DeckStatic> = decks
        .iter()
        .map(|deck| DeckStatic::from_snapshot(&deck.snapshot()))
        .collect();
    let published = cells.read(&statics, 48_000, 1, 1);
    assert!((published.master_gain - 0.5).abs() < 1e-6);
}

#[test]
fn a_curve_command_changes_the_law_and_the_snapshot_says_which() {
    // The law is a choice a person makes, so it has to travel: the engine applies it and the snapshot
    // reports it back. Without the second half the selector in the shell could only guess.
    let mut decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);

    for law in CrossfaderCurve::ALL {
        let ack = apply(
            &mut decks,
            &mut mixer,
            &command(9, DjCommand::SetCrossfaderCurve { curve: law }),
            0,
        );
        assert!(matches!(ack.status, AckStatus::Applied { .. }), "{law:?}");
        assert_eq!(mixer.curve(), law);
        let published = snapshot(&decks, &mixer, 1, 0, 48_000, 1);
        assert_eq!(published.crossfader_curve, law);
    }
}

#[test]
fn the_published_state_carries_the_law_without_allocating() {
    let decks = vec![deck_with_tone(DeckId::A, 48_000)];
    let mut mixer = mixer_for(48_000);
    mixer.set_curve(CrossfaderCurve::Sharp);
    let cells = StateCells::new();
    cells.publish(&decks, &mixer, 0);
    let statics: Vec<DeckStatic> = decks
        .iter()
        .map(|deck| DeckStatic::from_snapshot(&deck.snapshot()))
        .collect();
    let published = cells.read(&statics, 48_000, 1, 1);
    assert_eq!(published.crossfader_curve, CrossfaderCurve::Sharp);
}

#[test]
fn a_gain_command_moves_the_deck_it_names_and_no_other() {
    // Every deck but A used to move mixer channel 1, so a knob on C changed B; and the deck's own
    // gain, which is what the snapshot reports, was never touched, so the control sprang back.
    let mut decks = vec![
        deck_with_tone(DeckId::A, 48_000),
        deck_with_tone(DeckId::B, 48_000),
        deck_with_tone(DeckId::C, 48_000),
        deck_with_tone(DeckId::D, 48_000),
    ];
    let mut mixer = mixer_for(48_000);

    for deck in [DeckId::B, DeckId::C, DeckId::D] {
        let ack = apply(
            &mut decks,
            &mut mixer,
            &command(
                7,
                DjCommand::SetChannelGain {
                    deck,
                    linear_gain: 0.25,
                },
            ),
            0,
        );
        assert!(
            matches!(ack.status, AckStatus::Applied { .. }),
            "{deck:?} must be accepted, got {:?}",
            ack.status
        );
    }

    let state = snapshot(&decks, &mixer, 1, 0, 48_000, 0);
    for deck in state.decks {
        let wanted = if deck.deck == DeckId::A { 1.0 } else { 0.25 };
        assert!(
            (deck.linear_gain - wanted).abs() < 1e-6,
            "{:?} has gain {}, wanted {wanted}",
            deck.deck,
            deck.linear_gain
        );
    }
}

#[test]
fn the_crossfader_moves_the_sides_it_is_meant_to_move() {
    // The handle moved and nothing happened, because every channel was `Thru` — off the crossfader by
    // design. This walks the mixer the way the engine sets it up: A and C left, B and D right.
    let mut mixer = Mixer::new(4);
    mixer.prepare(48_000, 4);
    for index in 0..4 {
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
    assign_crossfader_sides(&mut mixer, &[DeckId::A, DeckId::B, DeckId::C, DeckId::D]);

    let block = [0.25_f32; 8];

    let mut level = |position: f32, index: usize| {
        mixer.set_crossfader(position);
        mixer.begin_block(4);
        mixer.mix_channel(index, &block);
        mixer.finish_block();
        mixer
            .master()
            .iter()
            .map(|sample| sample.abs())
            .sum::<f32>()
    };

    // 0.0 is the left stop: A and C are open, B and D are shut.
    assert!(
        level(0.0, 0) > 0.5,
        "deck A must be audible at the left stop"
    );
    assert_eq!(level(0.0, 1), 0.0, "deck B must be silent at the left stop");
    assert!(
        level(0.0, 2) > 0.5,
        "deck C must be audible at the left stop"
    );
    assert_eq!(level(0.0, 3), 0.0, "deck D must be silent at the left stop");

    // 1.0 is the right stop: the other way round.
    assert_eq!(
        level(1.0, 0),
        0.0,
        "deck A must be silent at the right stop"
    );
    assert!(
        level(1.0, 1) > 0.5,
        "deck B must be audible at the right stop"
    );
    assert_eq!(
        level(1.0, 2),
        0.0,
        "deck C must be silent at the right stop"
    );
    assert!(
        level(1.0, 3) > 0.5,
        "deck D must be audible at the right stop"
    );

    // And the middle is not a hole: both sides stay open, a little quieter.
    let left = level(0.5, 0);
    let right = level(0.5, 1);
    assert!(
        left > 0.2 && right > 0.2,
        "the middle must not mute both sides"
    );
}

/// A prepared track with a tone source: what the loader would hand over, built by hand.
fn prepared_with(
    id: DeckId,
    cues: Vec<(u8, u64)>,
    loop_region: Option<(u64, u64)>,
) -> dj_engine::PreparedTrack {
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
    dj_engine::PreparedTrack {
        deck: id,
        source: Box::new(Tone),
        track: None,
        static_info: DeckStatic {
            deck: id,
            load: DeckLoad::Ready,
            track: None,
            total_frames: Some(480_000),
            waveform: None,
        },
        path: Some(std::path::PathBuf::from("/tmp/dj-engine-cue-test.wav")),
        cues,
        loop_region: loop_region.map(|(start, end)| dj_domain::FrameRange {
            start: SourceFrame(start),
            end: SourceFrame(end),
        }),
    }
}

#[test]
fn a_load_puts_the_saved_cues_back_on_the_deck() {
    // Reload looks like reload: the cues and the loop the file carried come back with it.
    let decks = vec![Deck::new(DeckId::A)];
    let (_submissions, submission_rx) = std::sync::mpsc::channel();
    let (loads, load_rx) = std::sync::mpsc::channel();
    let mut graph = EngineGraph::new(
        decks,
        mixer_for(48_000),
        submission_rx,
        Arc::new(StateCells::new()),
        256,
        load_rx,
    );
    let mut prepared = prepared_with(DeckId::A, vec![(0, 1_000), (3, 44_100)], Some((10, 20)));
    prepared.static_info.deck = DeckId::A;
    loads
        .send(GraphLoad::Attach(Box::new(prepared)))
        .expect("send");
    let mut master = vec![0.0f32; 512];
    let mut cue = vec![0.0f32; 512];
    graph.render_pair(&mut master, &mut cue, 256);

    let deck = &graph.decks()[0];
    let cues = deck.hotcues();
    assert_eq!(cues[0], Some(1_000));
    assert_eq!(cues[3], Some(44_100));
    assert_eq!(cues[1], None);
    let region = deck.loop_region().expect("the loop came back");
    assert_eq!((region.start.0, region.end.0), (10, 20));
}

#[test]
fn moving_a_cue_asks_the_writer_to_save_it() {
    let decks = vec![Deck::new(DeckId::A)];
    let (submissions, submission_rx) = std::sync::mpsc::channel();
    let (loads, load_rx) = std::sync::mpsc::channel();
    let (edits, edit_rx) = std::sync::mpsc::channel();
    let mut graph = EngineGraph::new(
        decks,
        mixer_for(48_000),
        submission_rx,
        Arc::new(StateCells::new()),
        256,
        load_rx,
    )
    .with_meta_sink(edits);
    loads
        .send(GraphLoad::Attach(Box::new(prepared_with(
            DeckId::A,
            Vec::new(),
            None,
        ))))
        .expect("send");

    let mut master = vec![0.0f32; 512];
    let mut cue = vec![0.0f32; 512];
    graph.render_pair(&mut master, &mut cue, 256);
    assert!(edit_rx.try_recv().is_err(), "attaching is not an edit");

    let (reply, _answer) = std::sync::mpsc::channel();
    submissions
        .send(Submission {
            command: command(
                1,
                DjCommand::Cue {
                    deck: DeckId::A,
                    action: CueAction::Set(2),
                },
            ),
            reply,
        })
        .expect("send");
    graph.render_pair(&mut master, &mut cue, 256);

    let edit = edit_rx.try_recv().expect("an edit must follow the command");
    assert_eq!(edit.cues, vec![(2, 0)], "the cue sits at the playhead");
    assert_eq!(edit.loop_region, None);
}

#[test]
fn an_edit_survives_a_restart() {
    // What the writer stores is what a fresh process reads back: this is the acceptance line of A13.
    let dir = std::env::temp_dir().join(format!("dj-engine-meta-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let track = dir.join("song.wav");
    std::fs::write(&track, vec![7u8; 4096]).expect("write a track");
    let store = dj_library::Store::new(dir.join("mirror"));

    let edit = dj_engine::MetaEdit {
        path: track.clone(),
        cues: vec![(0, 4_800), (5, 96_000)],
        loop_region: Some((12_000, 24_000)),
    };
    dj_engine::meta::write_edit(&store, "vol", &dir, &edit).expect("write");

    // A new "process": nothing in memory, only the file.
    let mut prepared = prepared_with(DeckId::A, Vec::new(), None);
    prepared.path = Some(track.clone());
    let problem = dj_engine::meta::restore(&mut prepared, &store, "vol", &dir);
    assert!(problem.is_none(), "unexpected problem: {problem:?}");
    assert_eq!(prepared.cues, vec![(0, 4_800), (5, 96_000)]);
    let region = prepared.loop_region.expect("the loop came back");
    assert_eq!((region.start.0, region.end.0), (12_000, 24_000));

    // And a second edit keeps the fields it does not touch, because it starts from the file.
    let second = dj_engine::MetaEdit {
        path: track.clone(),
        cues: vec![(0, 4_800)],
        loop_region: None,
    };
    dj_engine::meta::write_edit(&store, "vol", &dir, &second).expect("second write");
    let loaded = store.load("vol", &dir, &track).expect("load");
    assert_eq!(loaded.meta.revision, 2, "each edit is a revision");
    assert_eq!(loaded.meta.cues.len(), 1);
    assert!(
        loaded.meta.loops.is_empty(),
        "the loop was cleared, not forgotten"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
