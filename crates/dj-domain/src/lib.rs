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

/// One coalesced slot per continuous control per deck, plus one per EQ band per deck.
///
/// The EQ needs its own block: a band is a separate knob, and with one slot per channel two knobs
/// moving at once would overwrite each other, so one of them would silently stop following the hand.
pub const COALESCE_SLOTS: usize =
    (CoalesceKey::COUNT - 1) * DeckId::COUNT + EqBand::COUNT * DeckId::COUNT;

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

/// Crossfader law: how the two sides trade level as the handle moves.
///
/// The law travels in the protocol rather than living in the engine, because a client has to show
/// which one is in force and a person will notice the difference between them (DJP-MIX-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossfaderCurve {
    /// Each side falls linearly; both sit at half at the centre.
    Linear,
    /// Quarter sine: each side sits at −3 dB at the centre and the energy sum stays at unity.
    /// The DJ default.
    #[default]
    ConstantPower,
    /// S-curve: flat at both extremes, steepest in the middle.
    Smooth,
    /// Cut: full until a quarter from the edge, then a hard cut, so both channels stay open across
    /// the middle. The scratch setting.
    Sharp,
}

impl CrossfaderCurve {
    /// Every law, in the order a selector shows them.
    pub const ALL: [Self; 4] = [Self::ConstantPower, Self::Linear, Self::Smooth, Self::Sharp];

    /// Name used on the wire and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::ConstantPower => "constant_power",
            Self::Smooth => "smooth",
            Self::Sharp => "sharp",
        }
    }

    /// Parse a name; `None` for anything unknown, so a typo cannot silently mean the default.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|law| law.as_str() == name.trim().to_ascii_lowercase())
    }
}

/// A band of the channel three-band equaliser (A14, DJP-MIX-002).
///
/// The band travels in the protocol rather than in the shell: a client names the knob it moved, and
/// the engine answers with the value it actually applied, so three knobs cannot each keep their own
/// idea of the setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBand {
    /// Low shelf.
    Low,
    /// Mid peak.
    Mid,
    /// High shelf.
    High,
}

impl EqBand {
    /// Every band, in table order.
    pub const ALL: [Self; 3] = [Self::Low, Self::Mid, Self::High];

    /// How many bands a channel has.
    pub const COUNT: usize = Self::ALL.len();

    /// Stable index, used by the command queue and by the coefficient table.
    pub fn index(self) -> usize {
        match self {
            Self::Low => 0,
            Self::Mid => 1,
            Self::High => 2,
        }
    }

    /// Name used on the wire and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Mid => "mid",
            Self::High => "high",
        }
    }

    /// Parse a name; `None` for anything unknown, so a typo cannot silently move another band.
    pub fn parse(name: &str) -> Option<Self> {
        let wanted = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|band| band.as_str() == wanted)
    }
}

/// How far a band can be lifted, in decibels — the DJ limit, not the studio one.
///
/// The boost is capped low on purpose: a shelf lift raises the loudness of the whole channel, and at
/// +26 dB one knob could double the level of a bass-heavy track (measured on our own curve: +11 dB
/// RMS at +12 dB of low shelf). +6 dB is what a mixer channel offers, and it is the number the curve
/// was measured against.
pub const EQ_BOOST_DB: f32 = 6.0;

/// How far a band can be cut with the ordinary law: a deep cut, but not a kill.
pub const EQ_CUT_DB: f32 = 26.0;

/// The floor of the knob, reached only in isolator mode.
///
/// -120 dB is silence in practice — a millionth of the signal — and it is still an ordinary shelf
/// coefficient, so a full kill needs no second filter and no mode in the engine: the knob law
/// decides how far left the hand can go, the filter simply obeys the number it is given.
pub const EQ_FLOOR_DB: f32 = -120.0;

/// The three band gains of one channel, in decibels.
///
/// This is both the payload of a knob command and the published state, so the number a person moves
/// and the number the strip draws are one number.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct EqGains {
    /// Low shelf gain.
    #[serde(default)]
    pub low_db: f32,
    /// Mid peak gain.
    #[serde(default)]
    pub mid_db: f32,
    /// High shelf gain.
    #[serde(default)]
    pub high_db: f32,
}

impl EqGains {
    /// Everything at unity: the section is then skipped rather than multiplied by one.
    pub const FLAT: Self = Self {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: 0.0,
    };

    /// True when no band would change the signal.
    pub fn is_flat(&self) -> bool {
        self.low_db == 0.0 && self.mid_db == 0.0 && self.high_db == 0.0
    }

    /// One band's gain.
    pub fn band(&self, band: EqBand) -> f32 {
        match band {
            EqBand::Low => self.low_db,
            EqBand::Mid => self.mid_db,
            EqBand::High => self.high_db,
        }
    }

    /// Set one band, clamped into the accepted range.
    ///
    /// A value that is not a number is refused as unity: a NaN from a broken client must not become a
    /// filter that outputs silence and looks like a dead channel.
    pub fn set_band(&mut self, band: EqBand, gain_db: f32) {
        let value = if gain_db.is_finite() {
            gain_db.clamp(EQ_FLOOR_DB, EQ_BOOST_DB)
        } else {
            0.0
        };
        match band {
            EqBand::Low => self.low_db = value,
            EqBand::Mid => self.mid_db = value,
            EqBand::High => self.high_db = value,
        }
    }

    /// Every band inside the range and finite.
    pub fn clamped(self) -> Self {
        let mut out = self;
        for band in EqBand::ALL {
            out.set_band(band, out.band(band));
        }
        out
    }
}

/// One when the field is absent, so an older snapshot reads as unity gain rather than as silence.
fn unity() -> f32 {
    1.0
}

/// One command from a client to the engine.
///
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
    /// Switch loop mode for a deck: on repeats every saved loop, off repeats none.
    SetLoopMode {
        /// Target deck.
        deck: DeckId,
        /// Whether loop mode is on.
        enabled: bool,
    },
    /// Store the deck's active loop on a pad, so the same four bars come back with one press
    /// (DJP-DECK-003, DJP-DECK-004).
    SetPadLoop {
        /// Target deck.
        deck: DeckId,
        /// Hotcue slot `0..8`.
        slot: u8,
    },
    /// The name and the colour of a hotcue: what makes a pad recognisable in a dark booth.
    ///
    /// A cue's position is a `Cue` command; this one only writes what a hand sees, and it is critical
    /// because a lost rename is a lost piece of the DJ's own library.
    SetCueMeta {
        /// Target deck.
        deck: DeckId,
        /// Hotcue slot `0..8`.
        slot: u8,
        /// New name, or `None` to leave it alone.
        label: Option<String>,
        /// New colour as `#rrggbb`, or `None` to leave it alone.
        color: Option<String>,
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
    /// One band of a channel's three-band equaliser, in decibels (A14, DJP-MIX-002).
    ///
    /// One band per command: the shell sends exactly the band the hand touched, and the queue keeps
    /// a slot per band, so two knobs moving together do not cancel each other.
    SetChannelEq {
        /// Target deck / channel.
        deck: DeckId,
        /// Which band moved.
        band: EqBand,
        /// New gain, clamped to `EQ_FLOOR_DB`…`EQ_BOOST_DB` by the engine.
        gain_db: f32,
    },
    /// Crossfader position, 0.0 = A, 1.0 = B.
    SetCrossfader {
        /// Linear position.
        position: f32,
    },
    /// Choose the crossfader law.
    SetCrossfaderCurve {
        /// The law to use from the next block on.
        curve: CrossfaderCurve,
    },
    /// Master output gain in linear units (1.0 = unity); the knob at the top of the mixer.
    SetMasterGain {
        /// Linear gain.
        linear_gain: f32,
    },
    /// The channel fader of a channel: the level control a hand rides during a mix.
    ///
    /// A fader is not a gain. Gain (trim) is set once per track to line up levels and sits before the
    /// EQ; the fader moves continuously after it, and the two must not share one number: a knob that
    /// is re-trimmed mid-set and a fader that is thrown at the same moment would fight each other.
    /// 0.0 is silence, 1.0 is unity — a channel fader does not boost.
    SetChannelFader {
        /// Target deck / channel.
        deck: DeckId,
        /// Fader position, 0.0 … 1.0.
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
            | DjCommand::SetPadLoop { .. }
            | DjCommand::SetLoopMode { .. }
            | DjCommand::SetCueMeta { .. }
            | DjCommand::Cue { .. }
            | DjCommand::SeekFrames { .. }
            | DjCommand::SetLoop { .. }
            | DjCommand::Load { .. }
            | DjCommand::Eject { .. } => true,
            DjCommand::SetTempoRatio { .. }
            | DjCommand::SetChannelGain { .. }
            | DjCommand::SetChannelFader { .. }
            | DjCommand::SetChannelEq { .. }
            | DjCommand::SetCrossfader { .. }
            | DjCommand::SetCrossfaderCurve { .. }
            | DjCommand::SetMasterGain { .. } => false,
        }
    }

    /// Fixed queue slot for this continuous control, or `None` for critical commands.
    ///
    /// The slot includes the deck: a gain change on deck B must not overwrite the pending
    /// gain of deck A. Critical commands never share a slot (DJP-ARCH-003).
    pub fn coalesce_slot(&self) -> Option<usize> {
        let key = self.coalesce_key()?;
        let deck = match self {
            DjCommand::SetTempoRatio { deck, .. }
            | DjCommand::SetChannelGain { deck, .. }
            | DjCommand::SetChannelFader { deck, .. }
            | DjCommand::SetChannelEq { deck, .. } => deck.index(),
            _ => 0,
        };
        Some(match self {
            // The EQ block lives after the deck-wide controls, so the two schemes cannot overlap.
            DjCommand::SetChannelEq { band, .. } => {
                CoalesceKey::EQ_SLOT_BASE + deck * EqBand::COUNT + band.index()
            }
            _ => key.index() * DeckId::COUNT + deck,
        })
    }

    /// Slot a continuous control collapses into, or `None` for critical commands.
    ///
    /// A newer value supersedes an older one, so a fader flood cannot push critical
    /// commands out of the queue.
    pub fn coalesce_key(&self) -> Option<CoalesceKey> {
        match self {
            DjCommand::SetTempoRatio { .. } => Some(CoalesceKey::Tempo),
            DjCommand::SetChannelGain { .. } => Some(CoalesceKey::ChannelGain),
            DjCommand::SetChannelFader { .. } => Some(CoalesceKey::ChannelFader),
            DjCommand::SetChannelEq { .. } => Some(CoalesceKey::ChannelEq),
            DjCommand::SetCrossfader { .. } => Some(CoalesceKey::Crossfader),
            DjCommand::SetCrossfaderCurve { .. } => Some(CoalesceKey::CrossfaderCurve),
            DjCommand::SetMasterGain { .. } => Some(CoalesceKey::MasterGain),
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
    /// Crossfader law: only the last choice matters.
    CrossfaderCurve,
    /// Master gain: only the last value matters.
    MasterGain,
    /// Channel fader of a deck: the last position wins.
    ChannelFader,
    /// One band of a deck's equaliser: the last value of that band wins.
    ChannelEq,
}

impl CoalesceKey {
    /// Number of distinct coalesced controls.
    ///
    /// This has to name every variant: it sizes the queue, and it used to say three while five keys
    /// existed, so the master gain landed past the end of the array.
    pub const COUNT: usize = 7;

    /// First slot of the EQ block: the deck-wide controls come first, one block of `DeckId::COUNT`
    /// slots each.
    pub const EQ_SLOT_BASE: usize = (Self::COUNT - 1) * DeckId::COUNT;

    /// Stable index of this key, used by the fixed-size queue.
    pub fn index(self) -> usize {
        match self {
            CoalesceKey::Tempo => 0,
            CoalesceKey::ChannelGain => 1,
            CoalesceKey::Crossfader => 2,
            CoalesceKey::CrossfaderCurve => 3,
            CoalesceKey::MasterGain => 4,
            CoalesceKey::ChannelFader => 5,
            // The EQ keeps the last index on purpose: its slots are a block of three per deck, and the
            // block sits after every deck-wide control.
            CoalesceKey::ChannelEq => 6,
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
    /// The deck is playing: loading would cut the track the room is dancing to.
    Busy,
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
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Levels {
    /// Left channel peak.
    pub peak_left: f32,
    /// Right channel peak.
    pub peak_right: f32,
    /// Left bar value: the peak held and falling back, which is what a meter shows.
    ///
    /// Defaulted, so a snapshot from a build without ballistics still reads: a client that knows
    /// nothing about the fall back is not a protocol error.
    #[serde(default)]
    pub held_left: f32,
    /// Right bar value.
    #[serde(default)]
    pub held_right: f32,
}

impl Levels {
    /// Silence.
    pub const SILENT: Levels = Levels {
        peak_left: 0.0,
        peak_right: 0.0,
        held_left: 0.0,
        held_right: 0.0,
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
    /// Loops stored on pads; a pad holds a point or a loop, never both.
    #[serde(default)]
    pub pad_loops: Vec<Option<FrameRange>>,
    /// Every loop loop mode is watching: the saved ones plus the one the Loop button made. A track
    /// holds many loops, and loop mode repeats all of them.
    #[serde(default)]
    pub active_loops: Vec<FrameRange>,
    /// Names of the hotcues, slot for slot; `None` where a cue has no name or no cue exists.
    ///
    /// Defaulted, so a state document written before pads had names still reads: a client that knows
    /// nothing about labels is not a protocol error.
    #[serde(default)]
    pub cue_labels: Vec<Option<String>>,
    /// Colours of the hotcues as `#rrggbb`, slot for slot.
    #[serde(default)]
    pub cue_colors: Vec<Option<String>>,
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
    /// Correction the loader measured for this file, linear. Not a knob: shown beside the clock.
    #[serde(default = "unity")]
    pub auto_gain: f32,
    /// The same correction in decibels, as measured. Absent when the level was never measured, so a
    /// shell can tell "no correction" from "nothing was measured".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_gain_db: Option<f32>,
    /// Current tempo ratio.
    pub tempo_ratio: f64,
    /// Channel fader in force, linear: 1.0 is unity, 0.0 is silence.
    #[serde(default = "unity")]
    pub fader: f32,
    /// The three band gains in force on this channel (A14), already on the knob's grid.
    ///
    /// Defaulted so a snapshot from a build without the EQ still parses here.
    #[serde(default)]
    pub eq: EqGains,
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
    /// Crossfader law in force at the moment of publication.
    #[serde(default)]
    pub crossfader_curve: CrossfaderCurve,
    /// Master output peak since the previous snapshot, after gain and the limiter.
    #[serde(default)]
    pub master: Levels,
    /// Master gain in force, linear.
    #[serde(default = "unity")]
    pub master_gain: f32,
    /// Frames the master path delays the sound by: the limiter's lookahead.
    ///
    /// Defaulted, so a snapshot from a build without a lookahead still reads as zero.
    #[serde(default)]
    pub master_latency_frames: u32,
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
    fn every_coalesced_control_has_its_own_slot() {
        let keys = [
            CoalesceKey::Tempo,
            CoalesceKey::ChannelGain,
            CoalesceKey::Crossfader,
            CoalesceKey::CrossfaderCurve,
            CoalesceKey::MasterGain,
            CoalesceKey::ChannelFader,
            CoalesceKey::ChannelEq,
        ];
        for key in keys {
            // The queue is sized from COUNT, and COUNT once said three while five keys existed: the
            // master gain then landed past the end of the array, inside the audio callback.
            assert!(
                key.index() < CoalesceKey::COUNT,
                "{key:?} is outside the table"
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        for deck in [DeckId::A, DeckId::B, DeckId::C, DeckId::D] {
            let fader = DjCommand::SetChannelFader {
                deck,
                position: 0.5,
            };
            let slot = fader.coalesce_slot().unwrap_or(usize::MAX);
            assert!(
                slot < COALESCE_SLOTS,
                "{deck:?} fader lands outside the queue"
            );
            assert!(seen.insert(slot), "{deck:?} fader shares a slot");
            for band in EqBand::ALL {
                let command = DjCommand::SetChannelEq {
                    deck,
                    band,
                    gain_db: 3.0,
                };
                let slot = command.coalesce_slot().unwrap_or(usize::MAX);
                assert!(
                    slot < COALESCE_SLOTS,
                    "{deck:?} {band:?} lands outside the queue"
                );
                assert!(
                    seen.insert(slot),
                    "{deck:?} {band:?} shares a slot with another control"
                );
            }
        }
    }

    #[test]
    fn an_eq_gain_is_clamped_and_a_nan_reads_as_unity() {
        let mut gains = EqGains::FLAT;
        gains.set_band(EqBand::Low, 99.0);
        assert_eq!(gains.low_db, EQ_BOOST_DB);
        gains.set_band(EqBand::Mid, f32::NAN);
        assert_eq!(gains.mid_db, 0.0);
        assert_eq!(gains.band(EqBand::High), 0.0);
        assert!(!gains.is_flat(), "the boosted low band was forgotten");
        assert!(
            !gains.clamped().is_flat(),
            "clamping dropped the boosted band"
        );

        // A kill is allowed to reach the floor: the knob, not the engine, decides whether the hand can
        // get there (isolator mode), and the filter simply obeys the number it is given.
        let mut kill = EqGains::FLAT;
        kill.set_band(EqBand::Low, -999.0);
        assert_eq!(kill.low_db, EQ_FLOOR_DB);
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
