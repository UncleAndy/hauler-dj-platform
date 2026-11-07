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
const SEARCH: usize = 256;
/// Granularity of that search.
const SEARCH_STEP: usize = 2;

/// A WSOLA stage: feed it source frames, take hops of stretched material.
#[derive(Debug, Clone)]
pub(crate) struct Stretcher {
    /// Source frames pulled but not yet played, interleaved stereo.
    input: Vec<f32>,
    /// Where the next window would start in a world without a tempo change: the previous window plus
    /// one hop. The search moves around *this*, not around the read pointer: the two drift apart by
    /// `HOP * (ratio - 1)` every hop, and a search centred on the read pointer stops finding the
    /// continuation after a few dozen hops — which is heard as roughness, not as a timing error.
    natural: usize,
    /// How far the deck has read ahead, in frames of `input`.
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
            natural: 0,
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
        self.natural = 0;
        self.cursor = 0.0;
        self.carry.fill(0.0);
        self.tails.fill(0.0);
        self.started = false;
    }

    /// Source frames needed before the next hop can be placed, at this tempo ratio.
    pub(crate) fn needs(&self, ratio: f64) -> usize {
        let next = (self.natural + SEARCH + WINDOW) as f64;
        let ahead = self.cursor + HOP as f64 * ratio;
        let wanted = next.max(ahead) + WINDOW as f64;
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
        // The window is searched around the natural continuation, not around the read pointer.
        let centre = self.natural;
        let lowest = centre.saturating_sub(SEARCH);
        let highest = (centre + SEARCH).min(have.saturating_sub(WINDOW));
        if centre + WINDOW > have || highest < lowest {
            return 0;
        }

        let mut best = centre;
        if !self.started {
            // The first hop is the material itself, unwindowed: starting a track through a Hann
            // rising edge is a twenty-millisecond fade-in on every press of Play and after every seek.
            best = centre.min(have.saturating_sub(WINDOW));
            for frame in 0..HOP {
                out[frame * 2] = self.input[best * 2 + frame * 2];
                out[frame * 2 + 1] = self.input[best * 2 + frame * 2 + 1];
            }
            self.write_tail(best);
            self.started = true;
            self.advance(best, ratio);
            self.compact();
            return HOP;
        }

        // The window that continues the previous one best: same phase, no seam. The score is the
        // normalised correlation, so a window cannot win by being louder.
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

        for frame in 0..HOP {
            let source = best + frame;
            let gain = self.rising[frame];
            out[frame * 2] = self.carry[frame * 2] + self.input[source * 2] * gain;
            out[frame * 2 + 1] = self.carry[frame * 2 + 1] + self.input[source * 2 + 1] * gain;
        }
        self.write_tail(best);
        self.advance(best, ratio);
        self.compact();
        HOP
    }

    /// Remember the second half of the window just mixed: half of it is the head of the next hop, the
    /// other half is what that hop has to continue.
    fn write_tail(&mut self, best: usize) {
        for frame in 0..HOP {
            let source = best + HOP + frame;
            let gain = self.rising[HOP - 1 - frame];
            self.carry[frame * 2] = self.input[source * 2] * gain;
            self.carry[frame * 2 + 1] = self.input[source * 2 + 1] * gain;
            self.tails[frame * 2] = self.input[source * 2];
            self.tails[frame * 2 + 1] = self.input[source * 2 + 1];
        }
    }

    /// Move both pointers past the window just played: the natural one by a whole hop, the read one by
    /// what the tempo asks for.
    fn advance(&mut self, best: usize, ratio: f64) {
        self.natural = best + HOP;
        self.cursor = best as f64 + HOP as f64 * ratio;
    }

    /// Drop what no window can reach any more: the buffer would otherwise grow with the track.
    fn compact(&mut self) {
        let keep = self.natural.min(self.cursor.max(0.0) as usize);
        let consumed = keep.saturating_sub(WINDOW);
        if consumed > WINDOW {
            self.input.drain(..consumed * 2);
            self.natural = self.natural.saturating_sub(consumed);
            self.cursor -= consumed as f64;
        }
    }
}

impl Default for Stretcher {
    fn default() -> Self {
        Self::new()
    }
}
