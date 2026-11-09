//! The click of the metronome (DJP-SYNC-003).
//!
//! A click is a reference beat, not music: a short tone on every beat of a deck's grid, played beside
//! the mix. It is generated here rather than read from a file, so it costs no decode, cannot drift and
//! never misses a beat to a slow disk: it fires on the sample where the deck's own play head crosses a
//! beat frame of the grid.
//!
//! Everything below runs inside the audio callback, so it allocates nothing, locks nothing and never
//! waits: the voices live in a fixed array and a click is a phase and a decaying gain.

use dj_domain::BeatGrid;

/// Length of one click, in seconds: long enough to be heard as a click rather than a blip, short
/// enough that two beats never overlap — a beat at 240 BPM is 250 ms.
const CLICK_SECONDS: f32 = 0.020;

/// Pitch of a click on a beat, and of the accent that opens a bar. A fifth apart, so the first beat of
/// the bar is heard without being counted.
const CLICK_HZ: f32 = 1_000.0;
const ACCENT_HZ: f32 = 1_500.0;

/// Peak of a click, linear — about −14 dBFS. A reference that masks the music is worse than none.
const CLICK_LEVEL: f32 = 0.2;

/// How fast a click dies away, in seconds. A sine that starts at zero has no attack to click with, and
/// a four-millisecond tail ends it at a thousandth of its peak: a click, not a pop.
const DECAY_SECONDS: f32 = 0.004;

/// Clicks that can sound at once: one per deck, so two decks clicking never cut each other off.
const VOICES: usize = 4;

/// One click in flight.
#[derive(Clone, Copy)]
struct Voice {
    /// Samples left before it starts.
    delay: usize,
    /// Samples left to play.
    remaining: usize,
    phase: f32,
    step: f32,
    gain: f32,
    decay: f32,
}

impl Voice {
    const SILENT: Self = Self {
        delay: 0,
        remaining: 0,
        phase: 0.0,
        step: 0.0,
        gain: 0.0,
        decay: 1.0,
    };
}

/// The click generator: four voices, fired on the beats of a grid and mixed into the master bus.
pub struct Metronome {
    sample_rate: f32,
    length: usize,
    voices: [Voice; VOICES],
    next: usize,
}

impl Metronome {
    /// A silent metronome for a device at `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        Self {
            sample_rate,
            length: (CLICK_SECONDS * sample_rate).round().max(1.0) as usize,
            voices: [Voice::SILENT; VOICES],
            next: 0,
        }
    }

    /// Whether any click is still sounding or waiting. Lets a caller skip the mix entirely.
    pub fn is_idle(&self) -> bool {
        self.voices.iter().all(|voice| voice.remaining == 0)
    }

    /// Schedule a click `at` samples from the start of the block being rendered.
    pub fn fire(&mut self, at: usize, accent: bool) {
        let frequency = if accent { ACCENT_HZ } else { CLICK_HZ };
        let voice = &mut self.voices[self.next];
        self.next = (self.next + 1) % VOICES;
        *voice = Voice {
            delay: at,
            remaining: self.length,
            phase: 0.0,
            step: std::f32::consts::TAU * frequency / self.sample_rate,
            gain: CLICK_LEVEL,
            decay: (-1.0 / (DECAY_SECONDS * self.sample_rate)).exp(),
        };
    }

    /// Add the clicks in flight to `out`, an interleaved stereo block of `frames` frames.
    ///
    /// A click that has not started yet waits across block boundaries: the hop from "the beat is
    /// there" to "the beat is heard" is a sample, not a block.
    pub fn mix(&mut self, out: &mut [f32], frames: usize) {
        let frames = frames.min(out.len() / 2);
        for voice in &mut self.voices {
            if voice.remaining == 0 {
                continue;
            }
            let start = if voice.delay >= frames {
                voice.delay -= frames;
                continue;
            } else if voice.delay > 0 {
                std::mem::take(&mut voice.delay)
            } else {
                0
            };
            for index in start..frames {
                if voice.remaining == 0 {
                    break;
                }
                let value = voice.gain * voice.phase.sin();
                out[index * 2] += value;
                out[index * 2 + 1] += value;
                voice.phase += voice.step;
                voice.gain *= voice.decay;
                voice.remaining -= 1;
            }
        }
    }
}

/// Where the deck crossed a beat of its grid inside this block, and whether that beat opens a bar.
///
/// The answer is a sample offset into the block, because the play head moves `rate` source frames per
/// output frame and a beat sits at an exact source frame: the crossing is where the two meet. Exactly
/// one beat is reported — a seek or a loop jumps over bars, and a rattle of clicks is not what "the
/// beat is here" sounds like.
pub fn beat_crossing(
    grid: &BeatGrid,
    rate: f64,
    before: u64,
    after: u64,
    frames: usize,
) -> Option<(usize, bool)> {
    if frames == 0 || after <= before || !(rate.is_finite() && rate > 0.0) {
        return None;
    }
    let at_start = grid.beat_at(before, rate);
    let at_end = grid.beat_at(after, rate);
    if at_end <= at_start {
        return None;
    }
    // Half-open on purpose: a beat exactly on the block boundary already counted as the previous
    // block's last sample, and counting it twice would double the tempo.
    let beat = at_start.floor() + 1.0;
    if at_end < beat {
        return None;
    }
    if at_end.floor() > beat {
        return None;
    }
    let frame = grid.frame_of_beat(beat, rate);
    let span = (after - before) as f64;
    let fraction = ((frame - before as f64) / span).clamp(0.0, 1.0);
    let offset = (fraction * frames as f64).round() as usize;
    let metre = u64::from(grid.beats_per_bar());
    let accent = metre > 0 && (beat as u64).is_multiple_of(metre);
    Some((offset.min(frames), accent))
}

#[cfg(test)]
// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use dj_domain::BeatGrid;

    const RATE: f64 = 48_000.0;

    fn grid() -> BeatGrid {
        BeatGrid::single(120.0, 4_800)
            .expect("a grid")
            .with_beats_per_bar(4)
    }

    #[test]
    fn a_crossing_is_placed_inside_the_block_to_the_sample() {
        // 120 BPM at 48 kHz: a beat every 24 000 frames, the first at 4 800. In a 256-frame block that
        // starts at a beat boundary minus 100 frames, the crossing is 100 frames in.
        let grid = grid();
        let before = 4_800 + 24_000 - 100;
        let after = before + 256;
        let (offset, accent) = beat_crossing(&grid, RATE, before, after, 256).expect("a crossing");
        assert_eq!(offset, 100, "the click starts on the frame the beat is on");
        assert!(!accent, "the second beat of the bar is not the accent");

        // The bar's first beat, four beats after the anchor: that one is.
        let before = 4_800 + 4 * 24_000 - 50;
        let (offset, accent) =
            beat_crossing(&grid, RATE, before, before + 256, 256).expect("a crossing");
        assert_eq!(offset, 50);
        assert!(accent, "the first beat of the bar carries the accent");
    }

    #[test]
    fn a_block_without_a_beat_has_no_crossing() {
        let grid = grid();
        assert!(beat_crossing(&grid, RATE, 4_800, 4_800 + 256, 256).is_none());
        // A paused deck does not move, and a deck that jumps back does not click backwards.
        assert!(beat_crossing(&grid, RATE, 30_000, 30_000, 256).is_none());
        assert!(beat_crossing(&grid, RATE, 30_000, 20_000, 256).is_none());
    }

    #[test]
    fn a_jump_over_bars_does_not_rattle() {
        // A seek from the first beat to the fifth covers four beats in one block: the beat is landed
        // on, not counted through.
        let grid = grid();
        assert!(beat_crossing(&grid, RATE, 4_800, 4_800 + 24_000 * 5, 256).is_none());
    }

    #[test]
    fn a_click_sounds_and_then_stops() {
        let mut metronome = Metronome::new(48_000);
        metronome.fire(10, false);
        let mut block = vec![0.0f32; 512 * 2];
        metronome.mix(&mut block, 512);
        assert!(
            block[..10 * 2].iter().all(|sample| *sample == 0.0),
            "silent before the beat"
        );
        assert!(
            block[10 * 2..].iter().any(|sample| sample.abs() > 0.05),
            "audible after it"
        );
        // A 20 ms click at 48 kHz is 960 samples: the rest of a second is silence.
        for _ in 0..40 {
            let mut later = vec![0.0f32; 512 * 2];
            metronome.mix(&mut later, 512);
        }
        assert!(metronome.is_idle(), "the click ended");
    }
}
