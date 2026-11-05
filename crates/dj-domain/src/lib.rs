//! Domain contracts: the command protocol, acknowledgements, state snapshots and the
//! audio-frame timebase that every other crate speaks.
//!
//! Status: **A01**. These types are the wire format of the engine bridge (ADR-003), so
//! they are serialisable and versioned from the start: a field added here is a protocol
//! change, not a refactor. This is the one crate where `missing_docs` is enforced —
//! naming the semantics of a field is how the protocol stays readable.
//!
//! Ownership: audio-core (`docs/architecture/crate-map.md`).
//!
//! - Requirements: DJP-ARCH-001, DJP-ARCH-002, DJP-ARCH-004, DJP-DATA-002
//! - Commands carry `command_id`, an optional `effective_sample` and a target; the
//!   engine answers with an ack and changes state by no other path.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// Version tag of the command protocol. Bump it on any incompatible change.
pub const COMMAND_PROTOCOL: &str = "command-v1";
/// Version tag of the state snapshot protocol. Bump it on any incompatible change.
pub const STATE_PROTOCOL: &str = "state-v1";

/// A deck as the engine sees it.
///
/// C and D exist in the protocol from the start so that going from two decks to four is
/// not a protocol break (DJP-DECK-001, DJP-DECK-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DeckId {
    /// Deck A.
    A,
    /// Deck B.
    B,
    /// Deck C (R1).
    C,
    /// Deck D (R1).
    D,
}

impl DeckId {
    /// Number of decks the protocol knows about.
    pub const COUNT: usize = 4;

    /// Stable index of the deck, used for fixed-size tables.
    pub fn index(self) -> usize {
        match self {
            DeckId::A => 0,
            DeckId::B => 1,
            DeckId::C => 2,
            DeckId::D => 3,
        }
    }
}

/// Total number of coalesced slots: one per continuous control per deck.
pub const COALESCE_SLOTS: usize = CoalesceKey::COUNT * DeckId::COUNT;

/// Position in the *source* material, in frames.
///
/// DJP-DATA-002: source frames and the device audio clock are different timebases and
/// are never mixed implicitly; any conversion names its `sample_rate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceFrame(pub u64);

/// Half-open range of source frames: `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameRange {
    /// First frame of the range.
    pub start: SourceFrame,
    /// One past the last frame of the range.
    pub end: SourceFrame,
}

impl FrameRange {
    /// Build a range, rejecting empty or inverted ones.
    pub fn new(start: u64, end: u64) -> Option<Self> {
        if end <= start {
            return None;
        }
        Some(Self {
            start: SourceFrame(start),
            end: SourceFrame(end),
        })
    }

    /// Number of frames in the range.
    pub fn len(&self) -> u64 {
        self.end.0.saturating_sub(self.start.0)
    }

    /// True when the range covers no frames.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True when `frame` lies inside the range.
    pub fn contains(&self, frame: SourceFrame) -> bool {
        frame.0 >= self.start.0 && frame.0 < self.end.0
    }
}

/// What a cue command asks for (DJP-DECK-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CueAction {
    /// Store the current position in hotcue slot `0..8`.
    Set(u8),
    /// Remove hotcue slot `0..8`.
    Delete(u8),
    /// Jump playback to hotcue slot `0..8`.
    Jump(u8),
}

/// Every state change of the live engine goes through one of these (DJP-ARCH-001).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DjCommand {
    /// Transport: `enabled = false` is Stop, which is a critical command.
    Play {
        /// Target deck.
        deck: DeckId,
        /// True to play, false to stop.
        enabled: bool,
    },
    /// Prepare a file into a deck (ADR-0008).
    ///
    /// Acknowledged as `Accepted`, not `Applied`: the deck is playable only when the state reports
    /// `Ready`, because decoding and cache building happen off the audio path.
    Load {
        /// Target deck.
        deck: DeckId,
        /// Absolute path as the client spelled it.
        path: String,
    },
    /// Return a deck to `Empty` and stop it.
    Eject {
        /// Target deck.
        deck: DeckId,
    },
    /// Hotcue operation.
    Cue {
        /// Target deck.
        deck: DeckId,
        /// What to do with the slot.
        action: CueAction,
    },
    /// Seek the source position.
    SeekFrames {
        /// Target deck.
        deck: DeckId,
        /// Destination in source frames.
        frame: SourceFrame,
    },
    /// Tempo change, 1.0 is the source tempo.
    SetTempoRatio {
        /// Target deck.
        deck: DeckId,
        /// Playback rate multiplier.
        ratio: f64,
    },
    /// Channel gain in linear units (1.0 = unity).
    SetChannelGain {
        /// Target deck / channel.
        deck: DeckId,
        /// Linear gain.
        linear_gain: f32,
    },
    /// Crossfader position, 0.0 = A, 1.0 = B.
    SetCrossfader {
        /// Linear position.
        position: f32,
    },
    /// Activate, move or clear a loop.
    SetLoop {
        /// Target deck.
        deck: DeckId,
        /// Loop region, or `None` to clear it.
        region: Option<FrameRange>,
    },
}

impl DjCommand {
    /// Commands that must never be dropped or coalesced.
    ///
    /// Losing Stop, Load or Seek silently is a defect (DJP-ARCH-002/003); continuous
    /// controls are the only ones allowed to collapse.
    pub fn is_critical(&self) -> bool {
        match self {
            DjCommand::Play { .. }
            | DjCommand::Cue { .. }
            | DjCommand::SeekFrames { .. }
            | DjCommand::SetLoop { .. }
            | DjCommand::Load { .. }
            | DjCommand::Eject { .. } => true,
            DjCommand::SetTempoRatio { .. }
            | DjCommand::SetChannelGain { .. }
            | DjCommand::SetCrossfader { .. } => false,
        }
    }

    /// Fixed queue slot for this continuous control, or `None` for critical commands.
    ///
    /// The slot includes the deck: a gain change on deck B must not overwrite the pending
    /// gain of deck A. Critical commands never share a slot (DJP-ARCH-003).
    pub fn coalesce_slot(&self) -> Option<usize> {
        let key = self.coalesce_key()?;
        let deck = match self {
            DjCommand::SetTempoRatio { deck, .. } | DjCommand::SetChannelGain { deck, .. } => {
                deck.index()
            }
            _ => 0,
        };
        Some(key.index() * DeckId::COUNT + deck)
    }

    /// Slot a continuous control collapses into, or `None` for critical commands.
    ///
    /// A newer value supersedes an older one, so a fader flood cannot push critical
    /// commands out of the queue.
    pub fn coalesce_key(&self) -> Option<CoalesceKey> {
        match self {
            DjCommand::SetTempoRatio { .. } => Some(CoalesceKey::Tempo),
            DjCommand::SetChannelGain { .. } => Some(CoalesceKey::ChannelGain),
            DjCommand::SetCrossfader { .. } => Some(CoalesceKey::Crossfader),
            _ => None,
        }
    }
}

/// Key of a coalesced control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoalesceKey {
    /// Tempo ratio of a deck.
    Tempo,
    /// Channel gain of a deck.
    ChannelGain,
    /// Crossfader position.
    Crossfader,
}

impl CoalesceKey {
    /// Number of distinct coalesced controls.
    pub const COUNT: usize = 3;

    /// Stable index of this key, used by the fixed-size queue.
    pub fn index(self) -> usize {
        match self {
            CoalesceKey::Tempo => 0,
            CoalesceKey::ChannelGain => 1,
            CoalesceKey::Crossfader => 2,
        }
    }
}

/// Identifier of a command, unique within an engine epoch.
pub type CommandId = u64;

/// A command bound to the audio-clock sample at which it must take effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledCommand {
    /// Identifier echoed back in the ack.
    pub id: CommandId,
    /// Device-clock sample (not a source frame) at which it becomes effective.
    pub effective_sample: u64,
    /// The command itself.
    pub command: DjCommand,
}

/// Who issued a command, so a stuck controller is distinguishable from a stuck UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOrigin {
    /// The user interface process.
    Ui,
    /// A MIDI/HID controller adapter.
    Controller,
    /// The engine itself (scheduler, session restore).
    Internal,
}

/// Outcome of a command, reported back to the issuer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AckStatus {
    /// Queued; it will be applied at or after its `effective_sample`.
    Accepted,
    /// Rejected before touching engine state; nothing changed.
    Rejected(RejectReason),
    /// Applied exactly at this audio-clock sample.
    Applied {
        /// Device-clock sample of application.
        at_sample: u64,
    },
}

/// Why a command was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// No room left in the queue for this class of command.
    QueueFull,
    /// The engine does not implement this command yet.
    NotSupported,
    /// A parameter was outside its accepted range.
    InvalidValue,
    /// The command was addressed to an older engine epoch.
    StaleEpoch,
}

/// Acknowledgement of a single command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    /// Identifier of the command being acknowledged.
    pub command_id: CommandId,
    /// What happened to it.
    pub status: AckStatus,
}

impl Ack {
    /// The command was queued.
    pub fn accepted(command_id: CommandId) -> Self {
        Self {
            command_id,
            status: AckStatus::Accepted,
        }
    }

    /// The command was applied at `at_sample`.
    pub fn applied(command_id: CommandId, at_sample: u64) -> Self {
        Self {
            command_id,
            status: AckStatus::Applied { at_sample },
        }
    }

    /// The command was rejected for `reason`.
    pub fn rejected(command_id: CommandId, reason: RejectReason) -> Self {
        Self {
            command_id,
            status: AckStatus::Rejected(reason),
        }
    }
}

/// Hotcue slots a deck exposes (DJP-DECK-003).
pub const HOTCUE_SLOTS: usize = 8;

/// How far a deck has got with preparing its material.
///
/// `Ready` is what ADR-0008 calls the moment a deck may be played: index built and window filled
/// for lossless, PCM written for lossy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeckLoad {
    /// Nothing loaded.
    Empty,
    /// Decoding, indexing or building the PCM cache.
    Loading {
        /// 0.0 … 1.0, as reported by the loader.
        progress: f32,
    },
    /// Playable.
    Ready,
    /// The file could not be prepared; the message is shown to the user as-is.
    Failed {
        /// Short, already user-facing reason.
        message: String,
    },
}

/// Track metadata a display needs. Unknown fields stay `None` rather than a guess.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackMeta {
    /// Track title.
    pub title: String,
    /// Artist, empty when unknown.
    pub artist: String,
    /// Duration in source frames, when the container states it.
    pub duration_frames: Option<u64>,
    /// Beats per minute, once analysis exists (DJP-MEDIA-006).
    pub bpm: Option<f64>,
    /// Musical key, once analysis exists.
    pub key: Option<String>,
}

/// Peak levels for meters, linear 0.0 … 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    /// Left channel peak.
    pub peak_left: f32,
    /// Right channel peak.
    pub peak_right: f32,
}

impl Levels {
    /// Silence.
    pub const SILENT: Levels = Levels {
        peak_left: 0.0,
        peak_right: 0.0,
    };
}

/// State of one deck in a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeckSnapshot {
    /// Which deck this describes.
    pub deck: DeckId,
    /// Preparation state: whether the deck can be played at all (ADR-0008 `Ready`).
    pub load: DeckLoad,
    /// Transport state.
    pub playing: bool,
    /// Source position in frames (not seconds, not the device clock).
    pub source_frame: SourceFrame,
    /// Frames left to the end of the track, when the duration is known.
    pub remaining_frames: Option<u64>,
    /// Position as a fraction of the track, when the duration is known; drives the position bar.
    pub position_ratio: Option<f32>,
    /// Track metadata for the display; `None` when nothing is loaded.
    pub track: Option<TrackMeta>,
    /// Active loop region, if any.
    pub loop_region: Option<FrameRange>,
    /// Stored hotcues; slot order is stable across snapshots.
    pub hotcues: Vec<Option<SourceFrame>>,
    /// Current output peaks.
    pub levels: Levels,
    /// Waveform envelope, once the deck has one.
    ///
    /// Absent from the wire when there is none: a client from an older build keeps working, and a
    /// deck without a picture is not a protocol error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waveform: Option<Waveform>,
    /// Current linear gain.
    pub linear_gain: f32,
    /// Current tempo ratio.
    pub tempo_ratio: f64,
}

/// Where a deck's waveform envelope lives (DJP-MEDIA-003).
///
/// The envelope itself is a file: a five-minute track is a few hundred kilobytes of extremes, which
/// does not belong in a snapshot that is published twenty times a second. The UI reads the file once
/// and draws it; the timebase is source frames, so no scaling is needed to place the playhead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Waveform {
    /// Cached envelope file, as an absolute path.
    pub path: String,
    /// Frames each column covers: the timebase.
    pub frames_per_bucket: u32,
    /// Number of columns in the file.
    pub buckets: u32,
    /// Source sample rate, so a client can turn columns into seconds.
    pub sample_rate: u32,
}

/// Read-only state published to the UI and controllers (DJP-ARCH-004).
///
/// `revision` grows monotonically and `epoch` changes when the engine restarts, so a
/// stale snapshot can never be read as confirmation that a command took effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Protocol tag, so a client can refuse an incompatible snapshot.
    pub protocol: String,
    /// Monotonic revision of this snapshot.
    pub revision: u64,
    /// Engine epoch; changes on restart.
    pub epoch: u64,
    /// Device-clock position at the moment of publication.
    pub audio_clock_samples: u64,
    /// Device sample rate the clock is counted at.
    pub sample_rate: u32,
    /// Crossfader position at the moment of publication.
    pub crossfader: f32,
    /// One entry per deck.
    pub decks: Vec<DeckSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_classified_for_the_queue() {
        assert!(
            DjCommand::Play {
                deck: DeckId::A,
                enabled: false
            }
            .is_critical()
        );
        assert!(
            DjCommand::SeekFrames {
                deck: DeckId::B,
                frame: SourceFrame(480)
            }
            .is_critical()
        );
        assert!(!DjCommand::SetCrossfader { position: 0.5 }.is_critical());
        assert_eq!(
            DjCommand::SetCrossfader { position: 0.5 }.coalesce_key(),
            Some(CoalesceKey::Crossfader)
        );
        assert_eq!(
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true
            }
            .coalesce_key(),
            None
        );
    }

    #[test]
    fn coalesced_slots_are_per_deck_and_never_collide() {
        let a = DjCommand::SetChannelGain {
            deck: DeckId::A,
            linear_gain: 0.5,
        };
        let b = DjCommand::SetChannelGain {
            deck: DeckId::B,
            linear_gain: 0.25,
        };
        assert_ne!(a.coalesce_slot(), b.coalesce_slot());
        assert!(a.coalesce_slot().unwrap_or(usize::MAX) < COALESCE_SLOTS);
        assert!(b.coalesce_slot().unwrap_or(usize::MAX) < COALESCE_SLOTS);
        assert_eq!(
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true
            }
            .coalesce_slot(),
            None
        );
    }

    #[test]
    fn a_command_survives_a_json_round_trip() {
        let command = DjCommand::SetLoop {
            deck: DeckId::B,
            region: FrameRange::new(1_000, 48_000),
        };
        let json = serde_json::to_string(&command).unwrap_or_else(|err| panic!("serialise: {err}"));
        let decoded: DjCommand =
            serde_json::from_str(&json).unwrap_or_else(|err| panic!("deserialise: {err}"));
        assert_eq!(decoded, command);
    }

    #[test]
    fn a_scheduled_command_keeps_its_sample_and_id() {
        let scheduled = ScheduledCommand {
            id: 7,
            effective_sample: 5_333,
            command: DjCommand::Play {
                deck: DeckId::A,
                enabled: true,
            },
        };
        let json = serde_json::to_string(&scheduled).unwrap_or_else(|err| panic!("{err}"));
        let decoded: ScheduledCommand =
            serde_json::from_str(&json).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(decoded, scheduled);
    }

    #[test]
    fn an_empty_or_inverted_range_is_rejected() {
        assert!(FrameRange::new(10, 10).is_none());
        assert!(FrameRange::new(10, 9).is_none());
        let range = FrameRange::new(10, 20).unwrap_or_else(|| panic!("valid range"));
        assert_eq!(range.len(), 10);
        assert!(range.contains(SourceFrame(19)));
        assert!(!range.contains(SourceFrame(20)));
    }
}
