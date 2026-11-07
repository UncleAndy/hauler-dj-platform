//! Time stretching that leaves the pitch alone (key lock, DJP-DECK-008).
//!
//! WSOLA: the deck reads windows of the source at a hop that does not match the window length, and
//! starts each window where it continues the previous one most closely. Nothing is resampled, so the
//! pitch stays where the file put it; only the moments the windows begin move.
//!
//! The cost per hop is bounded: one correlation search over a small range, then a windowed mix. A hop
//! is 512 frames — 10,7 ms at 48 kHz — so the search runs a few times per hundred milliseconds, not
//! once per sample.

/// Output frames produced per hop.
pub(crate) const HOP: usize = 512;
/// Analysis window: two hops, so consecutive windows overlap by exactly half.
pub(crate) const WINDOW: usize = HOP * 2;
/// How far a window may slide to find its continuation, in frames.
const SEARCH: usize = 192;
/// Granularity of that search.
const SEARCH_STEP: usize = 4;

/// A WSOLA stage: feed it source frames, take hops of stretched material.
#[derive(Debug, Clone)]
pub(crate) struct Stretcher {
    /// Source frames pulled but not yet played, interleaved stereo.
    input: Vec<f32>,
    /// Where the next window would start if it needed no search, in frames of `input`.
    cursor: f64,
    /// The second half of the previous window, already windowed: the head of the next hop.
    carry: Vec<f32>,
    /// The second half of the previous window, unwindowed: what the next window must continue.
    tails: Vec<f32>,
    /// Rising half of the Hann window; the falling half is the same reversed.
    rising: Vec<f32>,
    /// Whether a window has been placed yet: the first one has nothing to continue.
    started: bool,
}

impl Stretcher {
    pub(crate) fn new() -> Self {
        let mut rising = Vec::with_capacity(HOP);
        for index in 0..HOP {
            let phase = std::f64::consts::PI * index as f64 / WINDOW as f64;
            // Rising half of Hann over the whole window: 0.5 - 0.5 cos(2 pi n / WINDOW).
            let value = 0.5 - 0.5 * (2.0 * phase).cos();
            rising.push(value as f32);
        }
        Self {
            input: Vec::new(),
            cursor: 0.0,
            carry: vec![0.0; HOP * 2],
            tails: vec![0.0; HOP * 2],
            rising,
            started: false,
        }
    }

    /// Forget everything: the playhead is about to jump and the windows behind it are stale.
    pub(crate) fn reset(&mut self) {
        self.input.clear();
        self.cursor = 0.0;
        self.carry.fill(0.0);
        self.tails.fill(0.0);
        self.started = false;
    }

    /// Source frames needed before the next hop can be placed, at this tempo ratio.
    pub(crate) fn needs(&self, ratio: f64) -> usize {
        let wanted = self.cursor + HOP as f64 * ratio + WINDOW as f64 + SEARCH as f64;
        let have = (self.input.len() / 2) as f64;
        (wanted.ceil() - have).max(0.0) as usize
    }

    /// Append source frames (interleaved stereo).
    pub(crate) fn push(&mut self, frames: &[f32]) {
        self.input.extend_from_slice(frames);
    }

    /// Place one hop of `ratio`-compressed material into `out`, which holds `HOP` frames.
    ///
    /// Returns how many frames were written: `HOP` when the input could supply a window, zero when it
    /// could not (the caller pulls more and asks again).
    pub(crate) fn hop(&mut self, out: &mut [f32], ratio: f64) -> usize {
        if out.len() < HOP * 2 {
            return 0;
        }
        let have = self.input.len() / 2;
        let base = self.cursor.max(0.0);
        let lowest = (base as isize - SEARCH as isize).max(0) as usize;
        let highest = (base as usize).saturating_add(SEARCH);
        if highest + WINDOW > have {
            return 0;
        }
        // The window that continues the previous one best: same phase, no click at the seam.
        let mut best = base as usize;
        if !self.started {
            // The first hop is the material itself, unwindowed: starting a track through a Hann
            // rising edge is a twenty-millisecond fade-in on every press of Play and after every seek.
            for frame in 0..HOP {
                out[frame * 2] = self.input[best * 2 + frame * 2];
                out[frame * 2 + 1] = self.input[best * 2 + frame * 2 + 1];
            }
            for frame in 0..HOP {
                let source = best + HOP + frame;
                let gain = self.rising[HOP - 1 - frame];
                self.carry[frame * 2] = self.input[source * 2] * gain;
                self.carry[frame * 2 + 1] = self.input[source * 2 + 1] * gain;
                self.tails[frame * 2] = self.input[source * 2];
                self.tails[frame * 2 + 1] = self.input[source * 2 + 1];
            }
            self.started = true;
            self.cursor += HOP as f64 * ratio;
            return HOP;
        }
        if self.started {
            let mut score = f64::NEG_INFINITY;
            let mut offset = lowest;
            while offset <= highest {
                let mut dot = 0.0f64;
                let mut energy = 0.0f64;
                for frame in 0..HOP {
                    for channel in 0..2 {
                        let candidate = f64::from(self.input[(offset + frame) * 2 + channel]);
                        let reference = f64::from(self.tails[frame * 2 + channel]);
                        dot += candidate * reference;
                        energy += candidate * candidate;
                    }
                }
                let normalised = if energy > 0.0 {
                    dot / energy.sqrt()
                } else {
                    0.0
                };
                if normalised > score {
                    score = normalised;
                    best = offset;
                }
                offset += SEARCH_STEP;
            }
            // The search window starts at `lowest`; a candidate that is not a multiple of the step
            // away from the natural start is fine — only its distance from `lowest` matters.
        }
        for frame in 0..HOP {
            let source = best + frame;
            let gain = self.rising[frame];
            out[frame * 2] = self.carry[frame * 2] + self.input[source * 2] * gain;
            out[frame * 2 + 1] = self.carry[frame * 2 + 1] + self.input[source * 2 + 1] * gain;
        }
        for frame in 0..HOP {
            let source = best + HOP + frame;
            let gain = self.rising[HOP - 1 - frame];
            self.carry[frame * 2] = self.input[source * 2] * gain;
            self.carry[frame * 2 + 1] = self.input[source * 2 + 1] * gain;
            self.tails[frame * 2] = self.input[source * 2];
            self.tails[frame * 2 + 1] = self.input[source * 2 + 1];
        }
        self.started = true;
        self.cursor += HOP as f64 * ratio;
        // Drop what no window can reach any more: the buffer would otherwise grow with the track.
        let consumed = (self.cursor as usize).saturating_sub(SEARCH + 1);
        if consumed > WINDOW {
            self.input.drain(..consumed * 2);
            self.cursor -= consumed as f64;
        }
        HOP
    }
}

impl Default for Stretcher {
    fn default() -> Self {
        Self::new()
    }
}
