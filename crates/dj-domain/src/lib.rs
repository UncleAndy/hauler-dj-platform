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

mod grid;

pub use grid::{BeatGrid, BeatSegment};

/// Version tag of the command protocol. Bump it on any incompatible change.
pub const COMMAND_PROTOCOL: &str = "command-v1";
/// Version tag of the state snapshot protocol. Bump it on any incompatible change.
pub const STATE_PROTOCOL: &str = "state-v1";

/// An edit the DJ makes to a beat grid, in the beats the grid is made of.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GridEdit {
    /// Move every beat by this many frames; positive is later. At 48 kHz, 48 frames are a millisecond.
    Shift {
        /// Frames to move the grid by.
        frames: i64,
    },
    /// Give the track this tempo, keeping its first beat where it is.
    Tempo {
        /// Beats per minute, as the DJ hears them.
        bpm: f64,
    },
    /// Read the same grid twice as fast: every other beat becomes a beat (the "×2" of a half-time
    /// reading).
    Double,
    /// Read the same grid half as fast: every second beat is the beat (the "½" of a double-time
    /// reading).
    Half,
    /// Put a beat on this frame — the play head, where the DJ hears the beat they are looking at.
    ///
    /// A shift by however far the nearest beat of the grid is from that frame: the tempo and the counting
    /// stay as they were, and the grid moves the shortest way onto the head. It is the button for a grid
    /// that is right about the tempo and a few tens of milliseconds out of place at the play head, which is
    /// the one mistake the ten-millisecond arrows take many presses to fix.
    Here {
        /// Frame of the track the beat has to land on.
        frame: u64,
    },
}

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

/// What a deck does when it follows another deck's beat (DJP-SYNC-002).
///
/// Off is the default and the honest state of a deck nobody synchronised: the phase is still reported,
/// because a DJ wants to see how far apart two decks are before deciding to lock them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Nothing is followed; the deck keeps its own speed and its own beat.
    #[default]
    Off,
    /// Follow the master's tempo, keep the deck's own beat where it stands.
    Tempo,
    /// Follow the master's tempo and stand on its beat.
    Beat,
}

impl SyncMode {
    /// Whether the deck follows anything at all.
    ///
    /// Taken by reference because serde's `skip_serializing_if` hands it one: a mode of `off` is left
    /// off the wire rather than published twenty times a second.
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }
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
    /// Measure the deck's tempo again and replace the stored grid (B02, DJP-MEDIA-006).
    ///
    /// The deck's own analysis runs once, at load; this is the DJ saying "measure it again", after the
    /// first answer was wrong — so it overrides what the file remembers instead of deferring to it.
    AnalyzeTempo {
        /// Target deck.
        deck: DeckId,
        /// The file the deck holds, as the client spelled it when it loaded it.
        path: String,
    },
    /// Edit the beat grid of the track in this deck (DJP-SYNC-003).
    ///
    /// The DJ is the one who decides where the beat is: the measurement is a first answer, and every
    /// edit here overrules it for good — the grid is written into the file and locked against the next
    /// automatic measurement.
    EditGrid {
        /// Target deck.
        deck: DeckId,
        /// The file the deck holds, as the client spelled it when it loaded it: the edit is written
        /// beside the music.
        path: String,
        /// What to do to the grid.
        edit: GridEdit,
    },
    /// Add files and folders to the collection (DJP-MEDIA-004).
    ///
    /// The engine does the work: the collection is written by one process, and the browser only says
    /// what the DJ picked. Progress and the duplicate question travel back in the snapshot.
    Import {
        /// Files and folders, as the browser spells them.
        paths: Vec<String>,
        /// The collection root to add them to.
        root: String,
        /// Run the batch on several workers.
        parallel: bool,
    },
    /// Stop the running import between files: the file in hand is finished.
    ImportCancel,
    /// Measure a file that is not on a deck: tempo and key, written beside it (DJP-MEDIA-004).
    ///
    /// The collection asks for this when a track arrived without metadata, and when the DJ asks for the
    /// answer again. Acknowledged as `Accepted`: the measurement is a full decode and runs on a worker,
    /// so the new numbers appear in the file and in the next listing, not in this snapshot.
    AnalyzeTrack {
        /// The file to measure.
        path: String,
    },
    /// Answer the duplicate question the engine is waiting on.
    ImportAnswer {
        /// What to do with the file in question.
        answer: DuplicateAnswer,
    },
    /// Return a deck to `Empty` and stop it.
    Eject {
        /// Target deck.
        deck: DeckId,
    },
    /// The metronome: a click on every beat of this deck's grid (DJP-SYNC-003).
    ///
    /// A click is a reference, not part of the music. It follows the deck's own play head, so a paused
    /// deck is silent and a track that changes tempo clicks in its own tempo — and it goes out beside
    /// the mix on the master bus, because a reference nobody can hear beside the track is no use.
    Metronome {
        /// Target deck: whose grid the click follows.
        deck: DeckId,
        /// Whether the click sounds.
        enabled: bool,
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
    /// Play the material backwards from the playhead (DJP-DECK-007).
    ///
    /// Critical: a lost "off" would leave a deck running into the start of the file. A deck whose
    /// source cannot be walked backwards — one still streaming from disk — refuses with `NotSupported`,
    /// because the alternative is a callback waiting on a decoder.
    SetReverse {
        /// Target deck.
        deck: DeckId,
        /// Whether the material plays backwards.
        enabled: bool,
    },
    /// Switch slip on or off (DJP-DECK-007).
    ///
    /// With slip on, a jump or a loop moves the sound but not the virtual play head; switching it off
    /// puts the sound where the track would be. Critical: a lost "off" would leave the deck slipped.
    SetSlip {
        /// Target deck.
        deck: DeckId,
        /// Whether slip is on.
        enabled: bool,
    },
    /// Beatjump: move the play head by a musical distance (DJP-DECK-006).
    ///
    /// Fractions and multiples of a beat are one command: `beats` is a number and its sign says which
    /// way. With `quantize` the jump lands on a line of the deck's grid — the grid the loop button and
    /// the wave already obey — and without it the jump is a plain step of time from where the play head
    /// stands, offset inside the beat kept.
    BeatJump {
        /// Target deck.
        deck: DeckId,
        /// Beats to move: `0.25`, `0.5`, `1`, `2`, `4`, `8`, `32`; negative jumps back.
        beats: f64,
        /// Land on the grid instead of a plain step of time.
        quantize: bool,
    },
    /// Tempo change, 1.0 is the source tempo.
    SetTempoRatio {
        /// Target deck.
        deck: DeckId,
        /// Playback rate multiplier.
        ratio: f64,
    },
    /// Follow another deck's beat, or stop following it (DJP-SYNC-002).
    Sync {
        /// The following deck.
        deck: DeckId,
        /// What it follows: nothing, the tempo, or the tempo and the beat.
        mode: SyncMode,
    },
    /// Make this deck the master the others follow (DJP-SYNC-001).
    ///
    /// A master is a deck, not a number: the others need its beat, not only its tempo, and a tempo
    /// without a phase cannot put two kicks together.
    Master {
        /// The deck that leads.
        deck: DeckId,
    },
    /// No deck leads until a hand names one (DJP-SYNC-001).
    ///
    /// The decks that were following stop following, and keep playing exactly as they were: the tempo
    /// a hand set is a decision, not a state to undo. A lit SYNC over nothing would be a button lying
    /// about what it does, so the mode goes off with the leader.
    MasterOff,
    /// Bend the rate the way a hand on the platter does (DJP-DECK-006): `-1 … 1`, zero lets go.
    ///
    /// Continuous: the shell sends the deflection while the control is held and zero on release. It is
    /// not a tempo change — the fader keeps its value, and the deck returns to it when the hand is off.
    Nudge {
        /// Target deck.
        deck: DeckId,
        /// Deflection, `-1 … 1`, of the deck's nudge range.
        amount: f32,
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
    /// Put a loop of a musical length under the play head (DJP-DECK-005).
    ///
    /// A loop made by two presses is the frames between them: musical only if the hand was on the beat.
    /// This one is built from the grid instead — the start snaps to the nearest beat and the end is
    /// `beats` beats later, so the seam is where the bar is. Refused when the deck has no grid, when
    /// `beats` is zero, or when the region would cross another loop.
    LoopBeats {
        /// Target deck.
        deck: DeckId,
        /// Length in beats: one, two, four, eight.
        beats: u32,
    },
    /// Move the active loop by whole beats, keeping its length (DJP-DECK-005).
    ///
    /// The play head that stood inside the loop moves with it: a loop that is moved out from under the
    /// sound is a jump, and the DJ asked for a loop, not for a jump.
    LoopMove {
        /// Target deck.
        deck: DeckId,
        /// Beats to move: positive is later in the track, negative is earlier.
        beats: i32,
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
            | DjCommand::Metronome { .. }
            | DjCommand::SetPadLoop { .. }
            | DjCommand::SetLoopMode { .. }
            | DjCommand::SetCueMeta { .. }
            | DjCommand::Cue { .. }
            | DjCommand::SeekFrames { .. }
            | DjCommand::SetLoop { .. }
            | DjCommand::LoopBeats { .. }
            | DjCommand::LoopMove { .. }
            | DjCommand::BeatJump { .. }
            | DjCommand::SetSlip { .. }
            | DjCommand::SetReverse { .. }
            | DjCommand::Sync { .. }
            | DjCommand::Master { .. }
            | DjCommand::MasterOff
            | DjCommand::Load { .. }
            | DjCommand::AnalyzeTempo { .. }
            | DjCommand::EditGrid { .. }
            | DjCommand::Import { .. }
            | DjCommand::ImportCancel
            | DjCommand::AnalyzeTrack { .. }
            | DjCommand::ImportAnswer { .. }
            | DjCommand::Eject { .. } => true,
            DjCommand::SetTempoRatio { .. }
            | DjCommand::Nudge { .. }
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
            | DjCommand::Nudge { deck, .. }
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
            DjCommand::Nudge { .. } => Some(CoalesceKey::Nudge),
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
    /// A nudge of a deck: the last deflection wins.
    Nudge,
    /// One band of a deck's equaliser: the last value of that band wins.
    ChannelEq,
}

impl CoalesceKey {
    /// Number of distinct coalesced controls.
    ///
    /// This has to name every variant: it sizes the queue, and it used to say three while five keys
    /// existed, so the master gain landed past the end of the array.
    pub const COUNT: usize = 8;

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
            CoalesceKey::Nudge => 6,
            // The EQ keeps the last index on purpose: its slots are a block of three per deck, and the
            // block sits after every deck-wide control.
            CoalesceKey::ChannelEq => 7,
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
    /// Frame of the first beat of the grid: the beat lines sit at
    /// `first_beat_frame + k · 60 / bpm · rate`. Defaulted and omitted when unknown, so a snapshot from
    /// a build without a tempo detector reads the same as one from a file without a measured grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_beat_frame: Option<u64>,
    /// The beat grid in full: one segment per tempo, so a track that changes tempo is described
    /// honestly (DJP-SYNC-003). Absent when nothing was measured and nothing was edited, which reads
    /// the same as a file without a grid; `bpm` and `first_beat_frame` stay as the first segment for
    /// clients that only know a single tempo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<BeatGrid>,
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

/// Whether a count or a rate is zero, so a field that says nothing stays off the wire.
fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// Whether a bend is at rest, so a nudge that is not happening stays off the wire.
fn is_no_bend(value: &f32) -> bool {
    value.abs() < f32::EPSILON
}

/// Whether a flag is false, so a switch that is off stays off the wire.
fn is_false(value: &bool) -> bool {
    !*value
}

/// Whether a count of events is zero: a deck that has never clicked says nothing about clicks.
fn is_zero_count(value: &u64) -> bool {
    *value == 0
}

/// State of one deck in a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeckSnapshot {
    /// Which deck this describes.
    pub deck: DeckId,
    /// Blocks the deck asked its source for and did not get (DJP-SYNC: a starved deck is not a drifted
    /// one, and a phase meter cannot tell them apart on its own). A deck that was never starved says
    /// nothing about it, so the wire format an older client reads stays the same.
    #[serde(default, skip_serializing_if = "is_zero_count")]
    pub underruns: u64,
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
    /// True while a measurement is running over this deck's file (B02).
    ///
    /// Defaulted: a client that knows nothing about re-measuring is not a protocol error, and an old
    /// engine never says a measurement is running.
    #[serde(default)]
    pub analyzing: bool,
    /// True while loop mode repeats this deck's loops.
    ///
    /// The Loop button draws this and keeps no copy of its own: a mode the engine refused would
    /// otherwise look lit, and a lit button over a silent loop is worse than no button.
    #[serde(default, skip_serializing_if = "is_false")]
    pub loop_mode: bool,
    /// True while the metronome clicks this deck's grid.
    ///
    /// Defaulted and omitted when off: a client that knows nothing about a click is not a protocol
    /// error, and an engine that never clicks says nothing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub metronome: bool,
    /// Clicks the metronome has played for this deck since the engine started.
    ///
    /// The one number that shows the click reached the audio path and not only the state: a check
    /// cannot hear, and "the flag is on" is not the same claim as "it clicks".
    #[serde(default, skip_serializing_if = "is_zero_count")]
    pub clicks: u64,
    /// Sample rate of the source material, so a client can turn frames into seconds and beats into
    /// lines on the wave (DJP-SYNC-003). Defaulted to zero: a client that knows nothing about beats is
    /// not a protocol error.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub source_rate: u32,
    /// The file the deck holds. A client that reconnects, or one that wonders whether its own load has
    /// arrived yet, reads it here instead of remembering what it asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Where the play head is in beats: `12.5` is halfway between the twelfth and thirteenth beat of
    /// the grid (DJP-SYNC-003). Defaulted and omitted when the track has no grid, so a client that
    /// knows nothing about beats is not a protocol error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat: Option<f64>,
    /// How far this deck stands from the master inside the bar, in beats, wrapped to half a bar either
    /// way (DJP-SYNC-002). A beat-level phase cannot tell "on the beat" from "on the wrong beat", and on
    /// a bar of four that is a quarter of the music.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_error_beats: Option<f32>,
    /// The tempo under the play head. Differs from `track.bpm` only on a track that changes tempo, and
    /// it is the number the deck shows and sync has to follow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_bpm: Option<f64>,
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
    /// The bend a hand on the platter is applying right now, `-1 … 1` (DJP-DECK-006).
    ///
    /// Defaulted and left off the wire at rest: a client that knows nothing about nudging is not a
    /// protocol error, and a deck whose hand is off is not bending anything.
    #[serde(default, skip_serializing_if = "is_no_bend")]
    pub nudge: f32,
    /// Whether slip is on (DJP-DECK-007). Defaulted and left off the wire when off: a client that knows
    /// nothing about slip is not a protocol error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub slip: bool,
    /// Whether this deck plays its material from memory rather than a decoder.
    ///
    /// Left off the wire when false: a streamed deck is the ordinary case, and a client that knows
    /// nothing about the cache is not a protocol error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cached: bool,
    /// Bytes this deck holds in memory, zero when it streams (DJP-MEDIA cache).
    ///
    /// Said next to `cached` because "in memory" without a size cannot answer the only question a DJ
    /// has about it: will the next track fit.
    #[serde(default, skip_serializing_if = "is_zero_count")]
    pub cached_bytes: u64,
    /// Whether the material plays backwards (DJP-DECK-007). Left off the wire when forward.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
    /// Where the virtual play head stands while slip is on: what the shell draws as the ghost marker.
    /// Absent when nothing is slipping, so "no slip" and "slip at frame zero" cannot be confused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slip_frame: Option<SourceFrame>,
    /// Channel fader in force, linear: 1.0 is unity, 0.0 is silence.
    #[serde(default = "unity")]
    pub fader: f32,
    /// The three band gains in force on this channel (A14), already on the knob's grid.
    ///
    /// Defaulted so a snapshot from a build without the EQ still parses here.
    #[serde(default)]
    pub eq: EqGains,
    /// Whether this deck leads the others (DJP-SYNC-001). Left off the wire when it does not.
    #[serde(default, skip_serializing_if = "is_false")]
    pub master: bool,
    /// What this deck follows (DJP-SYNC-002). Left off the wire when it follows nothing.
    #[serde(default, skip_serializing_if = "SyncMode::is_off")]
    pub sync: SyncMode,
    /// How far this deck's beat stands from the master's, in milliseconds, positive when the deck has
    /// to move forward. Absent when the deck follows nothing or has no beat to compare.
    ///
    /// This is what a phase meter draws, and it is published rather than computed by every client: the
    /// engine is the only place that sees both decks at one instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_error_ms: Option<f64>,
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

/// What the DJ answered when the file offered turned out to be the same recording as one already in
/// the collection (DJP-MEDIA-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateAnswer {
    /// Keep the copy already there; the new file is not added.
    Skip,
    /// Put the new file in place of the old one; the old one goes to `_duplicates/`.
    Replace,
    /// Keep both, without a marker: the pair is a duplicate again on the next addition.
    Add,
    /// Keep both and mark the new copy as a variant, so the pair is not a duplicate again.
    AddMarked,
}

/// What a client needs to judge a duplicate: the file offered and the one already in the collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackFacts {
    /// Path as the engine has it.
    pub path: String,
    /// Name without the extension.
    pub name: String,
    /// Container as the decoder named it, e.g. `FLAC` or `MP3`.
    pub format: String,
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Bits per sample, zero when the container does not say.
    #[serde(default)]
    pub bits: u32,
    /// Average bitrate in kilobits per second, zero when it cannot be computed.
    #[serde(default)]
    pub bitrate_kbps: f64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Size in bytes.
    pub size_bytes: u64,
    /// True when the format keeps every sample.
    pub lossless: bool,
}

/// The question the engine asks when a duplicate needs the DJ's decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateQuestion {
    /// The file the DJ is adding.
    pub incoming: TrackFacts,
    /// The file already in the collection.
    pub existing: TrackFacts,
    /// How much the two fingerprints agree, 0…1.
    pub agreement: f32,
}

/// State of the import the DJ started from the browser (DJP-MEDIA-004, DJP-MEDIA-005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportState {
    /// Files the batch will try.
    pub total: usize,
    /// Files already decided.
    pub done: usize,
    /// Files added to the collection.
    pub placed: usize,
    /// Files left out because they were already there.
    pub skipped: usize,
    /// Files that could not be read.
    pub failed: usize,
    /// True while the batch is running.
    pub running: bool,
    /// The file being worked on, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// The question waiting for an answer, when the batch is paused on one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<DuplicateQuestion>,
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
    /// Bytes the whole-track cache may hold (its budget), zero when the cache is off.
    ///
    /// The deck reports what it costs; this says what it is allowed to cost, which is what makes the
    /// number a fraction instead of a fact nobody can judge.
    #[serde(default, skip_serializing_if = "is_zero_count")]
    pub cache_budget_bytes: u64,
    /// One entry per deck.
    pub decks: Vec<DeckSnapshot>,
    /// The import the DJ started from the browser, while it runs or while it waits for an answer.
    ///
    /// Absent when nothing was imported since the engine started: a client that knows nothing about
    /// imports is not a protocol error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import: Option<ImportState>,
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
