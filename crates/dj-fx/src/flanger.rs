//! Flanger: a short delay whose length is swept by an oscillator, with feedback.
//!
//! The shape is Zölzer's (*DAFX*, the modulation chapter): a delay in the millisecond range, a low-frequency
//! oscillator moving it, and part of the output returned into the delay. What the ear hears is a comb whose teeth
//! slide, and the amount decides how far they slide and how deep the feedback is.
//!
//! Base and depth are the two numbers that make it sound like a flanger rather than a chorus or a vibrato: 1.5 ms
//! is inside the range where the comb sits in the audible band, and 7 ms of travel is a wide sweep without
//! turning into a pitch wobble.
//!
//! What the knob does. Two reports from the owner shaped this (21.11): first the travel was too narrow, then its
//! left half was silent. The amount now drives **depth, feedback and the share of the effect in the mix** through a
//! square-root curve — half a turn still gives seventy per cent of the strength — while the sweep runs at a fixed
//! 0.45 Hz. At the very bottom the deck comes through untouched.

use crate::FxUnit;
use crate::delay::DelayLine;
use crate::smooth::OnePole;

/// The delay the sweep starts from.
const BASE_SECONDS: f32 = 0.0015;
/// How far the sweep moves from there at the far end of the knob.
const DEPTH_SECONDS: f32 = 0.007;
/// The sweep's rate. Constant on purpose: tying it to the knob made the lower half of the travel crawl, and a sweep
/// that takes five seconds to come round reads as "nothing is happening" (owner's report, 21.11: turning the knob
/// down changed nothing at all).
const LFO_HZ: f32 = 0.45;
/// The sweep's rate on the other side of the centre: slow enough to read as a movement and not as a wobble.
const LFO_HZ_LEFT: f32 = 0.18;
/// Feedback at the far end of the knob.
const FEEDBACK: f32 = 0.45;
/// How much of the effect is in the mix at the far end: a flanger is a half-and-half effect by nature.
const MIX: f32 = 0.5;
/// The mix on the other side. The comb there is inverted, which hollows the sound out; less of it goes in, or the deck
/// loses its body instead of gaining a colour.
const MIX_LEFT: f32 = 0.35;
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
            amount.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        // The side of the centre decides the sweep's speed, so the oscillator's step is set here as well as in `prepare`.
        self.phase_step = self.lfo_hz() / self.rate;
    }

    /// How much of the effect, 0 … 1: the distance from the centre, whichever way the knob is turned.
    fn travel(&self) -> f32 {
        self.amount.abs()
    }

    /// Whether the knob stands on the hollow side.
    fn inverted(&self) -> bool {
        self.amount < 0.0
    }

    fn lfo_hz(&self) -> f32 {
        if self.inverted() { LFO_HZ_LEFT } else { LFO_HZ }
    }

    /// Feedback at the current position: positive one way, negative the other, so the notches sit on the harmonics one
    /// way and between them the other.
    fn feedback_at(&self, strength: f32) -> f32 {
        let sign = if self.inverted() { -1.0 } else { 1.0 };
        sign * FEEDBACK * strength
    }

    fn mix_at(&self, depth: f32) -> f32 {
        let share = if self.inverted() { MIX_LEFT } else { MIX };
        share * depth
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
        self.phase_step = self.lfo_hz() / self.rate;
        self.depth.set_time(self.rate, 0.020);
        self.depth.reset(self.travel());
        self.feedback.set_time(self.rate, 0.020);
        self.feedback.reset(self.feedback_at(self.travel()));
        self.phase = 0.0;
    }

    fn reset(&mut self) {
        self.delay.clear();
        self.phase = 0.0;
        self.depth.reset(self.travel());
        self.feedback.reset(self.feedback_at(self.travel()));
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        // The strength curve: `sqrt` so that a quarter turn is already half the effect. A linear knob spends most of
        // its left half on changes nobody can hear (owner's report, 21.11).
        let strength = self.travel().sqrt();
        let depth_target = strength;
        let feedback_target = self.feedback_at(strength);
        self.depth.advance(depth_target, frames);
        self.feedback.advance(feedback_target, frames);
        let depth = self.depth.value();
        let feedback = self.feedback.value();
        // Nothing of the effect at the bottom of the travel: the deck has to come back untouched.
        if depth <= 0.0005 {
            out[..samples].copy_from_slice(&input[..samples]);
            // The delay still takes material, so switching on has something to read.
            for frame in 0..samples / 2 {
                self.delay.push(input[frame * 2], input[frame * 2 + 1]);
            }
            return;
        }
        let mix = self.mix_at(depth);

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
            out[frame * 2] = left * (1.0 - mix) + delayed_left * mix;
            out[frame * 2 + 1] = right * (1.0 - mix) + delayed_right * mix;
        }
    }
}
