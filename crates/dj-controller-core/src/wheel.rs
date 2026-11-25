//! A jog wheel's absolute counter, turned into movement.
//!
//! The device sends a position and its own free-running timer, both 32-bit and both wrapping. Nothing here is a guess:
//! the layout of that report lives in the driver's protocol notes, and this type only answers "how far, how fast".
//!
//! Wrapping is the whole difficulty. A position that goes from `0xFFFF_FFF0` to `0x0000_0004` moved forward twelve
//! ticks, not backwards four billion; the signed difference has to be taken in 32-bit arithmetic and read as two's
//! complement, and the timer is unsigned because time only goes one way.

use std::f64::consts::PI;

/// One wheel's counter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WheelCounter {
    last_position: Option<u32>,
    last_timer: Option<u32>,
    // The same movement by the host's clock: the device's timer unit is not written down anywhere, and a speed scaled by
    // an unknown constant is a speed nobody can calibrate.
    last_clock_position: Option<u32>,
    last_clock_at: Option<std::time::Instant>,
}

impl WheelCounter {
    /// A counter that has seen nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the previous report, so the next one answers nothing: a device that reconnects starts over.
    pub fn reset(&mut self) {
        self.last_position = None;
        self.last_timer = None;
        self.last_clock_position = None;
        self.last_clock_at = None;
    }

    /// The movement since the last report: signed ticks and elapsed timer units.
    ///
    /// The first report of a session returns `None` — there is nothing to compare it with, and inventing a starting
    /// point would produce a jump on connection.
    pub fn update(&mut self, position: u32, timer: u32) -> Option<(i32, u32)> {
        // The new report is remembered before the comparison: an early return that forgot to store would leave the
        // wheel permanently without a previous value, and it would never move again.
        let previous = match (self.last_position, self.last_timer) {
            (Some(position), Some(timer)) => Some((position, timer)),
            _ => None,
        };
        self.last_position = Some(position);
        self.last_timer = Some(timer);
        let (previous_position, previous_timer) = previous?;
        // Two's complement in 32 bits: forward past the wrap comes out positive, backward past it negative.
        let moved = position.wrapping_sub(previous_position) as i32;
        let elapsed = timer.wrapping_sub(previous_timer);
        Some((moved, elapsed))
    }

    /// Ticks per timer unit since the last report, or `None` for the first one.
    ///
    /// A report that repeats the same counter costs nothing and says `0.0`: the wheel is still.
    pub fn speed(&mut self, position: u32, timer: u32) -> Option<f64> {
        let (moved, elapsed) = self.update(position, timer)?;
        if elapsed == 0 {
            return Some(0.0);
        }
        Some(f64::from(moved) / f64::from(elapsed))
    }

    /// Ticks per **real** second since the last report, or `None` for the first one.
    ///
    /// The device's timer is not used for a rate: its unit is not written down in any public description, so a speed the
    /// shell scales by an unknown constant cannot be calibrated. Wall time is known on the host, so this is real ticks per
    /// real second — the number a physical model of the platter needs.
    pub fn ticks_per_second(&mut self, position: u32, now: std::time::Instant) -> Option<f64> {
        // The new report is remembered before the comparison, for the same reason as `update`.
        let previous = match (self.last_clock_position, self.last_clock_at) {
            (Some(position), Some(at)) => Some((position, at)),
            _ => None,
        };
        self.last_clock_position = Some(position);
        self.last_clock_at = Some(now);
        let (previous_position, previous_at) = previous?;
        let moved = position.wrapping_sub(previous_position) as i32;
        let elapsed = now.saturating_duration_since(previous_at).as_secs_f64();
        if elapsed <= f64::EPSILON {
            return Some(0.0);
        }
        Some(f64::from(moved) / elapsed)
    }
}

/// Wraps around the counter's period: one full turn of a 32-bit counter.
pub const WRAP: f64 = 4_294_967_296.0;

/// Turns ticks into revolutions, for a caller that wants the number a DJ would recognise.
#[must_use]
pub fn revolutions(ticks: i64, ticks_per_revolution: u32) -> f64 {
    if ticks_per_revolution == 0 {
        return 0.0;
    }
    ticks as f64 / f64::from(ticks_per_revolution)
}

/// Turns revolutions into radians, for a picture of the wheel.
#[must_use]
pub fn radians(revolutions: f64) -> f64 {
    revolutions * 2.0 * PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_report_has_nothing_to_compare_with() {
        let mut wheel = WheelCounter::new();
        assert_eq!(wheel.update(1000, 5000), None);
        assert_eq!(wheel.update(1012, 5060), Some((12, 60)));
    }

    #[test]
    fn a_counter_that_wraps_still_moves_forward() {
        let mut wheel = WheelCounter::new();
        let _ = wheel.update(u32::MAX - 9, 100);
        // Eleven ticks forward: ten to the wrap (the counter lands on zero) and one past it.
        assert_eq!(wheel.update(1, 200), Some((11, 100)));
    }

    #[test]
    fn a_counter_that_wraps_backwards_moves_backwards() {
        let mut wheel = WheelCounter::new();
        let _ = wheel.update(2, 100);
        assert_eq!(wheel.update(u32::MAX - 9, 200), Some((-12, 100)));
    }

    #[test]
    fn speed_is_ticks_over_elapsed_time() {
        let mut wheel = WheelCounter::new();
        let _ = wheel.speed(100, 10);
        assert_eq!(wheel.speed(160, 30), Some(3.0));
    }

    #[test]
    fn a_timer_that_does_not_move_means_no_speed_rather_than_infinity() {
        let mut wheel = WheelCounter::new();
        let _ = wheel.speed(100, 10);
        assert_eq!(wheel.speed(160, 10), Some(0.0));
    }

    #[test]
    fn a_wall_clock_speed_is_ticks_per_real_second() {
        let mut wheel = WheelCounter::new();
        let start = std::time::Instant::now();
        assert_eq!(wheel.ticks_per_second(100, start), None);
        assert_eq!(
            wheel.ticks_per_second(220, start + std::time::Duration::from_millis(100)),
            Some(1200.0)
        );
    }

    #[test]
    fn a_reconnect_starts_over() {
        let mut wheel = WheelCounter::new();
        let _ = wheel.update(100, 10);
        wheel.reset();
        assert_eq!(wheel.update(900, 500), None);
    }

    #[test]
    fn revolutions_are_ticks_over_a_turn() {
        assert!((revolutions(3600, 3600) - 1.0).abs() < f64::EPSILON);
        assert!((revolutions(-1800, 3600) + 0.5).abs() < f64::EPSILON);
        assert_eq!(revolutions(10, 0), 0.0);
    }
}
