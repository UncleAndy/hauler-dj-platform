//! Shared helpers: committed fixtures and WAV files generated on the fly.
//!
//! Generated WAVs are not committed: a four-second PCM file costs hundreds of kilobytes and the
//! tests can synthesise it deterministically in milliseconds.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A committed fixture under `tests/fixtures/audio`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

/// A unique path in the system temp directory.
pub fn temp_path(name: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("dj-codec-{}-{id}-{name}", std::process::id()))
}

/// Write a stereo 16-bit WAV: silence until `tone_from`, then a 440 Hz tone at 0.6 full scale.
pub fn write_wav(path: &Path, sample_rate: u32, seconds: f64, tone_from: u32) -> u64 {
    let frames = (f64::from(sample_rate) * seconds).round() as u32;
    let mut bytes = Vec::with_capacity(44 + frames as usize * 4);
    let data_len = frames * 4;

    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());

    for frame in 0..frames {
        let value = if frame >= tone_from {
            0.6 * (std::f32::consts::TAU * 440.0 * frame as f32 / sample_rate as f32).sin()
        } else {
            0.0
        };
        let sample = (value.clamp(-1.0, 1.0) * 32_000.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    std::fs::write(path, &bytes).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    u64::from(frames)
}

/// Peak absolute value of an interleaved stereo block.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |acc, value| acc.max(value.abs()))
}
