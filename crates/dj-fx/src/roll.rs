//! Roll / beat repeat: while the switch is held, a captured fraction of a beat plays round and round.
//!
//! The seam is the whole problem, and the answer is the standard one (Zölzer, *DAFX*; the equal-power pair is the
//! cookbook's): the last `fade` frames of a pass are crossfaded into the first frames of the same pass, with two
//! read positions `length - fade` apart. The pass then restarts at `fade`, so the join is continuous and the loop
//! costs `length - fade` frames instead of `length` — which is why the fade is short and the length is not.
//!
//! The loop is captured **forward**: what repeats is the beat that *follows* the press, recorded as it plays, so the
//! first pass is the deck itself, sample for sample. Capturing the beat before the press instead (which this did until
//! 25.11) jumps backwards in the material on the press — a click, and a phrase the hand did not ask for.
//!
//! The ring stops taking material once the pass has been recorded. A roll held while the deck plays on is exactly the
//! case a moving ring ruins: after the ring's two seconds it comes round and overwrites the captured slice, and the
//! "loop" quietly turns back into a window of live audio (measured: at 1.94 s, `crates/dj-fx/tests/roll_hold.rs`).

use crate::FxUnit;
use crate::delay::DelayLine;
use crate::smooth::equal_power;

/// The shortest and the longest fraction of a beat the knob can capture.
///
/// The travel between them is continuous on purpose (owner's report, 21.11: four steps read as a knob that does
/// nothing for most of its turn). Sixteenth to half a beat is the range a beat repeat is used in.
const SHARE_MIN: f32 = 1.0 / 16.0;
const SHARE_MAX: f32 = 0.5;
/// The share the centre of the knob stands for: an eighth of a beat. Turning left shortens towards a sixteenth, turning
/// right lengthens towards half, so the two ways are two different stutters and not one range read twice.
const SHARE_SHORT: f32 = 1.0 / 8.0;
/// The seam's crossfade: 8 ms is long enough to hide a jump and short enough not to smear the loop.
const FADE_SECONDS: f32 = 0.008;
/// The capture buffer: half a beat at the slowest tempo the shell allows, with room to spare.
const BUFFER_SECONDS: f32 = 2.0;
const DEFAULT_BEAT: f32 = 0.5;

/// A beat repeat.
#[derive(Debug, Clone)]
pub struct Roll {
    amount: f32,
    beat: f32,
    buffer: DelayLine,
    /// The loop's length in frames, and where inside it the read head stands.
    length: usize,
    play: usize,
    /// The ring index where the captured segment begins: the loop is anchored here, so live material arriving later
    /// cannot drift into what is playing.
    origin: usize,
    fade: usize,
    captured: bool,
    /// Whether the first pass is still being recorded. Only the recording pass writes to the ring — after it the slice
    /// must survive however long the switch is held.
    recording: bool,
    engaged: bool,
    rate: f32,
}

impl Default for Roll {
    fn default() -> Self {
        Self::new()
    }
}

impl Roll {
    pub fn new() -> Self {
        Self {
            amount: 0.0,
            beat: DEFAULT_BEAT,
            buffer: DelayLine::with_capacity(2),
            length: 1,
            play: 0,
            origin: 0,
            fade: 1,
            captured: false,
            recording: false,
            engaged: false,
            rate: 48_000.0,
        }
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        // A new length is a new capture, and only a *new* one: the rack sends the whole patch on every change (the
        // switch itself sends one), so a message that repeats the length the loop already has must leave the read head
        // where it is — otherwise a held roll stutters on every message (measured: 0.89 of a jump, 25.11).
        let wanted = self.length_for(self.amount);
        if wanted != self.length {
            self.length = wanted;
            if self.engaged && self.captured {
                // The capture runs forward, so re-anchoring on the material arriving now is inaudible: the first pass
                // of the new loop is the deck itself.
                self.origin = (self.origin + self.play) % self.buffer.capacity().max(1);
                self.play = 0;
                self.recording = true;
            }
        }
    }

    /// The deck's beat, in seconds of output.
    pub fn set_beat(&mut self, seconds_per_beat: f32) {
        if seconds_per_beat.is_finite() && seconds_per_beat > 0.0 {
            self.beat = seconds_per_beat;
        }
    }

    /// The loop's length in frames, as the knob and the deck's beat make it.
    pub fn length_frames(&self) -> usize {
        self.length
    }

    /// The share of a beat this position asks for: an eighth at the centre, a sixteenth at the far left, half at the
    /// far right, geometric either way so that equal turns of the knob are equal musical steps.
    fn share_for(amount: f32) -> f32 {
        let travel = amount.abs().clamp(0.0, 1.0);
        let end = if amount < 0.0 { SHARE_MIN } else { SHARE_MAX };
        SHARE_SHORT * (end / SHARE_SHORT).powf(travel)
    }

    fn length_for(&self, amount: f32) -> usize {
        ((self.beat * Self::share_for(amount) * self.rate).round() as usize).max(1)
    }
}

impl FxUnit for Roll {
    fn prepare(&mut self, sample_rate: f32, _max_frames: usize) {
        self.rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.buffer.prepare(self.rate, BUFFER_SECONDS);
        self.fade = ((FADE_SECONDS * self.rate).round() as usize).max(2);
        self.length = self.length_for(self.amount);
        self.play = 0;
        self.origin = 0;
        self.captured = false;
        self.recording = false;
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.play = 0;
        self.captured = false;
        self.recording = false;
    }

    fn engage(&mut self, on: bool) {
        // Switching on captures what is playing *now*: the loop starts from the moment the hand pressed.
        if on && !self.engaged {
            self.length = self.length_for(self.amount);
            self.play = 0;
            // The segment starts here and runs forward: the first pass records what the press began.
            self.origin = self.buffer.write_position();
            self.captured = true;
            self.recording = true;
        }
        self.engaged = on;
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        let capacity = self.buffer.capacity().max(1);
        let length = self.length.min(capacity.saturating_sub(2)).max(2);
        let fade = self.fade.min(length / 2).max(1);
        // Nothing to play until the hand has said so: the deck passes through untouched.
        let held = self.engaged && self.captured;

        for frame in 0..samples / 2 {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];
            if !held {
                out[frame * 2] = left;
                out[frame * 2 + 1] = right;
                continue;
            }

            // The recording pass writes the frame and gives it straight back, so the switch going down is inaudible.
            // After it the ring is left alone: the captured slice must outlive any hold.
            if self.recording {
                self.buffer.push(left, right);
            }
            let play = self.play.min(length - 1);
            let at = (self.origin + play) % capacity;
            // The last `fade` frames of a pass are crossfaded into the head of the same pass: the partner stands
            // `length - fade` frames earlier in the segment, so at the join the two are neighbouring frames.
            let fading = play + fade >= length;
            let into = if fading {
                (play + fade - length) as f32 / fade as f32
            } else {
                0.0
            };
            let partner =
                (at as isize - (length - fade) as isize).rem_euclid(capacity as isize) as usize;

            for channel in 0..2 {
                let current = self.buffer.read_index(at, channel);
                out[frame * 2 + channel] = if fading {
                    let ahead = self.buffer.read_index(partner, channel);
                    let (keep, take) = equal_power(into);
                    current * keep + ahead * take
                } else {
                    current
                };
            }

            self.play += 1;
            if self.play >= length {
                // Restart inside the head of the pass, which is where the crossfade left the signal.
                self.play = fade.min(length - 1);
                self.recording = false;
            }
        }
    }
}
