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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WheelCounter {
    last_position: Option<u32>,
    last_timer: Option<u32>,
    // The recent movement by the host's clock. A speed is taken over a window, not between two reports: at 500 Hz a
    // two-millisecond window holds one or two integer ticks for a slow hand, so the rate swings by half and the record
    // stutters. Averaging over a longer window is what makes the mouse scratch smooth, and the wheel can have the same.
    window: Vec<(u32, std::time::Instant)>,
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
        self.window.clear();
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

    /// Ticks per **real** second over the last `SPEED_WINDOW`, or `None` for the first report.
    ///
    /// The rate is not taken between two reports: a two-millisecond window at 500 Hz holds only a couple of integer ticks
    /// for a slow hand, so the raw rate swings by half and the record stutters (the owner's own mouse scratch is smooth
    /// because it never sees a two-tick window). Averaging over roughly twenty milliseconds trades a little latency for a
    /// smooth hand. The device's timer is not used: its unit is not written down anywhere.
    pub fn ticks_per_second(&mut self, position: u32, now: std::time::Instant) -> Option<f64> {
        if self.window.is_empty() {
            self.window.push((position, now));
            return None;
        }
        self.window.push((position, now));
        while self.window.len() > 2
            && now.saturating_duration_since(self.window[0].1) > SPEED_WINDOW
        {
            self.window.remove(0);
        }
        let (oldest_position, oldest_at) = self.window[0];
        let elapsed = now.saturating_duration_since(oldest_at).as_secs_f64();
        if elapsed <= f64::EPSILON {
            return Some(0.0);
        }
        let moved = position.wrapping_sub(oldest_position) as i32;
        Some(f64::from(moved) / elapsed)
    }
}

/// How far back a wheel's speed is averaged. Long enough that an integer tick is a small part of the movement, short
/// enough that the hand still feels immediate.
pub const SPEED_WINDOW: std::time::Duration = std::time::Duration::from_millis(20);

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
    fn a_back_and_forth_leaves_the_record_where_it_was() {
        // The speed is a rate, so what matters is its integral: a hand that goes and comes back must net zero, or the
        // playhead walks through the track on every scratch (owner's report, 25.11). The window only shifts the phase, so
        // a full cycle in steady state — not the first, which needs history — nets nothing.
        let mut wheel = WheelCounter::new();
        let start = std::time::Instant::now();
        let mut positions: Vec<u32> = Vec::new();
        let cycle = 41usize;
        for _ in 0..3 {
            positions.extend((0..=20).map(|step| step * 10));
            positions.extend((0..20).rev().map(|step| step * 10));
        }
        let mut net = 0.0_f64;
        let mut last_ms = 0_u64;
        for (index, position) in positions.iter().enumerate() {
            let at = start + std::time::Duration::from_millis(5 * index as u64);
            if let Some(speed) = wheel.ticks_per_second(*position, at)
                && index >= cycle
                && index < cycle * 3
            {
                let dt = (5 * index as u64 - last_ms) as f64 / 1000.0;
                net += speed * dt;
            }
            last_ms = 5 * index as u64;
        }
        assert!(
            net.abs() < 10.0,
            "two full cycles moved the record by {net} ticks"
        );
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
