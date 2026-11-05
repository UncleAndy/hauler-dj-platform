//! Minimal WAV writer for offline renders.
//!
//! 16-bit PCM mono is enough to prove A01: the byte stream is deterministic, so a golden
//! file can be compared by hash (DJP-NFR-001) without pulling in a codec crate.

use std::io::{self, Write};

/// Bits per sample in the files this writer produces.
pub const BITS_PER_SAMPLE: u16 = 16;

/// Write `samples` (in `-1.0..=1.0`) as a mono 16-bit PCM WAV.
pub fn write_pcm16_mono<W: Write>(
    writer: &mut W,
    sample_rate: u32,
    samples: &[f32],
) -> io::Result<()> {
    write_pcm16_interleaved(writer, sample_rate, 1, samples)
}

/// Write interleaved `samples` as a 16-bit PCM WAV with `channels` channels.
pub fn write_pcm16_interleaved<W: Write>(
    writer: &mut W,
    sample_rate: u32,
    channels: u16,
    samples: &[f32],
) -> io::Result<()> {
    let data_len = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX);
    let byte_rate = sample_rate
        .saturating_mul(u32::from(channels))
        .saturating_mul(u32::from(BITS_PER_SAMPLE) / 8);
    let block_align = channels * BITS_PER_SAMPLE / 8;

    writer.write_all(b"RIFF")?;
    writer.write_all(&(36u32.saturating_add(data_len)).to_le_bytes())?;
    writer.write_all(b"WAVE")?;

    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?; // PCM
    writer.write_all(&channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;

    writer.write_all(b"data")?;
    writer.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round();
        writer.write_all(&(scaled as i16).to_le_bytes())?;
    }
    Ok(())
}

/// Audio format of a rendered file, for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderInfo {
    /// Sample rate written into the header.
    pub sample_rate: u32,
    /// Number of frames (mono: samples) written.
    pub frames: u64,
}

impl RenderInfo {
    /// Bytes the data chunk occupies.
    pub fn data_bytes(&self) -> u64 {
        self.frames * u64::from(BITS_PER_SAMPLE / 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_describes_the_payload() {
        let samples = vec![0.0f32; 480];
        let mut bytes = Vec::new();
        write_pcm16_mono(&mut bytes, 48_000, &samples).unwrap_or_else(|err| panic!("{err}"));

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[36..40], b"data");
        let riff_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let data_size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        assert_eq!(data_size, 960, "480 samples * 2 bytes");
        assert_eq!(riff_size, 36 + data_size);
        assert_eq!(bytes.len(), 44 + 960);
    }

    #[test]
    fn samples_are_scaled_and_clipped() {
        let samples = vec![1.0f32, -1.0, 0.5, 2.0, -2.0];
        let mut bytes = Vec::new();
        write_pcm16_mono(&mut bytes, 48_000, &samples).unwrap_or_else(|err| panic!("{err}"));
        let values: Vec<i16> = bytes[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(values[0], i16::MAX);
        assert_eq!(values[1], -i16::MAX);
        assert_eq!(values[3], i16::MAX, "above range clips instead of wrapping");
        assert_eq!(values[4], -i16::MAX);
    }
}
