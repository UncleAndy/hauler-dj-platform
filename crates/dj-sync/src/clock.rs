//! Where a tempo and a beat phase come from.
//!
//! The deck of a file is the first clock, not the only one: a MIDI clock arrives as 24 ticks per
//! quarter note, a network clock arrives as a shared tempo with a phase someone else maintains, and a
//! future one may arrive as something else again. All of them answer the same three questions, and the
//! planner asks nothing else (DJP-SYNC-001/006).

use dj_domain::BeatGrid;

use crate::phase::Phase;

/// A reading younger than this is trusted without reservation.
pub const LOCK_MS: f64 = 50.0;

/// A reading older than this is not a clock any more: the beat it describes has already happened.
pub const COAST_MS: f64 = 250.0;

/// How much of a clock we still believe, in the language of the person watching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// The clock spoke recently; its beat is followed.
    Locked,
    /// The clock is late but not gone: it is followed, and flagged, so the interface can say so.
    Coasting,
    /// The clock is gone. Following it would be following a memory.
    Lost,
}

impl LockState {
    /// The state a reading of this age is in.
    pub fn of(freshness_ms: f64) -> Self {
        if !freshness_ms.is_finite() || freshness_ms > COAST_MS {
            Self::Lost
        } else if freshness_ms > LOCK_MS {
            Self::Coasting
        } else {
            Self::Locked
        }
    }

    /// Whether the beat of this clock may be followed at all.
    pub fn is_usable(self) -> bool {
        self != Self::Lost
    }
}

/// A source of tempo and beat phase: the one thing the planner needs to know about the world.
pub trait BeatClock {
    /// What to call this clock in a report: `"deck A"`, `"midi"`, `"link"`.
    fn name(&self) -> &str;

    /// Tempo as heard right now, in beats per minute — already including whatever ratio the source
    /// itself runs at.
    fn tempo_bpm(&self) -> f64;

    /// Where inside the beat the clock stands, in `0..1`.
    fn phase(&self) -> Phase;

    /// How old this reading is, in milliseconds. A deck is a reading taken now; an external clock is
    /// whatever arrived last.
    fn freshness_ms(&self) -> f64 {
        0.0
    }

    /// Whether the clock claims to be running. A stopped source has a phase but no beat to follow.
    fn running(&self) -> bool {
        true
    }

    /// The state of this clock, from its own freshness.
    fn lock(&self) -> LockState {
        LockState::of(self.freshness_ms())
    }
}

/// A deck as a clock: the grid says where the beats are, the ratio says how fast the file is running.
///
/// The frame is a **source** frame: the beat of a file does not move when the pitch fader does, and
/// with key lock on the output runs at one speed while the music stands at another.
#[derive(Debug, Clone, Copy)]
pub struct DeckClock<'a> {
    /// The track's grid, as the deck publishes it.
    pub grid: &'a BeatGrid,
    /// Frames per second of the file itself, which is what the grid was measured in.
    pub source_rate: f64,
    /// Where the play head stands, in source frames.
    pub frame: u64,
    /// The tempo ratio applied to the deck: `1.0` is the file's own speed.
    pub ratio: f64,
}

impl DeckClock<'_> {
    /// The beat position of the play head, in fractional beats from the first beat.
    pub fn beat(&self) -> f64 {
        self.grid.beat_at(self.frame, self.source_rate)
    }

    /// The tempo of the file at the play head, before the pitch fader.
    pub fn file_bpm(&self) -> f64 {
        self.grid.bpm_at(self.frame)
    }

    /// Frames of the file in one beat, at the play head.
    pub fn frames_per_beat(&self) -> f64 {
        let beat = self.beat();
        let here = self.grid.frame_of_beat(beat, self.source_rate);
        let next = self.grid.frame_of_beat(beat + 1.0, self.source_rate);
        next - here
    }

    /// The tempo as heard, which is the file's tempo times the ratio.
    pub fn heard_bpm(&self) -> f64 {
        self.file_bpm() * self.ratio
    }
}

impl BeatClock for DeckClock<'_> {
    fn name(&self) -> &str {
        "deck"
    }

    fn tempo_bpm(&self) -> f64 {
        self.heard_bpm()
    }

    fn phase(&self) -> Phase {
        Phase::of(self.beat())
    }
}

/// What an external clock reports about itself, and what the shell shows: the name of the source, its
/// tempo, and whether it is still there.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockStatus {
    /// Human-readable name of the source, e.g. `"midi clock"`.
    pub name: String,
    /// Tempo as heard, in beats per minute.
    pub tempo_bpm: f64,
    /// Age of the last reading, in milliseconds.
    pub freshness_ms: f64,
    /// Whether the source claims to be running.
    pub running: bool,
}

impl ClockStatus {
    /// The lock state of this report.
    pub fn lock(&self) -> LockState {
        if !self.running {
            return LockState::Lost;
        }
        LockState::of(self.freshness_ms)
    }

    /// Whether the source may be followed at all.
    pub fn is_usable(&self) -> bool {
        self.lock().is_usable()
    }
}
