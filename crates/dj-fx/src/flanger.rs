//! Flanger: a short delay whose length is swept by an oscillator, with feedback.
//!
//! The shape is Zölzer's (*DAFX*, the modulation chapter): a delay in the millisecond range, a low-frequency
//! oscillator moving it, and part of the output returned into the delay. What the ear hears is a comb whose teeth
//! slide, and the amount decides how far they slide and how deep the feedback is.
//!
//! Base and depth are the two numbers that make it sound like a flanger rather than a chorus or a vibrato: 1.5 ms
//! is inside the range where the comb sits in the audible band, and 7 ms of travel is a wide sweep without
//! turning into a pitch wobble.

use crate::FxUnit;
use crate::delay::DelayLine;
use crate::smooth::OnePole;

/// The delay the sweep starts from.
const BASE_SECONDS: f32 = 0.0015;
/// How far the sweep moves from there at the far end of the knob.
const DEPTH_SECONDS: f32 = 0.007;
/// The sweep's rate: slow enough to hear the comb move, fast enough to be a movement.
const LFO_HZ: f32 = 0.25;
/// Feedback at the far end of the knob.
const FEEDBACK: f32 = 0.4;
/// How much of the effect is in the mix: a flanger is a half-and-half effect by nature.
const MIX: f32 = 0.5;
const BUFFER_SECONDS: f32 = 0.05;

/// A swept short delay.
#[derive(Debug, Clone)]
pub struct Flanger {
    amount: f32,
    delay: DelayLine,
    phase: f32,
    phase_step: f32,
    rate: f32,
    depth: OnePole,
    feedback: OnePole,
}

impl Default for Flanger {
    fn default() -> Self {
        Self::new()
    }
}

impl Flanger {
    pub fn new() -> Self {
        Self {
            amount: 0.0,
            delay: DelayLine::with_capacity(2),
            phase: 0.0,
            phase_step: 0.0,
            rate: 48_000.0,
            depth: OnePole::new(),
            feedback: OnePole::new(),
        }
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

impl FxUnit for Flanger {
    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.delay.prepare(self.rate, BUFFER_SECONDS);
        self.phase_step = LFO_HZ / self.rate;
        self.depth.set_time(self.rate, 0.020);
        self.depth.reset(self.amount);
        self.feedback.set_time(self.rate, 0.020);
        self.feedback.reset(FEEDBACK * self.amount);
        self.phase = 0.0;
    }

    fn reset(&mut self) {
        self.delay.clear();
        self.phase = 0.0;
        self.depth.reset(self.amount);
        self.feedback.reset(FEEDBACK * self.amount);
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        let depth_target = self.amount;
        let feedback_target = FEEDBACK * self.amount;
        self.depth.advance(depth_target, frames);
        self.feedback.advance(feedback_target, frames);
        let depth = self.depth.value();
        let feedback = self.feedback.value();

        let capacity = self.delay.capacity();
        for frame in 0..samples / 2 {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];
            // A triangle would change speed at the ends; the sine keeps the sweep even, which is why the classic
            // flanger uses one.
            let sweep = (self.phase * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            self.phase += self.phase_step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            let seconds = BASE_SECONDS + DEPTH_SECONDS * depth * sweep;
            let back = ((seconds * self.rate) as usize).clamp(1, capacity - 2);
            let delayed_left = self.delay.read_back(back, 0);
            let delayed_right = self.delay.read_back(back, 1);
            self.delay.push(
                left + delayed_left * feedback,
                right + delayed_right * feedback,
            );
            out[frame * 2] = left * (1.0 - MIX) + delayed_left * MIX;
            out[frame * 2 + 1] = right * (1.0 - MIX) + delayed_right * MIX;
        }
    }
}
