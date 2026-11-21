//! Roll / beat repeat: while the switch is held, a captured fraction of a beat plays round and round.
//!
//! The seam is the whole problem, and the answer is the standard one (Zölzer, *DAFX*; the equal-power pair is the
//! cookbook's): the last `fade` frames of a pass are crossfaded into the first frames of the same pass, with two
//! read positions `length - fade` apart. The pass then restarts at `fade`, so the join is continuous and the loop
//! costs `length - fade` frames instead of `length` — which is why the fade is short and the length is not.
//!
//! The loop is captured the moment the switch goes down and stays frozen while it is held: that is the difference
//! between a roll and a plain delay, and it is what makes the effect useful in a transition.

use crate::FxUnit;
use crate::delay::DelayLine;
use crate::smooth::equal_power;

/// The fractions of a beat the knob chooses between.
const SHARES: [f32; 4] = [1.0 / 16.0, 1.0 / 8.0, 1.0 / 4.0, 1.0 / 2.0];
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
    /// The ring index just past the captured segment: the loop is anchored here, so live material arriving later
    /// cannot drift into what is playing.
    origin: usize,
    fade: usize,
    captured: bool,
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
            engaged: false,
            rate: 48_000.0,
        }
    }

    pub fn set_amount(&mut self, amount: f32) {
        self.amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
        // A new length is a new loop. While the switch is held the loop restarts at once; when it is up, the next
        // press captures with the new length — either way the knob is the law.
        self.length = self.length_for(self.amount);
        if self.engaged {
            self.play = 0;
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

    fn length_for(&self, amount: f32) -> usize {
        let index = ((amount * SHARES.len() as f32) as usize).min(SHARES.len() - 1);
        ((self.beat * SHARES[index] * self.rate).round() as usize).max(1)
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
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.play = 0;
        self.captured = false;
    }

    fn engage(&mut self, on: bool) {
        // Switching on captures what is playing *now*: the loop starts from the moment the hand pressed.
        if on && !self.engaged {
            self.length = self.length_for(self.amount);
            self.play = 0;
            // The segment is what has just played: it ends where the write head stands now.
            self.origin = self.buffer.write_position();
            self.captured = true;
        }
        self.engaged = on;
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(input.len()).min(out.len());
        if samples == 0 {
            return;
        }
        let capacity = self.buffer.capacity();
        let length = self.length.min(capacity.saturating_sub(2)).max(2);
        let fade = self.fade.min(length / 2).max(1);
        let live = !self.engaged || !self.captured;

        for frame in 0..samples / 2 {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];
            // The buffer keeps taking material while the loop is held: releasing must return to what is playing now.
            self.buffer.push(left, right);

            if live {
                out[frame * 2] = left;
                out[frame * 2 + 1] = right;
                continue;
            }

            // The segment starts `length` frames before the anchor and does not move while the loop is held.
            let base = (self.origin + capacity - length) % capacity;
            let play = self.play.min(length - 1);
            let at = (base + play) % capacity;
            // The last `fade` frames of a pass are crossfaded into the head of the same pass: the partner stands
            // `length - fade` frames earlier in the segment, so at the join the two are neighbouring frames.
            let fading = play + fade >= length;
            let into = if fading {
                (play + fade - length) as f32 / fade as f32
            } else {
                0.0
            };
            let partner = (base as isize + play as isize - (length - fade) as isize)
                .rem_euclid(capacity as isize) as usize;

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
            }
        }
    }
}
