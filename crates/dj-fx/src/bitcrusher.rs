//! Bitcrusher: fewer bits of amplitude and a sample-and-hold on the time axis.
//!
//! Both halves are what Zölzer calls *sound degradation* (*DAFX*): quantising the amplitude to a coarse grid, and
//! holding each sample for several frames — the second is what folds the spectrum back and gives the sound its
//! metallic edge. There is no filter here on purpose: the aliasing is the effect.

use crate::FxUnit;
use crate::smooth::OnePole;

/// The quantiser at each end of the knob: sixteen bits is the signal, four is rubble.
const BITS_FROM: f32 = 16.0;
const BITS_TO: f32 = 4.0;
/// How long each sample is held at the far end of the knob.
const HOLD_MAX: f32 = 32.0;

/// Amplitude quantiser with sample-and-hold.
#[derive(Debug, Clone)]
pub struct Bitcrusher {
    amount: f32,
    /// Smoothed, so a hand on the knob steps the sound instead of clicking.
    level: OnePole,
    hold: OnePole,
    counter: usize,
    held: [f32; 2],
}

impl Default for Bitcrusher {
    fn default() -> Self {
        Self::new()
    }
}

impl Bitcrusher {
    pub fn new() -> Self {
        Self {
            amount: 0.0,
            level: OnePole::new(),
            hold: OnePole::new(),
            counter: 0,
            held: [0.0; 2],
        }
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// The quantum a signal in −1 … 1 is rounded to, from the bits in force.
    fn quantum(amount: f32) -> f32 {
        let bits = BITS_FROM + (BITS_TO - BITS_FROM) * amount;
        // Two units across the full scale, `bits` steps in each: the step is 2 / 2^bits.
        2.0f32.powf(1.0 - bits)
    }
}

impl FxUnit for Bitcrusher {
    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.level.set_time(rate, 0.010);
        self.level.reset(self.amount);
        self.hold.set_time(rate, 0.010);
        self.hold.reset(1.0 + (HOLD_MAX - 1.0) * self.amount);
        self.counter = 0;
        self.held = [0.0; 2];
    }

    fn reset(&mut self) {
        self.counter = 0;
        self.held = [0.0; 2];
        self.level.reset(self.amount);
        self.hold.reset(1.0 + (HOLD_MAX - 1.0) * self.amount);
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        self.level.advance(self.amount, frames);
        self.hold
            .advance(1.0 + (HOLD_MAX - 1.0) * self.amount, frames);
        let quantum = Self::quantum(self.level.value());
        let hold = (self.hold.value().round() as usize).max(1);

        for frame in 0..samples / 2 {
            if self.counter == 0 {
                for channel in 0..2 {
                    let sample = input[frame * 2 + channel];
                    // Round to the grid, then keep the value inside full scale: a crusher must not clip, the mixer's
                    // limiter is the only thing allowed to touch the master.
                    self.held[channel] = (sample / quantum)
                        .round()
                        .clamp(-1.0 / quantum, 1.0 / quantum)
                        * quantum;
                }
            }
            out[frame * 2] = self.held[0];
            out[frame * 2 + 1] = self.held[1];
            self.counter += 1;
            if self.counter >= hold {
                self.counter = 0;
            }
        }
    }
}
