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
/// The bands are what a coloured waveform draws: bass-heavy passages read warm, hats and cymbals
/// read cold. They are the RMS of the FFT magnitudes inside each DJ band, computed per column: the
/// spectrum is what separates a kick from a hi-hat, and filters of any order blur that boundary.
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

/// Band edges, in Hz: bass / mids / treble as DJ software draws them.
pub const BASS_RANGE_HZ: (f32, f32) = (20.0, 280.0);
pub const MIDS_RANGE_HZ: (f32, f32) = (280.0, 3_500.0);
pub const TREBLE_RANGE_HZ: (f32, f32) = (3_500.0, 20_000.0);

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

/// In-place iterative radix-2 FFT, the textbook Cooley-Tukey.
///
/// Written out rather than pulled in as a dependency: the project must build offline and stay
/// licence-clean, and this is the only transform it needs.
fn fft_in_place(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    if n <= 1 {
        return;
    }
    debug_assert!(n.is_power_of_two(), "radix-2 needs a power of two");
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }
    // Butterflies, doubling the block length each pass.
    let mut len = 2usize;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        let (sin_half, cos_half) = (angle.sin(), angle.cos());
        let mut start = 0usize;
        while start < n {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for offset in 0..len / 2 {
                let even = start + offset;
                let odd = even + len / 2;
                let tr = real[odd] * wr - imag[odd] * wi;
                let ti = real[odd] * wi + imag[odd] * wr;
                real[odd] = real[even] - tr;
                imag[odd] = imag[even] - ti;
                real[even] += tr;
                imag[even] += ti;
                let next_r = wr * cos_half - wi * sin_half;
                wi = wr * sin_half + wi * cos_half;
                wr = next_r;
            }
            start += len;
        }
        len <<= 1;
    }
}

/// RMS of the FFT magnitudes whose frequency falls inside `range_hz`.
fn band_rms(magnitudes: &[f32], bin_hz: f32, range_hz: (f32, f32)) -> f32 {
    let first = ((range_hz.0 / bin_hz).ceil() as usize).max(1);
    let last = ((range_hz.1 / bin_hz).floor() as usize).min(magnitudes.len().saturating_sub(1));
    if last < first {
        return 0.0;
    }
    let energy: f32 = magnitudes[first..=last]
        .iter()
        .map(|value| value * value)
        .sum();
    (energy / (last - first + 1) as f32).sqrt()
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
pub const OVERVIEW_FRAMES_PER_BUCKET: u32 = 128;

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

    // Two resolutions on purpose. The amplitude of a column wants a short window: 128 samples at
    // 48 kHz is 2.7 ms, which is what makes drum hits needles instead of humps. The BANDS want a
    // long one: with 128 samples the frequency resolution is 375 Hz and bass cannot be told from
    // mids at all, so the spectrum is taken over a 1024-sample window that lags the column by its
    // own length and overlaps the next one by 7/8.
    let fft_size = bucket_frames.next_power_of_two().max(1024);
    let bin_hz = sample_rate as f32 / fft_size as f32;
    let window: Vec<f32> = (0..fft_size)
        .map(|index| {
            let phase = std::f32::consts::TAU * index as f32 / fft_size as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect();
    let window_gain: f32 = window.iter().sum::<f32>() / fft_size as f32;
    let mut real = vec![0.0f32; fft_size];
    let mut imag = vec![0.0f32; fft_size];
    let mut magnitudes = vec![0.0f32; fft_size / 2 + 1];
    // Rolling history of the mix, so the long window always has its 1024 samples.
    let mut history = vec![0.0f32; fft_size];
    let mut history_at = 0usize;

    loop {
        let read = decoder.read_frames(&mut buffer)?;
        if read == 0 {
            break;
        }
        // A short final read is still a column, otherwise the timebase would drift by the
        // remainder of every track.
        let mut min = 0.0f32;
        let mut max = 0.0f32;
        for frame in 0..read {
            let left = buffer[frame * 2];
            let right = buffer[frame * 2 + 1];
            min = min.min(left.min(right));
            max = max.max(left.max(right));
            history[(history_at + frame) % fft_size] = (left + right) * 0.5;
        }
        history_at = (history_at + read) % fft_size;

        // Oldest first, so the window lines up with time.
        for (index, slot) in real.iter_mut().enumerate() {
            *slot = history[(history_at + index) % fft_size] * window[index];
        }
        imag.iter_mut().for_each(|slot| *slot = 0.0);
        fft_in_place(&mut real, &mut imag);
        for (bin, magnitude) in magnitudes.iter_mut().enumerate() {
            let power = real[bin] * real[bin] + imag[bin] * imag[bin];
            // Two-sided spectrum to one-sided amplitude, and back to the signal's own scale.
            *magnitude = 2.0 * power.sqrt() / (fft_size as f32 * window_gain);
        }

        buckets.push(Bucket {
            min,
            max,
            low: band_rms(&magnitudes, bin_hz, BASS_RANGE_HZ),
            mid: band_rms(&magnitudes, bin_hz, MIDS_RANGE_HZ),
            high: band_rms(&magnitudes, bin_hz, TREBLE_RANGE_HZ),
        });
    }

    Ok(Peaks {
        frames_per_bucket,
        sample_rate,
        buckets,
    })
}
