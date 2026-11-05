//! Streaming decoder: one file, interleaved stereo `f32`, sample-accurate seek.
//!
//! The decoder never runs on the audio thread — [`crate::prefetch`] owns that boundary. Here we
//! only need correctness and honest counters: how many times the codec was called, how many times
//! a seek had to touch the file, and how many packets were corrupt.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{TimeBase, Timestamp};

use crate::info::{GaplessInfo, MediaError, MediaFormat, MediaInfo};

/// How many frames a single decoded packet produced, for the counters and for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStats {
    /// Packets passed to the codec.
    pub decoder_calls: u64,
    /// Seeks issued to the format reader.
    pub seeks: u64,
    /// Packets the codec rejected as corrupt and that were skipped.
    pub corrupt_packets: u64,
    /// Frames dropped after a seek to land exactly on the requested sample.
    pub discarded_frames: u64,
}

/// A decoder over one file. Output is always interleaved stereo `f32`.
pub struct StreamDecoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    info: MediaInfo,
    position: u64,
    /// Decoded frames waiting to be consumed, interleaved stereo.
    pending: Vec<f32>,
    /// Frames already consumed from `pending`.
    pending_frame: usize,
    /// Frames available in `pending`.
    pending_frames: usize,
    /// Scratch buffer reused between packets (never inside the audio callback).
    scratch: Vec<f32>,
    stats: DecodeStats,
    eof: bool,
}

impl std::fmt::Debug for StreamDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamDecoder")
            .field("info", &self.info)
            .field("position", &self.position)
            .field("stats", &self.stats)
            .field("eof", &self.eof)
            .finish()
    }
}

impl StreamDecoder {
    /// Open a file and prepare a decoder for its default audio track.
    pub fn open(path: &Path) -> Result<Self, MediaError> {
        let file = File::open(path)?;
        let source = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(extension);
        }

        let reader = symphonia::default::get_probe()
            .probe(
                &hint,
                source,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|err| MediaError::UnsupportedFormat(err.to_string()))?;

        let format = MediaFormat::from_extension(
            path.extension().and_then(|ext| ext.to_str()).unwrap_or(""),
        )
        .unwrap_or(MediaFormat::Other);

        let (decoder, info) = {
            let track = reader
                .default_track(TrackType::Audio)
                .ok_or(MediaError::NoAudioTrack)?;
            let params = track
                .codec_params
                .as_ref()
                .ok_or(MediaError::NoAudioTrack)?;
            let audio = params.audio().ok_or(MediaError::NoAudioTrack)?;
            let sample_rate = audio.sample_rate.ok_or(MediaError::MissingSampleRate)?;
            let channels = audio
                .channels
                .as_ref()
                .ok_or(MediaError::NoAudioTrack)?
                .count()
                .try_into()
                .unwrap_or(u16::MAX);
            if channels == 0 || channels > 2 {
                return Err(MediaError::UnsupportedChannelCount(channels));
            }

            // Gapless trimming is on by default upstream; we ask for it explicitly because the
            // whole MP3 policy (ADR-0008) leans on it.
            let decoder = symphonia::default::get_codecs()
                .make_audio_decoder(audio, &AudioDecoderOptions::default().gapless(true))
                .map_err(|err| MediaError::UnsupportedCodec(err.to_string()))?;

            let gapless = match (track.delay, track.padding) {
                (Some(delay), Some(padding)) => Some(GaplessInfo { delay, padding }),
                (Some(delay), None) => Some(GaplessInfo { delay, padding: 0 }),
                (None, Some(padding)) => Some(GaplessInfo { delay: 0, padding }),
                (None, None) => None,
            };

            (
                decoder,
                MediaInfo {
                    format,
                    sample_rate,
                    channels,
                    frames: track.num_frames,
                    gapless,
                },
            )
        };

        let track_id = reader
            .default_track(TrackType::Audio)
            .map(|track| track.id)
            .ok_or(MediaError::NoAudioTrack)?;
        let time_base = reader
            .default_track(TrackType::Audio)
            .and_then(|track| track.time_base);

        Ok(Self {
            reader,
            decoder,
            track_id,
            time_base,
            info,
            position: 0,
            pending: Vec::new(),
            pending_frame: 0,
            pending_frames: 0,
            scratch: Vec::new(),
            stats: DecodeStats::default(),
            eof: false,
        })
    }

    /// What the container declares about the file.
    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    /// Frame position the next read will return.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Counters, for tests and for the load report.
    pub fn stats(&self) -> DecodeStats {
        self.stats
    }

    /// Sample rate of the source material.
    pub fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    /// Read up to `out.len() / 2` stereo frames. Returns frames written; 0 means end of file.
    pub fn read_frames(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        if !out.len().is_multiple_of(2) {
            return Err(MediaError::OddBuffer(out.len()));
        }
        let want = out.len() / 2;
        let mut produced = 0usize;

        while produced < want {
            if self.pending_frame < self.pending_frames {
                let available = self.pending_frames - self.pending_frame;
                let take = available.min(want - produced);
                let start = self.pending_frame * 2;
                out[produced * 2..(produced + take) * 2]
                    .copy_from_slice(&self.pending[start..start + take * 2]);
                self.pending_frame += take;
                self.position += take as u64;
                produced += take;
                continue;
            }
            if self.eof || !self.decode_next_packet()? {
                self.eof = true;
                break;
            }
        }
        Ok(produced)
    }

    /// Seek to `frame`. Returns the position the decoder actually landed on.
    ///
    /// The format reader seeks to the nearest point *before* the target; the remaining frames are
    /// discarded here, so the next read starts exactly at `frame` for formats with exact
    /// timestamps (WAV, FLAC) and within one codec frame for MP3.
    pub fn seek(&mut self, frame: u64) -> Result<u64, MediaError> {
        if let Some(total) = self.info.frames
            && frame >= total
        {
            return Err(MediaError::SeekOutOfRange(frame));
        }

        let target = Timestamp::new(i64::try_from(frame).unwrap_or(i64::MAX));
        let seeked = self
            .reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts: target,
                    track_id: self.track_id,
                },
            )
            .map_err(|err| MediaError::Decode(err.to_string()))?;

        self.stats.seeks += 1;
        self.pending_frame = 0;
        self.pending_frames = 0;
        self.pending.clear();
        self.decoder.reset();
        self.eof = false;

        let landed = self.frame_of(seeked.actual_ts).unwrap_or(frame);
        self.position = landed;
        if landed < frame {
            let to_drop = usize::try_from(frame - landed).unwrap_or(usize::MAX);
            self.discard(to_drop)?;
        }
        Ok(self.position)
    }

    /// Convert a stream timestamp into a frame index.
    fn frame_of(&self, ts: Timestamp) -> Option<u64> {
        match self.time_base {
            Some(time_base) => {
                let seconds = time_base.calc_time(ts)?.as_secs_f64();
                if seconds < 0.0 {
                    return Some(0);
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let frames = (seconds * f64::from(self.info.sample_rate)).round() as u64;
                Some(frames)
            }
            None if ts.get() >= 0 => u64::try_from(ts.get()).ok(),
            None => Some(0),
        }
    }

    /// Read and throw away `frames` frames, tracking how many were wasted.
    fn discard(&mut self, frames: usize) -> Result<(), MediaError> {
        let mut left = frames;
        let mut buffer = vec![0.0f32; 2048];
        while left > 0 {
            let want = (left * 2).min(buffer.len());
            let chunk = buffer.get_mut(..want).ok_or(MediaError::OddBuffer(want))?;
            let read = self.read_frames(chunk)?;
            if read == 0 {
                break;
            }
            left = left.saturating_sub(read);
            self.stats.discarded_frames += read as u64;
        }
        Ok(())
    }

    /// Decode the next packet belonging to the selected track into `pending`.
    fn decode_next_packet(&mut self) -> Result<bool, MediaError> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(false),
                Err(SymphoniaError::IoError(err))
                    if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(false);
                }
                Err(SymphoniaError::ResetRequired) => return Err(MediaError::StreamResetRequired),
                Err(err) => return Err(MediaError::Decode(err.to_string())),
            };

            if packet.track_id != self.track_id {
                continue;
            }

            self.stats.decoder_calls += 1;
            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    let samples = buffer.samples_interleaved();
                    self.scratch.clear();
                    self.scratch.resize(samples, 0.0);
                    buffer.copy_to_slice_interleaved(&mut self.scratch);

                    self.pending.clear();
                    if self.info.channels == 2 {
                        self.pending.extend_from_slice(&self.scratch);
                    } else {
                        // Mono sources are duplicated into both output channels here, once.
                        self.pending.reserve(self.scratch.len() * 2);
                        for sample in &self.scratch {
                            self.pending.push(*sample);
                            self.pending.push(*sample);
                        }
                    }
                    self.pending_frame = 0;
                    self.pending_frames = self.pending.len() / 2;
                    if self.pending_frames == 0 {
                        continue;
                    }
                    return Ok(true);
                }
                Err(SymphoniaError::DecodeError(_)) => {
                    self.stats.corrupt_packets += 1;
                    continue;
                }
                Err(SymphoniaError::IoError(err)) => return Err(MediaError::Io(err)),
                Err(err) => return Err(MediaError::Decode(err.to_string())),
            }
        }
    }
}
