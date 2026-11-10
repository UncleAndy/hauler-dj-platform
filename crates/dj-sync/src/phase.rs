//! Where a deck sits inside a beat, and the distance between two decks' beats.

/// A position inside one beat: `0.0` is on the beat, `0.5` is halfway to the next one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phase(f64);

impl Phase {
    /// The phase of a beat position: the fraction of a beat, and nothing else.
    ///
    /// A negative position — a deck parked before its first beat — wraps into the same `0..1` range,
    /// so a beat before the grid counts like a beat after it. That is what a listener hears: the beat
    /// comes round either way.
    pub fn of(beat_position: f64) -> Self {
        if !beat_position.is_finite() {
            return Self(0.0);
        }
        let fraction = beat_position - beat_position.floor();
        Self(if fraction < 0.0 {
            fraction + 1.0
        } else {
            fraction
        })
    }

    /// The fraction itself, in `0..1`.
    pub fn value(self) -> f64 {
        self.0
    }

    /// Whether the phase stands on the beat, within `tolerance` beats on either side.
    ///
    /// The distance to a beat is the shorter of the way forward and the way back: a beat is a circle,
    /// and calling `0.98` almost a beat away from `0.0` would be arithmetic, not music.
    pub fn is_on_beat(self, tolerance: f64) -> bool {
        let distance = self.0.min(1.0 - self.0);
        tolerance.is_finite() && distance <= tolerance.abs()
    }
}

/// The shortest signed distance from `follower` to `master`, in beats, inside `[-0.5, 0.5)`.
///
/// Positive means the follower has to move **forward** by that many beats to stand on the master's
/// beat; negative means it is ahead and has to wait. Half a beat is the furthest a correction ever
/// needs to go, because half a beat behind is the same place as half a beat ahead.
///
/// Exactly half a beat is a tie — forward and back are the same move — and it is always reported as
/// `-0.5`, so the same two readings always produce the same plan and a check can assert it.
pub fn error_beats(follower: Phase, master: Phase) -> f64 {
    let raw = master.value() - follower.value();
    let mut wrapped = raw - (raw + 0.5).floor();
    // The interval is half-open; both edges are folded to the side that makes a jump shortest.
    if wrapped < -0.5 {
        wrapped += 1.0;
    }
    if wrapped >= 0.5 {
        wrapped -= 1.0;
    }
    wrapped
}

/// The distance between two decks inside a bar, in beats, wrapped to half a bar either way.
///
/// This is measured between *beat positions*, not between fractions of a beat: a follower a whole beat
/// behind its master has the same fraction and is a quarter of a four-beat bar away. Positive means the
/// deck has to move forward to stand where the master stands.
pub fn bar_error_beats(follower_beats: f64, master_beats: f64, beats_per_bar: f64) -> f64 {
    if !(beats_per_bar.is_finite()
        && beats_per_bar > 0.0
        && follower_beats.is_finite()
        && master_beats.is_finite())
    {
        return 0.0;
    }
    let half = beats_per_bar / 2.0;
    let raw = master_beats - follower_beats;
    let mut wrapped = raw - (raw / beats_per_bar + 0.5).floor() * beats_per_bar;
    if wrapped < -half {
        wrapped += beats_per_bar;
    }
    if wrapped >= half {
        wrapped -= beats_per_bar;
    }
    wrapped
}
