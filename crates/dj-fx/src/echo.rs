//! Beat-locked echo: a delay line whose length is a fraction of the track's beat, feedback with a damped tail.
//!
//! The shape is the one Zölzer describes for a delay effect (*DAFX*, the delay-line chapter): a read tap into a
//! ring buffer, a feedback path that returns the tap through a low-pass so each repeat is darker than the last, and
//! the amount deciding how much of the tap is added to the dry signal. The length is not a number of milliseconds
//! chosen by the user: it is half a beat of the deck's own grid, so the repeats land on the music.
//!
//! Retiming — the deck's tempo or pitch moved — is a crossfade between the old tap and the new one over 20 ms,
//! equal power: jumping a read pointer is a click, and a DJ moving the pitch fader is the normal case, not an
//! accident.

use crate::FxUnit;
use crate::delay::DelayLine;
use crate::smooth::{OnePole, equal_power};

/// How much of the beat one repeat spans on the long side of the centre.
const ECHO_BEAT_SHARE: f32 = 0.5;
/// The tap on the other side: an eighth of the beat — a slapback, short enough to be heard as one repeat and not as a
/// tail. Turning across the centre therefore moves the tap, which is a different effect, not more of the same one.
const SLAP_BEAT_SHARE: f32 = 0.125;
/// Feedback at the ends of the knob: a quarter, then two thirds — the point where a delay starts to run away.
const FEEDBACK_FROM: f32 = 0.2;
const FEEDBACK_TO: f32 = 0.7;
/// Feedback at the ends of the slapback side: a lighter tail, because a slapback that rings like a long echo is a long
/// echo played badly.
const SLAP_FEEDBACK_FROM: f32 = 0.10;
const SLAP_FEEDBACK_TO: f32 = 0.45;
/// The tail's low-pass corner: each repeat loses the top of the spectrum, which is what keeps a long repeat
/// musical instead of ringing.
const DAMP_HZ: f32 = 4_000.0;
/// How long the tap takes to move from the old length to the new one.
const RETIME_SECONDS: f32 = 0.020;
/// The buffer holds this much time: enough for half a beat at the slowest tempo the shell allows.
const BUFFER_SECONDS: f32 = 2.0;
const DEFAULT_BEAT: f32 = 0.5;

/// A beat-locked echo.
#[derive(Debug, Clone)]
pub struct Echo {
    rate: f32,
    amount: f32,
    beat: f32,
    delay: DelayLine,
    /// The tap's length in frames, and the length it is fading away from.
    tap: f32,
    from: f32,
    fade: f32,
    fade_step: f32,
    damp: [OnePole; 2],
    wet: f32,
    feedback: f32,
}

impl Default for Echo {
    fn default() -> Self {
        Self::new()
    }
}

impl Echo {
    pub fn new() -> Self {
        Self {
            rate: 48_000.0,
            amount: 0.0,
            beat: DEFAULT_BEAT,
            delay: DelayLine::with_capacity(2),
            tap: 1.0,
            from: 1.0,
            fade: 1.0,
            fade_step: 1.0,
            damp: [OnePole::new(), OnePole::new()],
            wet: 0.0,
            feedback: FEEDBACK_FROM,
        }
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        // Crossing the centre changes the tap, and a tap that moves is a crossfade: the same 20 ms a tempo change takes.
        self.retime();
    }

    /// How much of the effect, 0 … 1: the distance from the centre, whichever way the knob is turned.
    fn travel(&self) -> f32 {
        self.amount.abs()
    }

    /// The share of a beat one repeat spans: half on the long side, an eighth on the slapback side.
    fn share(&self) -> f32 {
        if self.amount < 0.0 {
            SLAP_BEAT_SHARE
        } else {
            ECHO_BEAT_SHARE
        }
    }

    /// Feedback at the bottom and the top of the side the knob stands on.
    fn feedback_range(&self) -> (f32, f32) {
        if self.amount < 0.0 {
            (SLAP_FEEDBACK_FROM, SLAP_FEEDBACK_TO)
        } else {
            (FEEDBACK_FROM, FEEDBACK_TO)
        }
    }

    /// The deck's beat, in seconds of output. The tap moves to half of it without a click.
    pub fn set_beat(&mut self, seconds_per_beat: f32) {
        if !seconds_per_beat.is_finite() || seconds_per_beat <= 0.0 {
            return;
        }
        if (seconds_per_beat - self.beat).abs() < f32::EPSILON * 8.0 {
            return;
        }
        self.beat = seconds_per_beat;
    }

    /// Recompute the tap for the current beat, rate and side.
    fn retime(&mut self) {
        let wanted = (self.beat * self.share() * self.rate).round().max(1.0);
        let capacity = self.delay.capacity() as f32;
        let wanted = wanted.min(capacity - 2.0);
        if (wanted - self.tap).abs() > 1.0 {
            self.from = self.tap;
            self.tap = wanted;
            self.fade = 0.0;
        }
    }
}

impl FxUnit for Echo {
    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.rate = rate;
        self.delay.prepare(rate, BUFFER_SECONDS);
        for pole in &mut self.damp {
            pole.set_time(rate, 1.0 / (2.0 * std::f32::consts::PI * DAMP_HZ));
            pole.reset(0.0);
        }
        self.fade_step = 1.0 / (RETIME_SECONDS * rate).max(1.0);
        self.tap = (self.beat * self.share() * rate).round().max(1.0);
        self.from = self.tap;
        self.fade = 1.0;
        self.wet = self.travel();
    }

    fn reset(&mut self) {
        self.delay.clear();
        for pole in &mut self.damp {
            pole.reset(0.0);
        }
        self.fade = 1.0;
        self.wet = self.travel();
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        // The knob's values are smoothed across the block: a hand on the knob is a ramp, not a staircase.
        let wet_target = self.travel();
        let (feedback_from, feedback_to) = self.feedback_range();
        let feedback_target = feedback_from + (feedback_to - feedback_from) * self.travel();
        let wet_step = (wet_target - self.wet) / frames as f32;
        let feedback_step = (feedback_target - self.feedback) / frames as f32;
        self.retime();

        let tap = self.tap as usize;
        let from = self.from as usize;
        let fading = self.fade < 1.0;

        for frame in 0..samples / 2 {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];

            // Two taps, one per channel: written out rather than looped, because the delay line takes a channel
            // number and a loop that indexes an array with it is a needless indirection (clippy is right).
            let mut delayed = [0.0f32; 2];
            let newest_left = self.delay.read_back(tap, 0);
            let newest_right = self.delay.read_back(tap, 1);
            if fading {
                let (keep, take) = equal_power(self.fade);
                let older_left = self.delay.read_back(from, 0);
                let older_right = self.delay.read_back(from, 1);
                delayed[0] = older_left * keep + newest_left * take;
                delayed[1] = older_right * keep + newest_right * take;
            } else {
                delayed[0] = newest_left;
                delayed[1] = newest_right;
            }
            if fading {
                self.fade = (self.fade + self.fade_step).min(1.0);
            }

            // The feedback path returns through the low-pass, so the tail darkens as it dies away.
            let damped_left = self.damp[0].process(delayed[0]);
            let damped_right = self.damp[1].process(delayed[1]);
            self.delay.push(
                left + damped_left * self.feedback,
                right + damped_right * self.feedback,
            );

            self.wet += wet_step;
            self.feedback += feedback_step;
            out[frame * 2] = left + delayed[0] * self.wet;
            out[frame * 2 + 1] = right + delayed[1] * self.wet;
        }
        self.wet = wet_target;
        self.feedback = feedback_target;
    }
}
