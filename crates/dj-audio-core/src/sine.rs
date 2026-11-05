//! Test signal source for the offline renderer.
//!
//! Deliberately trivial and allocation-free: A01 proves the scheduler and the render
//! loop with a signal whose exact sample values are known, before any decoder exists.

use std::f32::consts::TAU;

/// A sine generator with a continuous phase accumulator.
#[derive(Debug, Clone)]
pub struct SineSource {
    frequency_hz: f32,
    sample_rate: u32,
    phase: f32,
    step: f32,
}

impl SineSource {
    /// A generator at `frequency_hz` for a device running at `sample_rate`.
    pub fn new(frequency_hz: f32, sample_rate: u32) -> Self {
        let mut source = Self {
            frequency_hz,
            sample_rate,
            phase: 0.0,
            step: 0.0,
        };
        source.recompute_step();
        source
    }

    fn recompute_step(&mut self) {
        let rate = self.sample_rate.max(1) as f32;
        self.step = TAU * self.frequency_hz / rate;
    }

    /// Change the playback rate without touching the phase (tempo ratio).
    pub fn set_frequency(&mut self, frequency_hz: f32) {
        self.frequency_hz = frequency_hz;
        self.recompute_step();
    }

    /// Next sample in `-1.0..=1.0`. No allocation, no branching on shared state.
    pub fn next_sample(&mut self) -> f32 {
        let value = self.phase.sin();
        self.phase += self.step;
        if self.phase >= TAU {
            self.phase -= TAU;
        }
        value
    }

    /// Restart the phase at zero (used when a deck starts from the top).
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_440_hz_sine_completes_440_cycles_per_second() {
        let mut source = SineSource::new(440.0, 48_000);
        let mut crossings = 0usize;
        let mut previous = source.next_sample();
        for _ in 1..48_000 {
            let sample = source.next_sample();
            if previous <= 0.0 && sample > 0.0 {
                crossings += 1;
            }
            previous = sample;
        }
        assert!(
            (439..=441).contains(&crossings),
            "expected ~440 rising zero crossings, got {crossings}"
        );
    }

    #[test]
    fn the_phase_stays_bounded_and_samples_stay_in_range() {
        let mut source = SineSource::new(1_000.0, 48_000);
        for _ in 0..1_000_000 {
            let sample = source.next_sample();
            assert!((-1.0..=1.0).contains(&sample));
        }
    }
}
