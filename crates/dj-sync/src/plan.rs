//! The decision: how fast the follower should run, and whether it has to be moved onto the beat.
//!
//! The planner is a pure function of two readings. It sends nothing and seeks nothing — it says what
//! would put the follower on the master's beat, and refuses when no legal speed does it
//! (DJP-SYNC-002).

use crate::clock::{BeatClock, DeckClock, LockState};
use crate::phase::error_beats;

/// What the follower is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Nothing: the plan reports the phase and leaves the deck alone.
    Off,
    /// Match the master's tempo, keep the deck's own beat where it stands.
    Tempo,
    /// Match the master's tempo **and** stand on its beat.
    Beat,
}

/// How far the pitch may be moved, either way, in percent of the file's own speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchLimits {
    /// The largest distance from `1.0`, in percent: `16.0` is the usual fader travel.
    pub max_percent: f64,
}

impl PitchLimits {
    /// The limits a symmetric pitch fader of this travel gives.
    pub fn percent(max_percent: f64) -> Self {
        Self {
            max_percent: max_percent.abs(),
        }
    }

    /// The lowest legal ratio.
    pub fn lowest(&self) -> f64 {
        1.0 - self.max_percent / 100.0
    }

    /// The highest legal ratio.
    pub fn highest(&self) -> f64 {
        1.0 + self.max_percent / 100.0
    }

    /// Whether a ratio is inside the range.
    pub fn holds(&self, ratio: f64) -> bool {
        ratio.is_finite() && ratio >= self.lowest() && ratio <= self.highest()
    }
}

impl Default for PitchLimits {
    /// Sixteen percent, the travel of the decks as the reference sets it.
    fn default() -> Self {
        Self { max_percent: 16.0 }
    }
}

/// The move that puts the follower on the master's beat, in the follower's own frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jump {
    /// The distance in beats: positive is forward, negative is back.
    pub beats: f64,
    /// The same distance in source frames, rounded to the frame the deck can seek to.
    pub frames: i64,
}

/// What the engine should do, or why it should do nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPlan {
    /// The mode the plan was made for.
    pub mode: SyncMode,
    /// The ratio the follower should run at; for [`SyncMode::Off`] it is the ratio it has.
    pub ratio: f64,
    /// The move onto the master's beat: `Some` only in [`SyncMode::Beat`].
    pub jump: Option<Jump>,
    /// Signed phase error: positive means the follower is **behind** the master, in milliseconds at
    /// the master's tempo. This is what a phase meter draws, so it is reported in every mode.
    pub phase_error_ms: f64,
    /// Tempo of the master as heard, in beats per minute.
    pub master_bpm: f64,
    /// Tempo of the follower's file at its play head, before its own ratio.
    pub follower_bpm: f64,
}

impl SyncPlan {
    /// Whether the beat is already where the plan wants it: no jump worth a seek.
    ///
    /// The band is one frame: a deck cannot be moved by less than that, so a plan asking for zero
    /// frames is asking for nothing.
    pub fn is_on_beat(&self) -> bool {
        self.jump.is_none_or(|jump| jump.frames == 0)
    }
}

/// Why the follower cannot be put on the master's beat.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// The clock is not there any more: its last reading is too old to follow.
    ClockLost {
        /// Name of the clock.
        name: String,
        /// Age of its last reading, in milliseconds.
        age_ms: f64,
    },
    /// The clock says it is stopped, so there is no beat to follow.
    ClockStopped {
        /// Name of the clock.
        name: String,
    },
    /// The clock's tempo is not a tempo.
    TempoUnknown,
    /// Matching the master would need a pitch outside the fader's travel.
    OutOfRange {
        /// The ratio that would match.
        wanted: f64,
        /// The travel that refused it, in percent.
        limit_percent: f64,
    },
    /// The follower has no tempo to be matched, e.g. a file without a grid.
    FollowerWithoutTempo,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClockLost { name, age_ms } => {
                write!(formatter, "the clock {name} is {age_ms:.0} ms old")
            }
            Self::ClockStopped { name } => write!(formatter, "the clock {name} is stopped"),
            Self::TempoUnknown => write!(formatter, "the clock has no tempo"),
            Self::OutOfRange {
                wanted,
                limit_percent,
            } => write!(
                formatter,
                "matching needs {:.2}%, the fader allows {limit_percent:.0}%",
                (wanted - 1.0) * 100.0
            ),
            Self::FollowerWithoutTempo => write!(formatter, "the follower has no tempo"),
        }
    }
}

impl std::error::Error for Refusal {}

/// The ratio that makes the follower's tempo equal the master's.
///
/// A follower whose file runs at 120 and a master heard at 128 needs `1.0666…`: the file has to run
/// six and two thirds percent fast to sound at the master's tempo.
pub fn matching_ratio(master_bpm: f64, follower_bpm: f64) -> Option<f64> {
    if !master_bpm.is_finite()
        || master_bpm <= 0.0
        || !follower_bpm.is_finite()
        || follower_bpm <= 0.0
    {
        return None;
    }
    let ratio = master_bpm / follower_bpm;
    ratio.is_finite().then_some(ratio)
}

/// Plan the follower against the master.
///
/// The master is anything that implements [`BeatClock`]: a deck today, a MIDI or network clock later.
/// In [`SyncMode::Tempo`] the ratio is decided and the beat is left alone; in [`SyncMode::Beat`] the
/// jump that stands the follower on the master's beat comes with it.
pub fn plan(
    master: &dyn BeatClock,
    follower: &DeckClock<'_>,
    mode: SyncMode,
    limits: PitchLimits,
) -> Result<SyncPlan, Refusal> {
    if !master.running() {
        return Err(Refusal::ClockStopped {
            name: master.name().to_string(),
        });
    }
    let lock = master.lock();
    if lock == LockState::Lost {
        return Err(Refusal::ClockLost {
            name: master.name().to_string(),
            age_ms: master.freshness_ms(),
        });
    }
    let master_bpm = master.tempo_bpm();
    if !master_bpm.is_finite() || master_bpm <= 0.0 {
        return Err(Refusal::TempoUnknown);
    }
    let follower_bpm = follower.file_bpm();
    if !follower_bpm.is_finite() || follower_bpm <= 0.0 {
        return Err(Refusal::FollowerWithoutTempo);
    }

    let wanted = matching_ratio(master_bpm, follower_bpm).ok_or(Refusal::TempoUnknown)?;
    let beats_apart = error_beats(follower.phase(), master.phase());
    let phase_error_ms = beats_apart * 60_000.0 / master_bpm;

    if mode == SyncMode::Off {
        return Ok(SyncPlan {
            mode,
            ratio: follower.ratio,
            jump: None,
            phase_error_ms,
            master_bpm,
            follower_bpm,
        });
    }
    if !limits.holds(wanted) {
        return Err(Refusal::OutOfRange {
            wanted,
            limit_percent: limits.max_percent,
        });
    }

    let jump = (mode == SyncMode::Beat).then(|| Jump {
        beats: beats_apart,
        frames: (beats_apart * follower.frames_per_beat()).round() as i64,
    });

    Ok(SyncPlan {
        mode,
        ratio: wanted,
        jump,
        phase_error_ms,
        master_bpm,
        follower_bpm,
    })
}
