//! The wire between the engine process and everything that talks to it (ADR-0003).
//!
//! The engine owns the device; the UI, the controller service and the CLI are clients. They speak
//! `command-v1` in and `state-v1` out — the contracts from [`dj_domain`], not a private dialect —
//! over a length-prefixed JSON frame on a Unix socket.
//!
//! Framing exists for one reason: a stream has no message boundaries. A frame is a 4-byte
//! little-endian length followed by that many bytes of JSON, and the length is checked before
//! anything is allocated, so a broken or hostile peer cannot ask for a gigabyte.
//!
//! Windows and macOS get the same protocol over a named pipe or a socket path later; the frame
//! format and the message types are already platform-free. Owner: architect / UI (A06).

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use dj_domain::{Ack, COMMAND_PROTOCOL, ScheduledCommand, Snapshot};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Version of the transport handshake, separate from the payload protocols.
pub const BRIDGE_PROTOCOL: &str = "bridge-v1";

/// Largest frame either side will accept. A snapshot with four decks is a few kilobytes; a
/// megabyte is generous and still refuses to allocate on a lie.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// What a client sends to the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// First message: announces the protocol the client speaks.
    Hello {
        /// `command-v1` expected.
        protocol: String,
    },
    /// Ask for a snapshot on every publication, not only on change.
    Subscribe,
    /// A command bound to an audio-clock sample.
    Command(ScheduledCommand),
    /// Liveness check; answered with [`ServerMessage::Pong`].
    Ping,
}

/// What the engine sends back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Handshake accepted; carries the engine epoch so a client can notice a restart.
    Welcome {
        /// `bridge-v1`.
        protocol: String,
        /// `state-v1`, so a client can refuse incompatible snapshots.
        state_protocol: String,
        /// Changes whenever the engine restarts.
        engine_epoch: u64,
    },
    /// Outcome of a single command.
    Ack(Ack),
    /// Published state.
    Snapshot(Box<Snapshot>),
    /// Answer to [`ClientMessage::Ping`].
    Pong,
    /// The engine refused the request; the connection is closed by the sender.
    Error {
        /// Human-readable reason.
        message: String,
    },
}

/// Why a frame could not be read or written.
#[derive(Debug)]
pub enum FrameError {
    /// The peer closed the connection.
    Closed,
    /// The announced length exceeds [`MAX_FRAME_BYTES`].
    TooLarge {
        /// Length the peer announced.
        announced: usize,
    },
    /// The payload is not the message the protocol expects.
    Encoding(String),
    /// The socket failed.
    Io(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Closed => write!(f, "connection closed"),
            FrameError::TooLarge { announced } => write!(
                f,
                "frame of {announced} bytes exceeds the {MAX_FRAME_BYTES}-byte limit"
            ),
            FrameError::Encoding(detail) => write!(f, "cannot decode frame: {detail}"),
            FrameError::Io(detail) => write!(f, "socket error: {detail}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Write one framed message.
pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, message: &T) -> Result<(), FrameError> {
    let payload =
        serde_json::to_vec(message).map_err(|err| FrameError::Encoding(err.to_string()))?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            announced: payload.len(),
        });
    }
    let length = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge {
        announced: payload.len(),
    })?;
    writer
        .write_all(&length.to_le_bytes())
        .and_then(|()| writer.write_all(&payload))
        .and_then(|()| writer.flush())
        .map_err(|err| FrameError::Io(err.to_string()))
}

/// Read one framed message.
pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, FrameError> {
    let mut length_bytes = [0u8; 4];
    match reader.read_exact(&mut length_bytes) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::Closed);
        }
        Err(err) => return Err(FrameError::Io(err.to_string())),
    }
    let announced = u32::from_le_bytes(length_bytes) as usize;
    if announced > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { announced });
    }
    let mut payload = vec![0u8; announced];
    reader
        .read_exact(&mut payload)
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::UnexpectedEof => {
                FrameError::Encoding("frame ended before its announced length".to_string())
            }
            _ => FrameError::Io(err.to_string()),
        })?;
    serde_json::from_slice(&payload).map_err(|err| FrameError::Encoding(err.to_string()))
}

/// One accepted connection, with framing already handled.
#[derive(Debug)]
pub struct Connection {
    stream: std::os::unix::net::UnixStream,
    reader: BufReader<std::os::unix::net::UnixStream>,
}

impl Connection {
    /// Wrap an accepted socket.
    pub fn new(stream: std::os::unix::net::UnixStream) -> std::io::Result<Self> {
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { stream, reader })
    }

    /// Send a message.
    pub fn send(&mut self, message: &ServerMessage) -> Result<(), FrameError> {
        write_frame(&mut self.stream, message)
    }

    /// Send a message to the engine.
    pub fn send_client(&mut self, message: &ClientMessage) -> Result<(), FrameError> {
        write_frame(&mut self.stream, message)
    }

    /// Wait for the next message.
    pub fn recv(&mut self) -> Result<ClientMessage, FrameError> {
        read_frame(&mut self.reader)
    }

    /// Split into halves that can live on separate threads.
    ///
    /// A client needs two independent loops: one waiting for commands, one forwarding snapshots.
    /// One thread cannot do both — a blocking read parks it, and outgoing messages wait for the
    /// client to speak again. That was a real defect: the client got its greeting, then silence.
    pub fn split(self) -> (ConnectionReader, ConnectionWriter) {
        (
            ConnectionReader {
                reader: self.reader,
            },
            ConnectionWriter {
                stream: self.stream,
            },
        )
    }
}

/// The reading half of a split connection.
pub struct ConnectionReader {
    reader: BufReader<std::os::unix::net::UnixStream>,
}

impl ConnectionReader {
    /// Read one client message.
    pub fn recv_client(&mut self) -> Result<ClientMessage, FrameError> {
        read_frame(&mut self.reader)
    }

    /// Give up waiting after `timeout`.
    pub fn set_read_timeout(&self, timeout: std::time::Duration) -> Result<(), FrameError> {
        self.reader
            .get_ref()
            .set_read_timeout(Some(timeout))
            .map_err(|err| FrameError::Io(err.to_string()))
    }
}

/// The writing half of a split connection.
pub struct ConnectionWriter {
    stream: std::os::unix::net::UnixStream,
}

impl ConnectionWriter {
    /// Write one message.
    pub fn send(&mut self, message: &ServerMessage) -> Result<(), FrameError> {
        write_frame(&mut self.stream, message)
    }
}

/// A listening socket the engine owns.
#[derive(Debug)]
pub struct BridgeServer {
    listener: std::os::unix::net::UnixListener,
    path: PathBuf,
}

impl BridgeServer {
    /// Bind the socket, replacing a stale one from a crashed engine.
    ///
    /// The stale file is only removed when nothing answers on it: a second engine must not steal
    /// the path from a running one.
    pub fn bind(path: &Path) -> Result<Self, FrameError> {
        if path.exists() {
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    return Err(FrameError::Io(format!(
                        "{} is already served by a running engine",
                        path.display()
                    )));
                }
                Err(_) => {
                    std::fs::remove_file(path).map_err(|err| {
                        FrameError::Io(format!("cannot remove stale {}: {err}", path.display()))
                    })?;
                }
            }
        }
        let listener = std::os::unix::net::UnixListener::bind(path)
            .map_err(|err| FrameError::Io(format!("cannot bind {}: {err}", path.display())))?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// Path the socket is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accept the next client.
    pub fn accept(&self) -> Result<Connection, FrameError> {
        let (stream, _) = self
            .listener
            .accept()
            .map_err(|err| FrameError::Io(err.to_string()))?;
        Connection::new(stream).map_err(|err| FrameError::Io(err.to_string()))
    }

    /// Handshake a freshly accepted connection.
    ///
    /// Returns the epoch to greet the client with, or an error message to send before hanging up.
    pub fn handshake(connection: &mut Connection, engine_epoch: u64) -> Result<(), String> {
        match connection.recv() {
            Ok(ClientMessage::Hello { protocol }) if protocol == COMMAND_PROTOCOL => connection
                .send(&ServerMessage::Welcome {
                    protocol: BRIDGE_PROTOCOL.to_string(),
                    state_protocol: dj_domain::STATE_PROTOCOL.to_string(),
                    engine_epoch,
                })
                .map_err(|err| err.to_string()),
            Ok(ClientMessage::Hello { protocol }) => {
                let message = format!(
                    "unsupported protocol {protocol:?}; this engine speaks {COMMAND_PROTOCOL}"
                );
                let _ = connection.send(&ServerMessage::Error {
                    message: message.clone(),
                });
                Err(message)
            }
            Ok(other) => {
                let message = format!("expected Hello, got {other:?}");
                let _ = connection.send(&ServerMessage::Error {
                    message: message.clone(),
                });
                Err(message)
            }
            Err(err) => Err(err.to_string()),
        }
    }
}

impl Drop for BridgeServer {
    fn drop(&mut self) {
        // Leave no socket file behind for the next bind to trip over.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Connect to a running engine and complete the handshake.
pub fn connect(path: &Path) -> Result<(Connection, ServerMessage), FrameError> {
    connect_with_timeout(path, None)
}

/// Connect with a deadline on the handshake.
///
/// A server that accepts but never greets must not hang its client: tests and the UI both need the
/// failure to arrive as an error.
pub fn connect_with_timeout(
    path: &Path,
    timeout: Option<std::time::Duration>,
) -> Result<(Connection, ServerMessage), FrameError> {
    let stream = std::os::unix::net::UnixStream::connect(path)
        .map_err(|err| FrameError::Io(format!("cannot connect to {}: {err}", path.display())))?;
    if let Some(timeout) = timeout {
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|err| FrameError::Io(err.to_string()))?;
    }
    let mut connection = Connection::new(stream).map_err(|err| FrameError::Io(err.to_string()))?;
    connection.send_client(&ClientMessage::Hello {
        protocol: COMMAND_PROTOCOL.to_string(),
    })?;
    let welcome = connection.recv_server()?;
    Ok((connection, welcome))
}

impl Connection {
    /// Receive a message from the engine side.
    pub fn recv_server(&mut self) -> Result<ServerMessage, FrameError> {
        read_frame(&mut self.reader)
    }

    /// Give up waiting for a message after `timeout`.
    ///
    /// Without it, a missing message turns into a hung client instead of a reported failure.
    pub fn set_read_timeout(&self, timeout: std::time::Duration) -> Result<(), FrameError> {
        self.reader
            .get_ref()
            .set_read_timeout(Some(timeout))
            .map_err(|err| FrameError::Io(err.to_string()))
    }
}
