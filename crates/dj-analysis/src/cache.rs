//! On-disk cache for waveforms, keyed by the file's fingerprint.
//!
//! A waveform costs a full decode, so it is stored next to the collection and reused until the file
//! itself changes. The fingerprint is the file's size and modification time: cheap to read, and it
//! changes whenever a track is re-encoded, re-tagged or replaced — which is exactly when the old
//! envelope is wrong (DJP-MEDIA-003).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use dj_codec::MediaError;

use crate::waveform::{Bucket, Peaks};

/// What the cache trusts about a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    /// Size in bytes.
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub modified_ns: u128,
}

impl Fingerprint {
    /// Read the fingerprint of `path`.
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        Ok(Self {
            size: metadata.len(),
            modified_ns,
        })
    }
}

const MAGIC: &[u8; 8] = b"DJWAVE10";

/// Bytes per column on disk: two extremes as i16 and three band levels as u8.
const BUCKET_BYTES: usize = 7;

/// Envelope cache in a directory of its own.
#[derive(Debug, Clone)]
pub struct WaveformCache {
    root: PathBuf,
}

impl WaveformCache {
    /// A cache under `root`; the directory is created on first use.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Where the envelope for `path` at this resolution lives.
    pub fn entry_for(&self, path: &Path, frames_per_bucket: u32) -> PathBuf {
        // The file name carries the resolution: the same track needs several, and a coarse one must
        // never be served where a fine one was asked for.
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("track");
        let digest = fnv1a(path.to_string_lossy().as_bytes());
        self.root
            .join(format!("{stem}-{digest:016x}-{frames_per_bucket}.wave"))
    }

    /// Stored envelope, or `None` when absent or stale.
    pub fn load(&self, path: &Path, frames_per_bucket: u32) -> Option<Peaks> {
        let entry = self.entry_for(path, frames_per_bucket);
        let file = std::fs::File::open(entry).ok()?;
        let mut reader = std::io::BufReader::new(file);
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic).ok()?;
        if &magic != MAGIC {
            return None;
        }
        let mut number = [0u8; 8];
        reader.read_exact(&mut number).ok()?;
        let stored_size = u64::from_le_bytes(number);
        let mut wide = [0u8; 16];
        reader.read_exact(&mut wide).ok()?;
        let stored_modified = u128::from_le_bytes(wide);
        reader.read_exact(&mut number).ok()?;
        let stored_rate = u32::try_from(u64::from_le_bytes(number)).ok()?;
        reader.read_exact(&mut number).ok()?;
        let stored_bucket_frames = u32::try_from(u64::from_le_bytes(number)).ok()?;
        reader.read_exact(&mut number).ok()?;
        let bucket_count = usize::try_from(u64::from_le_bytes(number)).ok()?;
        let mut reference_bytes = [0u8; 3];
        reader.read_exact(&mut reference_bytes).ok()?;
        let mut loud_bytes = [0u8; 3];
        reader.read_exact(&mut loud_bytes).ok()?;

        let current = Fingerprint::of(path).ok()?;
        if current.size != stored_size || current.modified_ns != stored_modified {
            // The file was replaced or re-encoded; the stored envelope describes another file.
            return None;
        }
        if stored_bucket_frames != frames_per_bucket {
            return None;
        }

        let mut raw = vec![0u8; bucket_count * BUCKET_BYTES];
        reader.read_exact(&mut raw).ok()?;
        let mut buckets = Vec::with_capacity(bucket_count);
        for column in raw.chunks_exact(BUCKET_BYTES) {
            let min = i16::from_le_bytes([column[0], column[1]]);
            let max = i16::from_le_bytes([column[2], column[3]]);
            buckets.push(Bucket {
                min: f32::from(min) / 32_767.0,
                max: f32::from(max) / 32_767.0,
                low: dequantise_band(column[4]),
                mid: dequantise_band(column[5]),
                high: dequantise_band(column[6]),
            });
        }
        Some(Peaks {
            frames_per_bucket: stored_bucket_frames,
            sample_rate: stored_rate,
            buckets,
            band_reference: [
                dequantise_band(reference_bytes[0]),
                dequantise_band(reference_bytes[1]),
                dequantise_band(reference_bytes[2]),
            ],
            band_level: [
                dequantise_band(loud_bytes[0]),
                dequantise_band(loud_bytes[1]),
                dequantise_band(loud_bytes[2]),
            ],
        })
    }

    /// Store an envelope for `path`.
    pub fn store(&self, path: &Path, peaks: &Peaks) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        let entry = self.entry_for(path, peaks.frames_per_bucket);
        write_envelope(path, peaks, &entry)?;
        Ok(entry)
    }

    /// Envelope for `path`, from the cache when it is still valid, otherwise computed and stored.
    ///
    /// The second value says whether the cache was used, so a caller can report what it did instead
    /// of guessing.
    pub fn get_or_analyse(
        &self,
        path: &Path,
        frames_per_bucket: u32,
    ) -> Result<(Peaks, bool), MediaError> {
        if let Some(peaks) = self.load(path, frames_per_bucket) {
            return Ok((peaks, true));
        }
        let peaks = crate::waveform::analyse(path, frames_per_bucket)?;
        // A cache that cannot be written is not a failure: the waveform is still usable.
        let _ = self.store(path, &peaks);
        Ok((peaks, false))
    }
}

/// Band levels are stored with a square-root curve: a bass line at -30 dB still has to be visible
/// next to a loud passage, and a linear byte would flatten it to zero.
fn quantise_band(value: f32) -> u8 {
    let scaled = (value.clamp(0.0, 1.0).sqrt() * 255.0).round();
    scaled.clamp(0.0, 255.0) as u8
}

fn dequantise_band(byte: u8) -> f32 {
    let normalised = f32::from(byte) / 255.0;
    normalised * normalised
}

fn quantise(value: f32) -> i16 {
    let scaled = (value.clamp(-1.0, 1.0) * 32_767.0).round();
    scaled.clamp(-32_767.0, 32_767.0) as i16
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Write an envelope for `source` to `entry`, whatever the destination is called.
///
/// The cache names its files by fingerprint and resolution, but a caller that wants to look at one
/// envelope — a command-line render, a test — needs to pick the path itself.
pub fn write_envelope(source: &Path, peaks: &Peaks, entry: &Path) -> std::io::Result<()> {
    let fingerprint = Fingerprint::of(source)?;
    let mut writer = std::io::BufWriter::new(std::fs::File::create(entry)?);
    writer.write_all(MAGIC)?;
    writer.write_all(&fingerprint.size.to_le_bytes())?;
    writer.write_all(&fingerprint.modified_ns.to_le_bytes())?;
    writer.write_all(&u64::from(peaks.sample_rate).to_le_bytes())?;
    writer.write_all(&u64::from(peaks.frames_per_bucket).to_le_bytes())?;
    writer.write_all(&(peaks.buckets.len() as u64).to_le_bytes())?;
    writer.write_all(&[
        quantise_band(peaks.band_reference[0]),
        quantise_band(peaks.band_reference[1]),
        quantise_band(peaks.band_reference[2]),
    ])?;
    writer.write_all(&[
        quantise_band(peaks.band_level[0]),
        quantise_band(peaks.band_level[1]),
        quantise_band(peaks.band_level[2]),
    ])?;
    for bucket in &peaks.buckets {
        let min = quantise(bucket.min);
        let max = quantise(bucket.max);
        writer.write_all(&min.to_le_bytes())?;
        writer.write_all(&max.to_le_bytes())?;
        writer.write_all(&[
            quantise_band(bucket.low),
            quantise_band(bucket.mid),
            quantise_band(bucket.high),
        ])?;
    }
    writer.flush()?;
    Ok(())
}
