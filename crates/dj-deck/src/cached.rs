//! A deck reader over material that is already in memory: the track cache.
//!
//! Decoding is the one thing in the deck's path that can miss a deadline: a slow disk or a heavy codec
//! turns a block into an underrun, and a seek into a wait. A file that fits the cache is decoded once, off
//! the audio thread, and from then on the deck reads it like a tape: a memcpy for a block, an assignment
//! for a seek, nothing that can block.
//!
//! The cache holds the file **at its own sample rate**, interleaved stereo. Rate conversion and tempo are
//! the deck's business, not the cache's: one copy per file, and no second resampler in the path.

use std::sync::Arc;

use dj_codec::CachedReader;

use crate::DeckSource;

/// A deck's source over a decoded file.
pub struct CachedSource {
    /// Interleaved stereo frames, at the file's own rate.
    data: Arc<[f32]>,
    /// The file's sample rate: the deck converts it to the device's rate as it does for any file.
    rate: u32,
    /// Next frame to hand out.
    position: u64,
}

impl CachedSource {
    /// A reader standing at the start of the material.
    pub fn new(data: Arc<[f32]>, rate: u32) -> Self {
        Self {
            data,
            rate,
            position: 0,
        }
    }

    /// Frames the cache holds.
    pub fn frames(&self) -> u64 {
        (self.data.len() / 2) as u64
    }

    /// Bytes the cache holds: what the budget is spent on.
    pub fn bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
    }
}

impl DeckSource for CachedSource {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available =
            usize::try_from(self.frames().saturating_sub(self.position)).unwrap_or(usize::MAX);
        let take = want.min(available);
        let from = usize::try_from(self.position).unwrap_or(usize::MAX);
        out[..take * 2].copy_from_slice(&self.data[from * 2..(from + take) * 2]);
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        // The same rule the other sources keep: a play head never leaves the material, and frame zero of
        // an empty file is not a frame at all.
        self.position = frame.min(self.frames().saturating_sub(1));
        Ok(self.position)
    }
}

/// A deck's source over a cache entry: the same material the reader holds, in the deck's own terms.
///
/// The cache stores samples in its own format, so this adapter is where bytes become the `f32` the deck
/// works in — once per block, off the decision path, and never a decode.
pub struct CachedReaderSource {
    reader: CachedReader,
}

impl CachedReaderSource {
    /// A reader standing where the cache entry's reader stands.
    pub fn new(reader: CachedReader) -> Self {
        Self { reader }
    }

    /// The entry behind this reader, for reports and for pinning.
    pub fn info(&self) -> &dj_codec::MediaInfo {
        self.reader.info()
    }
}

impl DeckSource for CachedReaderSource {
    fn sample_rate(&self) -> u32 {
        self.reader.info().sample_rate
    }

    fn frames(&self) -> Option<u64> {
        self.reader.frames_expected()
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        self.reader
            .read_frames(out)
            .map_err(|error| format!("cache: {error}"))
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.reader
            .seek(frame)
            .map_err(|error| format!("cache: {error}"))
    }
}
