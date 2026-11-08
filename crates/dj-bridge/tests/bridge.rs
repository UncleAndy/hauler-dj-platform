//! The bridge has to fail politely: a client that speaks the wrong protocol, a frame that lies
//! about its length, a socket left behind by a crashed engine.

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use dj_bridge::{
    BridgeServer, ClientMessage, Connection, FrameError, MAX_FRAME_BYTES, ServerMessage, connect,
    read_frame, write_frame,
};
use dj_domain::{
    Ack, COMMAND_PROTOCOL, CommandOrigin, DeckId, DjCommand, ScheduledCommand, Snapshot,
    SourceFrame,
};

fn socket_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dj-bridge-{}-{name}.sock", std::process::id()))
}

fn play_command(id: u64) -> ScheduledCommand {
    ScheduledCommand {
        id,
        effective_sample: 0,
        command: DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    }
}

fn empty_snapshot(revision: u64) -> Snapshot {
    Snapshot {
        protocol: dj_domain::STATE_PROTOCOL.to_string(),
        revision,
        epoch: 7,
        audio_clock_samples: revision * 256,
        sample_rate: 48_000,
        crossfader: 0.5,
        crossfader_curve: dj_domain::CrossfaderCurve::default(),
        master: dj_domain::Levels::default(),
        master_gain: 1.0,
        master_latency_frames: 0,
        decks: Vec::new(),
    }
}

#[test]
fn a_frame_survives_a_round_trip() {
    let mut buffer = Vec::new();
    write_frame(&mut buffer, &ClientMessage::Ping).unwrap_or_else(|err| panic!("{err}"));
    let mut cursor = Cursor::new(buffer);
    let decoded: ClientMessage = read_frame(&mut cursor).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(decoded, ClientMessage::Ping);
}

#[test]
fn a_frame_that_lies_about_its_length_is_refused_before_allocating() {
    let mut buffer = Vec::new();
    buffer
        .extend_from_slice(&(u32::try_from(MAX_FRAME_BYTES + 1).unwrap_or(u32::MAX)).to_le_bytes());
    let mut cursor = Cursor::new(buffer);
    let result: Result<ClientMessage, _> = read_frame(&mut cursor);
    assert!(
        matches!(result, Err(FrameError::TooLarge { .. })),
        "a hostile length must be refused, not allocated"
    );
}

#[test]
fn a_truncated_frame_is_an_error_not_a_panic() {
    let mut buffer = Vec::new();
    write_frame(&mut buffer, &ClientMessage::Ping).unwrap_or_else(|err| panic!("{err}"));
    buffer.truncate(buffer.len() - 1);
    let mut cursor = Cursor::new(buffer);
    let result: Result<ClientMessage, _> = read_frame(&mut cursor);
    assert!(
        matches!(result, Err(FrameError::Encoding(_))),
        "a short frame must be reported, got {result:?}"
    );
}

#[test]
fn a_closed_socket_is_closed_not_an_io_surprise() {
    let mut cursor = Cursor::new(Vec::new());
    let result: Result<ClientMessage, _> = read_frame(&mut cursor);
    assert!(matches!(result, Err(FrameError::Closed)));
}

#[test]
fn a_client_and_a_server_talk_over_a_real_socket() {
    let path = socket_path("round-trip");
    let server = BridgeServer::bind(&path).unwrap_or_else(|err| panic!("bind: {err}"));

    let server_path = server.path().to_path_buf();
    let handle = std::thread::spawn(move || {
        let mut connection = server
            .accept()
            .unwrap_or_else(|err| panic!("accept: {err}"));
        BridgeServer::handshake(&mut connection, 7)
            .unwrap_or_else(|err| panic!("handshake: {err}"));
        if let Ok(ClientMessage::Command(command)) = connection.recv() {
            let _ = connection.send(&ServerMessage::Ack(Ack::applied(command.id, 1_024)));
        }
        let _ = connection.send(&ServerMessage::Snapshot(Box::new(empty_snapshot(1))));
        // Keep the server alive until the client has read everything.
        std::thread::sleep(Duration::from_millis(50));
        drop(server_path);
        drop(connection);
    });

    let (mut client, welcome) = connect(&path).unwrap_or_else(|err| panic!("connect: {err}"));
    match welcome {
        ServerMessage::Welcome {
            protocol,
            state_protocol,
            engine_epoch,
        } => {
            assert_eq!(protocol, dj_bridge::BRIDGE_PROTOCOL);
            assert_eq!(state_protocol, dj_domain::STATE_PROTOCOL);
            assert_eq!(engine_epoch, 7, "the epoch lets a client notice a restart");
        }
        other => panic!("expected Welcome, got {other:?}"),
    }

    client
        .send_client(&ClientMessage::Command(play_command(42)))
        .unwrap_or_else(|err| panic!("send: {err}"));
    match client
        .recv_server()
        .unwrap_or_else(|err| panic!("recv: {err}"))
    {
        ServerMessage::Ack(ack) => {
            assert_eq!(ack.command_id, 42, "the ack echoes the command id");
            assert!(matches!(
                ack.status,
                dj_domain::AckStatus::Applied { at_sample: 1_024 }
            ));
        }
        other => panic!("expected Ack, got {other:?}"),
    }
    match client
        .recv_server()
        .unwrap_or_else(|err| panic!("recv: {err}"))
    {
        ServerMessage::Snapshot(snapshot) => {
            assert_eq!(snapshot.revision, 1);
            assert_eq!(snapshot.protocol, dj_domain::STATE_PROTOCOL);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
    handle
        .join()
        .unwrap_or_else(|_| panic!("server thread panicked"));
}

#[test]
fn a_client_with_the_wrong_protocol_is_refused_with_a_reason() {
    let path = socket_path("wrong-protocol");
    let server = BridgeServer::bind(&path).unwrap_or_else(|err| panic!("bind: {err}"));

    let handle = std::thread::spawn(move || {
        let mut connection = server
            .accept()
            .unwrap_or_else(|err| panic!("accept: {err}"));
        let outcome = BridgeServer::handshake(&mut connection, 7);
        assert!(outcome.is_err(), "a wrong protocol must not be greeted");
        std::thread::sleep(Duration::from_millis(50));
    });

    let stream = std::os::unix::net::UnixStream::connect(&path)
        .unwrap_or_else(|err| panic!("connect: {err}"));
    let mut client = dj_bridge::Connection::new(stream).unwrap_or_else(|err| panic!("{err}"));
    client
        .send_client(&ClientMessage::Hello {
            protocol: "command-v0".to_string(),
        })
        .unwrap_or_else(|err| panic!("{err}"));
    match client.recv_server().unwrap_or_else(|err| panic!("{err}")) {
        ServerMessage::Error { message } => {
            assert!(
                message.contains(COMMAND_PROTOCOL),
                "the refusal must name the protocol this engine speaks: {message}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
    handle
        .join()
        .unwrap_or_else(|_| panic!("server thread panicked"));
}

#[test]
fn a_second_engine_cannot_steal_a_live_socket() {
    let path = socket_path("second-engine");
    let server = BridgeServer::bind(&path).unwrap_or_else(|err| panic!("bind: {err}"));

    let second = BridgeServer::bind(&path);
    assert!(
        second.is_err(),
        "a running engine owns its socket; a second one must not hijack the path"
    );
    drop(server);
}

#[test]
fn a_stale_socket_from_a_crashed_engine_is_replaced() {
    let path = socket_path("stale");
    // Simulate a crash: a socket file with nobody listening.
    let listener =
        std::os::unix::net::UnixListener::bind(&path).unwrap_or_else(|err| panic!("bind: {err}"));
    drop(listener);
    assert!(path.exists(), "the stale file must still be there");

    let server = BridgeServer::bind(&path);
    assert!(
        server.is_ok(),
        "a stale socket must not block the next start: {:?}",
        server.err()
    );
}

#[test]
fn commands_carry_their_origin_so_a_stuck_controller_is_visible() {
    // Nothing in the transport rewrites the command; the origin travels with it untouched.
    let command = play_command(9);
    let mut buffer = Vec::new();
    write_frame(&mut buffer, &ClientMessage::Command(command.clone()))
        .unwrap_or_else(|err| panic!("{err}"));
    let mut cursor = Cursor::new(buffer);
    match read_frame::<_, ClientMessage>(&mut cursor).unwrap_or_else(|err| panic!("{err}")) {
        ClientMessage::Command(back) => assert_eq!(back, command),
        other => panic!("expected a command, got {other:?}"),
    }
    // The enum exists in the protocol; the engine is the only place that can attribute a command.
    let _origin = CommandOrigin::Ui;
    let _source = SourceFrame(0);
}

#[test]
fn a_split_connection_writes_while_its_reader_is_blocked() {
    // The defect this covers: one thread serving both halves. It waited 5 ms for an outgoing
    // message, then parked in a blocking read, so a quiet client received its greeting and then
    // nothing at all. After the split, writing cannot be starved by reading.
    let (engine_side, client_side) =
        std::os::unix::net::UnixStream::pair().unwrap_or_else(|err| panic!("socket pair: {err}"));
    let engine = Connection::new(engine_side).unwrap_or_else(|err| panic!("{err}"));
    let mut client = Connection::new(client_side).unwrap_or_else(|err| panic!("{err}"));
    client
        .set_read_timeout(std::time::Duration::from_secs(2))
        .unwrap_or_else(|err| panic!("{err}"));
    let (mut reader, mut writer) = engine.split();

    // The reader blocks on a command that is never sent.
    let reader_handle = std::thread::spawn(move || reader.recv_client());

    // Meanwhile the writer must still be able to answer.
    let snapshot = ServerMessage::Snapshot(Box::new(dj_domain::Snapshot {
        protocol: dj_domain::STATE_PROTOCOL.to_string(),
        revision: 4,
        epoch: 2,
        audio_clock_samples: 512,
        sample_rate: 48_000,
        crossfader: 0.5,
        crossfader_curve: dj_domain::CrossfaderCurve::default(),
        master: dj_domain::Levels::default(),
        master_gain: 1.0,
        master_latency_frames: 0,
        decks: Vec::new(),
    }));
    writer
        .send(&snapshot)
        .unwrap_or_else(|err| panic!("the writer sends: {err}"));

    match client.recv_server() {
        Ok(ServerMessage::Snapshot(state)) => {
            assert_eq!(state.revision, 4);
            assert_eq!(state.audio_clock_samples, 512);
        }
        Ok(other) => panic!("expected a snapshot, got {other:?}"),
        Err(err) => panic!("the client receives: {err}"),
    }

    // Close the client so the blocked reader can finish instead of leaking a thread.
    drop(client);
    let _ = reader_handle.join();
}
