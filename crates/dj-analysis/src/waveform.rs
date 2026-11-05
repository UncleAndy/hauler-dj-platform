//! Waveform peaks: the compressed envelope a UI draws instead of the samples.
//!
//! The timebase is source frames, exactly: bucket `n` covers frames
//! `[n * frames_per_bucket, (n + 1) * frames_per_bucket)`, so a playhead at frame `f` maps to
//! `f / frames_per_bucket` with no scaling fudge (DJP-MEDIA-003).

use std::path::Path;

use dj_codec::{MediaError, StreamDecoder};

/// One column of the waveform: the extremes of the samples it covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    /// Lowest sample value in the bucket, in [-1, 1].
    pub min: f32,
    /// Highest sample value in the bucket, in [-1, 1].
    pub max: f32,
}

impl Bucket {
    /// Silence.
    pub const SILENT: Self = Self { min: 0.0, max: 0.0 };
}

/// A whole track's envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    /// Frames each bucket covers. The timebase.
    pub frames_per_bucket: u32,
    /// Source sample rate, so a client can turn buckets into seconds without the file.
    pub sample_rate: u32,
    /// One entry per bucket, in source-frame order.
    pub buckets: Vec<Bucket>,
}

impl Peaks {
    /// Frames described by the envelope.
    pub fn frames(&self) -> u64 {
        self.buckets.len() as u64 * u64::from(self.frames_per_bucket)
    }

    /// Bucket index for a source frame.
    pub fn bucket_for(&self, frame: u64) -> usize {
        if self.frames_per_bucket == 0 {
            return 0;
        }
        usize::try_from(frame / u64::from(self.frames_per_bucket))
            .unwrap_or(usize::MAX)
            .min(self.buckets.len().saturating_sub(1))
    }

    /// Loudest absolute value in the envelope; 0 for silence.
    pub fn peak(&self) -> f32 {
        self.buckets
            .iter()
            .map(|bucket| bucket.min.abs().max(bucket.max.abs()))
            .fold(0.0f32, f32::max)
    }
}

/// Which resolution to build.
///
/// A 500-pixel overview of a five-minute track needs about 512 frames per bucket at 48 kHz; the
/// zoomed view asks for finer buckets from the same file. One number, no magic per call site.
pub const OVERVIEW_FRAMES_PER_BUCKET: u32 = 512;

/// Decode `path` and build its envelope.
///
/// Runs off the audio path: it is a full decode of the file, and for a long track that is seconds.
pub fn analyse(path: &Path, frames_per_bucket: u32) -> Result<Peaks, MediaError> {
    let frames_per_bucket = frames_per_bucket.max(1);
    let mut decoder = StreamDecoder::open(path)?;
    let sample_rate = decoder.sample_rate();
    let bucket_frames = frames_per_bucket as usize;
    let mut buffer = vec![0.0f32; bucket_frames * 2];
    let mut buckets = Vec::new();

    loop {
        let read = decoder.read_frames(&mut buffer)?;
        if read == 0 {
            break;
        }
        // The decoder fills interleaved stereo; a short final read is a bucket too, otherwise the
        // timebase would drift by the remainder of every track.
        let mut min = 0.0f32;
        let mut max = 0.0f32;
        for frame in 0..read {
            let left = buffer[frame * 2];
            let right = buffer[frame * 2 + 1];
            let low = left.min(right);
            let high = left.max(right);
            min = min.min(low);
            max = max.max(high);
        }
        buckets.push(Bucket { min, max });
    }

    Ok(Peaks {
        frames_per_bucket,
        sample_rate,
        buckets,
    })
}
