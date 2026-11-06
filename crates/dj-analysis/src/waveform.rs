//! Waveform peaks: the compressed envelope a UI draws instead of the samples.
//!
//! The timebase is source frames, exactly: bucket `n` covers frames
//! `[n * frames_per_bucket, (n + 1) * frames_per_bucket)`, so a playhead at frame `f` maps to
//! `f / frames_per_bucket` with no scaling fudge (DJP-MEDIA-003).

use std::path::Path;

use dj_codec::{MediaError, StreamDecoder};

/// One column of the waveform: the extremes of the samples it covers, plus how much of them is
/// bass and how much is treble.
///
/// The bands are what a coloured waveform draws: bass heavy passages read warm, hats and cymbals
/// read cold. They are per-bucket RMS of two filtered copies of the mix, not a spectrum — a full
/// FFT per column would cost more than the picture is worth at this resolution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    /// Lowest sample value in the bucket, in [-1, 1].
    pub min: f32,
    /// Highest sample value in the bucket, in [-1, 1].
    pub max: f32,
    /// RMS of the low band (below [`LOW_CUTOFF_HZ`]), in [0, 1].
    pub low: f32,
    /// RMS of the mid band (between the cutoffs), in [0, 1].
    pub mid: f32,
    /// RMS of the high band (above [`HIGH_CUTOFF_HZ`]), in [0, 1].
    pub high: f32,
}

/// Low band edge: everything below this counts as bass.
pub const LOW_CUTOFF_HZ: f32 = 250.0;

/// High band edge: everything above this counts as treble.
pub const HIGH_CUTOFF_HZ: f32 = 4_000.0;

impl Bucket {
    /// Silence.
    pub const SILENT: Self = Self {
        min: 0.0,
        max: 0.0,
        low: 0.0,
        mid: 0.0,
        high: 0.0,
    };

    /// True when the column carries no band information (an envelope written before bands existed).
    pub fn bands_are_missing(&self) -> bool {
        self.low == 0.0 && self.mid == 0.0 && self.high == 0.0
    }
}

/// One-pole lowpass, the cheapest way to split a band without an FFT.
#[derive(Debug, Clone, Copy)]
/// A second-order lowpass: two cascaded one-pole sections with a shared state.
///
/// The one-pole this replaced leaked badly — at 1 kHz a 250 Hz one-pole filter still passes a third
/// of the energy, so the "bass" band was loud in every column of every track and no colouring could
/// separate a kick from a chord. Four poles put the leakage where it belongs: about -24 dB per
/// octave, which keeps the three bands honest.
struct Lowpass4 {
    sections: [f32; 2],
    alpha: f32,
}

impl Lowpass4 {
    fn new(cutoff_hz: f32, sample_rate: u32) -> Self {
        // Two identical one-pole sections cascade to a -3 dB point at 0.6436 of each section's own
        // cutoff, so each section is placed at 1.554x the frequency we actually want. Getting this
        // wrong the other way (too low) makes the pair drop mid content early, and that content
        // leaks into the treble band as if the material were bright.
        let ratio = (cutoff_hz * 1.554 / sample_rate as f32).clamp(0.0, 0.49);
        let alpha = 1.0 - (-std::f32::consts::TAU * ratio).exp();
        Self {
            sections: [0.0, 0.0],
            alpha,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        self.sections[0] += self.alpha * (input - self.sections[0]);
        self.sections[1] += self.alpha * (self.sections[0] - self.sections[1]);
        self.sections[1]
    }
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
    /// Typical level of each band across the whole track: [low, mid, high].
    ///
    /// Without this the picture is one colour: the mid band spans four octaves and therefore holds
    /// more energy than bass or treble in almost any music, so raw energies always weight it up.
    /// Colouring by `band / reference` shows what makes a column different from the track itself —
    /// which is why a kick reads red and a hi-hat reads cold.
    pub band_reference: [f32; 3],
    /// Loud level of each band across the track: [low, mid, high], the 95th percentile.
    ///
    /// Treble carries far less energy than bass in any music, so an absolute comparison never
    /// reaches the cool end of the hue scale. Each band is measured against this instead, which is
    /// what lets a hi-hat read blue next to a kick reading red.
    pub band_level: [f32; 3],
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
    // Two filtered copies of the mix, carried across buckets so a bass note that crosses a bucket
    // boundary is not split into two quiet halves.
    let mut low_filter = Lowpass4::new(LOW_CUTOFF_HZ, sample_rate);
    let mut treble_filter = Lowpass4::new(HIGH_CUTOFF_HZ, sample_rate);

    loop {
        let read = decoder.read_frames(&mut buffer)?;
        if read == 0 {
            break;
        }
        // The decoder fills interleaved stereo; a short final read is a bucket too, otherwise the
        // timebase would drift by the remainder of every track.
        let mut min = 0.0f32;
        let mut max = 0.0f32;
        let mut low_energy = 0.0f32;
        let mut mid_energy = 0.0f32;
        let mut high_energy = 0.0f32;
        for frame in 0..read {
            let left = buffer[frame * 2];
            let right = buffer[frame * 2 + 1];
            let low = left.min(right);
            let high = left.max(right);
            min = min.min(low);
            max = max.max(high);

            let mono = (left + right) * 0.5;
            // Three bands from two fourth-order lowpasses: what is below the low cutoff, the slice
            // between the cutoffs, and what is left above the high one.
            let bass = low_filter.process(mono);
            let below_high = treble_filter.process(mono);
            let mids = below_high - bass;
            let treble = mono - below_high;
            low_energy += bass * bass;
            mid_energy += mids * mids;
            high_energy += treble * treble;
        }
        let frames = read.max(1) as f32;
        buckets.push(Bucket {
            min,
            max,
            low: (low_energy / frames).sqrt(),
            mid: (mid_energy / frames).sqrt(),
            high: (high_energy / frames).sqrt(),
        });
    }

    let band_reference = reference_levels(&buckets);
    let band_level = loud_levels(&buckets);
    Ok(Peaks {
        frames_per_bucket,
        sample_rate,
        buckets,
        band_reference,
        band_level,
    })
}

/// Typical level of each band: the median over the columns where the track is actually playing.
///
/// Two mistakes live here. The median over all columns is dragged down by the silence between hits,
/// and then a band that is quiet most of the time — the bass, on a sparse track — looks enormous the
/// moment it plays, which paints the whole waveform red. So the reference is taken from columns at
/// or above a quarter of the track's loudest column: what the track sounds like when it sounds.
fn reference_levels(buckets: &[Bucket]) -> [f32; 3] {
    let loudest = buckets
        .iter()
        .map(|bucket| bucket.max.abs().max(bucket.min.abs()))
        .fold(0.0f32, f32::max);
    let floor = loudest * 0.25;
    let mut per_band: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut all_columns: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for bucket in buckets {
        if bucket.bands_are_missing() {
            continue;
        }
        for (index, level) in [bucket.low, bucket.mid, bucket.high]
            .into_iter()
            .enumerate()
        {
            all_columns[index].push(level);
            if bucket.max.abs().max(bucket.min.abs()) >= floor {
                per_band[index].push(level);
            }
        }
    }
    let mut reference = [0.0f32; 3];
    for index in 0..3 {
        let values = if per_band[index].len() >= 8 {
            &mut per_band[index]
        } else {
            &mut all_columns[index]
        };
        if values.is_empty() {
            continue;
        }
        values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        reference[index] = values[values.len() / 2].max(1e-6);
    }
    reference
}

/// Loud level of each band: the 95th percentile of the columns where the track plays.
///
/// Not the maximum, because one clipped hit would make every other column of that band look quiet
/// for the whole track.
fn loud_levels(buckets: &[Bucket]) -> [f32; 3] {
    let mut per_band: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for bucket in buckets {
        if bucket.bands_are_missing() {
            continue;
        }
        for (index, level) in [bucket.low, bucket.mid, bucket.high]
            .into_iter()
            .enumerate()
        {
            per_band[index].push(level);
        }
    }
    let mut loud = [1e-6f32; 3];
    for (index, values) in per_band.iter_mut().enumerate() {
        if values.is_empty() {
            continue;
        }
        values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let at = (values.len() * 95) / 100;
        loud[index] = values[at.min(values.len() - 1)].max(1e-6);
    }
    loud
}
