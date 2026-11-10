//! Moving the pitch to a target without jumping the sound.
//!
//! Tempo sync asks for a new ratio in one step; a deck that obeys in one step is heard as a bump. The
//! engine therefore walks the ratio towards the target, and this is the arithmetic of that walk: a rate
//! of travel and the value after one control tick (DJP-SYNC-002).

/// A limited rate of change for the pitch, in percent of the file's speed per second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateTrim {
    /// How fast the ratio may travel, in percent per second. Four percent per second crosses a whole
    /// fader in four seconds, which is slow enough to be musical and fast enough to follow a hand.
    pub percent_per_second: f64,
}

impl RateTrim {
    /// A trim with this travel.
    pub fn percent_per_second(percent: f64) -> Self {
        Self {
            percent_per_second: percent.abs(),
        }
    }

    /// The ratio one step of `seconds` long may cover.
    pub fn step_size(&self, seconds: f64) -> f64 {
        if !self.percent_per_second.is_finite() || !seconds.is_finite() || seconds <= 0.0 {
            return 0.0;
        }
        self.percent_per_second / 100.0 * seconds
    }

    /// The ratio after one tick: never past the target, never in a jump.
    pub fn step(&self, current: f64, target: f64, seconds: f64) -> f64 {
        if !current.is_finite() || !target.is_finite() {
            return current;
        }
        let size = self.step_size(seconds);
        if size <= 0.0 {
            return target;
        }
        let distance = target - current;
        if distance.abs() <= size {
            return target;
        }
        current + size * distance.signum()
    }
}

impl Default for RateTrim {
    fn default() -> Self {
        Self::percent_per_second(4.0)
    }
}
