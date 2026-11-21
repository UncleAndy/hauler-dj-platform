//! A ring buffer of interleaved stereo frames, sized once and never reallocated.
//!
//! Integer delays only: an echo locked to the beat and a roll of a beat fraction do not need fractional reads, and
//! interpolating one would soften the transient the DJ is playing (J. O. Smith, *Physical Audio Signal Processing*,
//! on why a plain delay line is the right tool for a plain delay).

/// A stereo ring buffer.
#[derive(Debug, Clone)]
pub(crate) struct DelayLine {
    data: Vec<f32>,
    write: usize,
    frames: usize,
}

impl DelayLine {
    pub(crate) fn with_capacity(frames: usize) -> Self {
        Self {
            data: vec![0.0; frames.max(1) * 2],
            write: 0,
            frames: frames.max(1),
        }
    }

    /// Size the buffer for a rate and a time in seconds; the samples are cleared.
    pub(crate) fn prepare(&mut self, sample_rate: f32, seconds: f32) {
        let frames = ((sample_rate * seconds).ceil() as usize).max(2);
        self.frames = frames;
        self.data = vec![0.0; frames * 2];
        self.write = 0;
    }

    pub(crate) fn clear(&mut self) {
        self.data.fill(0.0);
        self.write = 0;
    }

    /// How many frames the buffer holds.
    pub(crate) fn capacity(&self) -> usize {
        self.frames
    }

    /// Write one frame and move on.
    #[inline]
    pub(crate) fn push(&mut self, left: f32, right: f32) {
        self.data[self.write * 2] = left;
        self.data[self.write * 2 + 1] = right;
        self.write += 1;
        if self.write == self.frames {
            self.write = 0;
        }
    }

    /// Where the next frame will be written: an anchor for a loop that must stay frozen while time moves on.
    pub(crate) fn write_position(&self) -> usize {
        self.write
    }

    /// The frame at an absolute ring index; the caller wraps it against `capacity()`.
    #[inline]
    pub(crate) fn read_index(&self, index: usize, channel: usize) -> f32 {
        self.data[(index % self.frames) * 2 + (channel & 1)]
    }

    /// The frame written `frames_back` frames ago, where 0 is the newest frame and the oldest the buffer can give
    /// is `capacity()`. `channel` is 0 or 1.
    #[inline]
    pub(crate) fn read_back(&self, frames_back: usize, channel: usize) -> f32 {
        let back = frames_back.min(self.frames - 1);
        let index = (self.write + self.frames - 1 - back) % self.frames;
        self.data[index * 2 + (channel & 1)]
    }
}

impl Default for DelayLine {
    fn default() -> Self {
        Self::with_capacity(1)
    }
}
