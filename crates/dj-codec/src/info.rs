//! What the rest of the app is allowed to know about a media file.

use std::fmt;

/// Container/codec family as far as the DJ app cares.
///
/// This is not a Symphonia detail: the classification drives the playback strategy, so it is a
/// domain type (ADR-0008).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFormat {
    /// RIFF/WAVE, uncompressed PCM.
    Wav,
    /// AIFF, uncompressed PCM.
    Aiff,
    /// FLAC, lossless.
    Flac,
    /// MPEG-1/2 Layer III — CBR or VBR, both are treated the same.
    Mp3,
    /// Ogg container with Vorbis audio.
    OggVorbis,
    /// Format the app does not classify; playback may still work, the cache policy will not.
    Other,
}

impl MediaFormat {
    /// Guess the format from a file extension. Case-insensitive, no file access.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Some(MediaFormat::Wav),
            "aif" | "aiff" => Some(MediaFormat::Aiff),
            "flac" => Some(MediaFormat::Flac),
            "mp3" | "mp2" | "mp1" => Some(MediaFormat::Mp3),
            "ogg" | "oga" => Some(MediaFormat::OggVorbis),
            _ => None,
        }
    }

    /// True for formats that need the PCM cache before playback (ADR-0008).
    ///
    /// The decision is a property of the file, never of the moment: two tracks in the same set
    /// must behave the same way.
    pub fn needs_pcm_cache(self) -> bool {
        matches!(self, MediaFormat::Mp3 | MediaFormat::OggVorbis)
    }

    /// True when the format is lossy (frame-based, encoder delay, no exact sample positions).
    pub fn is_lossy(self) -> bool {
        self.needs_pcm_cache()
    }

    /// Short stable name, used in reports and state messages.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaFormat::Wav => "wav",
            MediaFormat::Aiff => "aiff",
            MediaFormat::Flac => "flac",
            MediaFormat::Mp3 => "mp3",
            MediaFormat::OggVorbis => "ogg-vorbis",
            MediaFormat::Other => "other",
        }
    }
}

/// Encoder delay/padding as reported by the container (MP3 LAME tag, iTunSMPB, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaplessInfo {
    /// Frames inserted by the encoder before the first real sample.
    pub delay: u32,
    /// Frames inserted after the last real sample.
    pub padding: u32,
}

/// Everything the decoder knows about a file before the first sample is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    /// Classified format (drives the PCM cache policy).
    pub format: MediaFormat,
    /// Sample rate of the source material, Hz.
    pub sample_rate: u32,
    /// Channel count of the source; 1 or 2 are supported in R0.
    pub channels: u16,
    /// Playable frames, excluding encoder delay and padding, when the container states it.
    pub frames: Option<u64>,
    /// Encoder delay/padding when known (MP3 with a LAME tag, for example).
    pub gapless: Option<GaplessInfo>,
}

impl MediaInfo {
    /// Duration in seconds, when the frame count is known.
    pub fn duration_seconds(&self) -> Option<f64> {
        let frames = self.frames?;
        if self.sample_rate == 0 {
            return None;
        }
        Some(frames as f64 / f64::from(self.sample_rate))
    }

    /// True when the file must be decoded to PCM before playback (ADR-0008).
    pub fn needs_pcm_cache(&self) -> bool {
        self.format.needs_pcm_cache()
    }
}

/// Everything that can go wrong while opening or reading media.
#[derive(Debug)]
pub enum MediaError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The container/codec is not supported by this build.
    UnsupportedFormat(String),
    /// The container has no audio track.
    NoAudioTrack,
    /// The codec parameters are present but the decoder refused them.
    UnsupportedCodec(String),
    /// R0 handles mono and stereo only.
    UnsupportedChannelCount(u16),
    /// A format without a sample rate cannot be scheduled on the audio clock.
    MissingSampleRate,
    /// The decoder reported corrupt data.
    Decode(String),
    /// The seek target lies outside the file.
    SeekOutOfRange(u64),
    /// The caller asked for a buffer that is not a whole number of stereo frames.
    OddBuffer(usize),
    /// The stream changed mid-file (chained Ogg, for example).
    StreamResetRequired,
    /// The prefetch thread died and left this message.
    Prefetch(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaError::Io(err) => write!(f, "io error: {err}"),
            MediaError::UnsupportedFormat(detail) => write!(f, "unsupported format: {detail}"),
            MediaError::NoAudioTrack => write!(f, "no audio track"),
            MediaError::UnsupportedCodec(detail) => write!(f, "unsupported codec: {detail}"),
            MediaError::UnsupportedChannelCount(count) => {
                write!(f, "unsupported channel count {count} (R0 handles mono and stereo)")
            }
            MediaError::MissingSampleRate => write!(f, "the track does not declare a sample rate"),
            MediaError::Decode(detail) => write!(f, "decode error: {detail}"),
            MediaError::SeekOutOfRange(frame) => write!(f, "seek target {frame} is outside the file"),
            MediaError::OddBuffer(len) => {
                write!(f, "buffer of {len} samples is not a whole number of stereo frames")
            }
            MediaError::StreamResetRequired => {
                write!(f, "the stream requires a reset (chained stream); not supported")
            }
            MediaError::Prefetch(detail) => write!(f, "prefetch: {detail}"),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<std::io::Error> for MediaError {
    fn from(err: std::io::Error) -> Self {
        MediaError::Io(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_policy_is_a_property_of_the_format() {
        assert!(MediaFormat::Mp3.needs_pcm_cache());
        assert!(MediaFormat::OggVorbis.needs_pcm_cache());
        assert!(!MediaFormat::Wav.needs_pcm_cache());
        assert!(!MediaFormat::Flac.needs_pcm_cache());
        assert!(!MediaFormat::Aiff.needs_pcm_cache());
        assert!(MediaFormat::Mp3.is_lossy());
        assert!(!MediaFormat::Flac.is_lossy());
    }

    #[test]
    fn extensions_are_matched_case_insensitively() {
        assert_eq!(MediaFormat::from_extension("MP3"), Some(MediaFormat::Mp3));
        assert_eq!(MediaFormat::from_extension("Flac"), Some(MediaFormat::Flac));
        assert_eq!(MediaFormat::from_extension("oga"), Some(MediaFormat::OggVorbis));
        assert_eq!(MediaFormat::from_extension("aiff"), Some(MediaFormat::Aiff));
        assert_eq!(MediaFormat::from_extension("m4a"), None);
    }

    #[test]
    fn duration_comes_from_frames_and_rate() {
        let info = MediaInfo {
            format: MediaFormat::Flac,
            sample_rate: 44_100,
            channels: 2,
            frames: Some(441_000),
            gapless: None,
        };
        let duration = info.duration_seconds().unwrap_or_default();
        assert!((duration - 10.0).abs() < 1e-9);
        assert!(!info.needs_pcm_cache());
    }
}
