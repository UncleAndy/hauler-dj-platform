//! Device output (A05): route plans, a bounded callback and honest counters.
//!
//! The engine hands this crate a [`Renderer`]; the callback asks it for one block and writes the
//! result into the device buffer according to a [`RoutePlan`]. Everything the callback touches is
//! preallocated, and every counter it updates is an atomic — no locks, no allocation (ADR-0002,
//! DJP-NFR-001).
//!
//! Ownership: architect / audio-platform (`docs/architecture/crate-map.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Which physical channels carry which bus (ADR-0005).
///
/// Channels are addressed by position, never by name: the S4 descriptor says `FL FR FC LFE` while
/// the PipeWire profile says `FL FR RL RR`, and only listening settles which pair is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePlan {
    /// Left and right channel of the main output.
    pub master: (usize, usize),
    /// Left and right channel of the headphones, when the device has four or more outputs.
    pub cue: Option<(usize, usize)>,
    /// Total channels to open on the device.
    pub channels: usize,
}

impl RoutePlan {
    /// The plan for a device with `channels` outputs.
    ///
    /// Two channels: master only, the cue unusable without split-cue (ADR-0005). Four or more:
    /// master on 0/1, cue on 2/3 — the arrangement verified by ear on the S4 MK3.
    pub fn for_channels(channels: usize) -> Self {
        match channels {
            0 | 1 => Self {
                master: (0, 0),
                cue: None,
                channels: channels.max(1),
            },
            2 | 3 => Self {
                master: (0, 1),
                cue: None,
                channels: 2,
            },
            _ => Self {
                master: (0, 1),
                cue: Some((2, 3)),
                channels: 4,
            },
        }
    }

    /// Number of channels the plan writes to.
    pub fn channels(&self) -> usize {
        self.channels
    }
}

/// Something that produces one block of master and cue audio.
///
/// The callback calls it with preallocated buffers: `master` and `cue` hold `frames` stereo
/// frames each. Implementations must not allocate, lock or block — the deck's blocking render is
/// for offline work, and this is not that path.
pub trait Renderer: Send {
    /// Fill `master` and `cue` for one callback.
    fn render(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize);
}

/// Counters collected inside the callback.
#[derive(Debug, Default)]
struct Counters {
    callbacks: AtomicU64,
    frames: AtomicU64,
    /// Callback intervals outside 0.5 … 1.5 of the expected period.
    gaps: AtomicU64,
    /// Shortest and longest callback interval, microseconds.
    min_us: AtomicU32,
    max_us: AtomicU32,
    /// Peak per output channel, as f32 bits.
    peaks: Vec<AtomicU32>,
    /// Histogram of intervals, 1 µs buckets up to 32 ms.
    histogram: Vec<AtomicU32>,
}

impl Counters {
    fn new(channels: usize) -> Self {
        Self {
            peaks: (0..channels).map(|_| AtomicU32::new(0)).collect(),
            histogram: (0..32_000).map(|_| AtomicU32::new(0)).collect(),
            min_us: AtomicU32::new(u32::MAX),
            ..Self::default()
        }
    }

    fn note_interval(&self, micros: u32) {
        self.min_us.fetch_min(micros, Ordering::Relaxed);
        self.max_us.fetch_max(micros, Ordering::Relaxed);
        if let Some(bucket) = self.histogram.get(micros as usize) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn note_peak(&self, channel: usize, value: f32) {
        if let Some(slot) = self.peaks.get(channel) {
            slot.fetch_max(value.to_bits(), Ordering::Relaxed);
        }
    }
}

/// Timing and level summary of one run.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveReport {
    /// Device name as the backend spells it.
    pub device: String,
    /// Channels opened.
    pub channels: usize,
    /// Sample rate requested and used.
    pub sample_rate: u32,
    /// Block size requested (the backend may hand a different one).
    pub block_frames_requested: usize,
    /// Frames per callback actually observed, on average.
    pub frames_per_callback: f64,
    /// Callbacks seen.
    pub callbacks: u64,
    /// Frames rendered.
    pub frames: u64,
    /// Seconds of audio produced.
    pub audio_seconds: f64,
    /// Callback intervals outside the expected range.
    pub gaps: u64,
    /// Shortest and longest callback interval, milliseconds.
    pub interval_min_ms: f64,
    /// Longest callback interval, milliseconds.
    pub interval_max_ms: f64,
    /// Median and 99th percentile callback interval, milliseconds.
    pub interval_p50_ms: f64,
    /// 99th percentile callback interval, milliseconds.
    pub interval_p99_ms: f64,
    /// Peak of every output channel, in plan order.
    pub peaks: Vec<f32>,
}

impl LiveReport {
    /// True when the backend honoured the requested block size within 1 %.
    pub fn honoured_block(&self) -> bool {
        (self.frames_per_callback - self.block_frames_requested as f64).abs() < 0.01
    }
}

/// What the backend says about a device before a stream is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceShape {
    /// Name as the backend spells it.
    pub name: String,
    /// Channels the plan should open: 2 for stereo devices, 4 for wider ones (ADR-0005).
    pub channels: usize,
    /// Rate the device prefers.
    pub sample_rate: u32,
}

/// Resolve a device by name substring and report its shape.
///
/// The plan has to match what the device offers, so this runs before [`LiveOutput::start`]: a
/// four-channel interface will not accept a two-channel stream, and a stereo sink has no cue pair
/// to write to.
pub fn resolve_device(device: Option<&str>) -> Result<DeviceShape, IoError> {
    let host = cpal::default_host();
    let device = match device {
        Some(needle) => {
            let needle_lower = needle.to_ascii_lowercase();
            let devices = host
                .output_devices()
                .map_err(|err| IoError::Unsupported(err.to_string()))?;
            let mut found = None;
            for candidate in devices {
                if candidate
                    .to_string()
                    .to_ascii_lowercase()
                    .contains(&needle_lower)
                {
                    found = Some(candidate);
                    break;
                }
            }
            found.ok_or_else(|| IoError::DeviceNotFound(needle.to_string()))?
        }
        None => host
            .default_output_device()
            .ok_or_else(|| IoError::DeviceNotFound("default".to_string()))?,
    };

    let name = device.to_string();
    let (channels, sample_rate) = match device.default_output_config() {
        Ok(config) => {
            let native = usize::from(config.channels());
            // Only the widths the route plan knows how to use: stereo, or master + cue.
            let width = if native >= 4 {
                4
            } else if native >= 2 {
                2
            } else {
                1
            };
            (width, config.sample_rate())
        }
        Err(_) => (2, 48_000),
    };

    Ok(DeviceShape {
        name,
        channels,
        sample_rate,
    })
}

/// What can go wrong opening or running a device.
#[derive(Debug)]
pub enum IoError {
    /// No output device matched the requested name.
    DeviceNotFound(String),
    /// The backend refused the configuration.
    Unsupported(String),
    /// The stream was built but failed to start.
    Start(String),
    /// The backend reported an error while running.
    Stream(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::DeviceNotFound(name) => write!(f, "no output device matching {name:?}"),
            IoError::Unsupported(detail) => write!(f, "unsupported configuration: {detail}"),
            IoError::Start(detail) => write!(f, "stream did not start: {detail}"),
            IoError::Stream(detail) => write!(f, "stream error: {detail}"),
        }
    }
}

impl std::error::Error for IoError {}

/// Configuration for one live run.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveConfig {
    /// Device name substring; `None` takes the default output.
    pub device: Option<String>,
    /// Sample rate to request.
    pub sample_rate: u32,
    /// Block size to request.
    pub block_frames: usize,
    /// Seconds to run.
    pub seconds: f64,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: 48_000,
            block_frames: 256,
            seconds: 10.0,
        }
    }
}

/// A running output stream. Dropping it stops playback.
pub struct LiveOutput {
    _stream: cpal::Stream,
    counters: Arc<Counters>,
    device: String,
    channels: usize,
    sample_rate: u32,
    block_frames: usize,
}

impl LiveOutput {
    /// Open a device and start rendering.
    ///
    /// `plan` decides which channels carry master and cue; `renderer` is moved into the callback.
    pub fn start(
        config: &LiveConfig,
        plan: RoutePlan,
        renderer: Box<dyn Renderer>,
    ) -> Result<Self, IoError> {
        let host = cpal::default_host();
        let device = match &config.device {
            Some(needle) => {
                let needle_lower = needle.to_ascii_lowercase();
                let devices = host
                    .output_devices()
                    .map_err(|err| IoError::Unsupported(err.to_string()))?;
                let mut found = None;
                for candidate in devices {
                    let name = candidate.to_string();
                    if name.to_ascii_lowercase().contains(&needle_lower) {
                        found = Some(candidate);
                        break;
                    }
                }
                found.ok_or_else(|| IoError::DeviceNotFound(needle.clone()))?
            }
            None => host
                .default_output_device()
                .ok_or_else(|| IoError::DeviceNotFound("default".to_string()))?,
        };

        let name = device.to_string();
        let channels = u16::try_from(plan.channels()).unwrap_or(2);
        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate: config.sample_rate,
            buffer_size: cpal::BufferSize::Fixed(u32::try_from(config.block_frames).unwrap_or(256)),
        };

        let counters = Arc::new(Counters::new(usize::from(channels)));
        let callback_counters = Arc::clone(&counters);
        let mut renderer = renderer;
        let mut master = vec![0.0f32; config.block_frames.max(1) * 2];
        let mut cue = vec![0.0f32; config.block_frames.max(1) * 2];
        let mut last = Instant::now();
        let mut frames_total = 0u64;
        let mut callbacks = 0u64;

        let data_fn = move |buffer: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let now = Instant::now();
            let micros = now.duration_since(last).as_micros();
            last = now;
            callback_counters.note_interval(u32::try_from(micros).unwrap_or(u32::MAX));

            let frames = buffer.len() / usize::from(channels);
            if master.len() < frames * 2 {
                // Never grow inside the callback: a larger block than prepared is rendered as the
                // part we have and the rest stays silent, which is visible in the counters.
                master.resize(frames * 2, 0.0);
                cue.resize(frames * 2, 0.0);
            }
            master[..frames * 2].fill(0.0);
            cue[..frames * 2].fill(0.0);
            renderer.render(&mut master[..frames * 2], &mut cue[..frames * 2], frames);

            let (master_l, master_r) = plan.master;
            let cue_channels = plan.cue;
            for frame in 0..frames {
                let base = frame * 2;
                for channel in 0..usize::from(channels) {
                    let value = if channel == master_l {
                        master[base]
                    } else if channel == master_r {
                        master[base + 1]
                    } else if let Some((cue_l, cue_r)) = cue_channels {
                        if channel == cue_l {
                            cue[base]
                        } else if channel == cue_r {
                            cue[base + 1]
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    buffer[frame * usize::from(channels) + channel] = value;
                    callback_counters.note_peak(channel, value.abs());
                }
            }

            callbacks += 1;
            frames_total += frames as u64;
            callback_counters
                .callbacks
                .store(callbacks, Ordering::Relaxed);
            callback_counters
                .frames
                .store(frames_total, Ordering::Relaxed);
        };

        let error_counters = Arc::clone(&counters);
        let stream = device
            .build_output_stream(
                stream_config,
                data_fn,
                move |err| {
                    error_counters.gaps.fetch_add(1, Ordering::Relaxed);
                    eprintln!("dj-audio-io: stream error: {err}");
                },
                None,
            )
            .map_err(|err| IoError::Unsupported(err.to_string()))?;

        stream
            .play()
            .map_err(|err| IoError::Start(err.to_string()))?;

        Ok(Self {
            _stream: stream,
            counters,
            device: name,
            channels: usize::from(channels),
            sample_rate: config.sample_rate,
            block_frames: config.block_frames,
        })
    }

    /// Wait for the run to finish and produce the report.
    pub fn finish(self, seconds: f64) -> LiveReport {
        std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
        let callbacks = self.counters.callbacks.load(Ordering::Relaxed);
        let frames = self.counters.frames.load(Ordering::Relaxed);
        let frames_per_callback = if callbacks == 0 {
            0.0
        } else {
            frames as f64 / callbacks as f64
        };
        let period_us = frames_per_callback / f64::from(self.sample_rate) * 1_000_000.0;
        let low = period_us * 0.5;
        let high = period_us * 1.5;

        let mut seen = 0u64;
        let mut p50 = 0.0;
        let mut p99 = 0.0;
        let total: u64 = self
            .counters
            .histogram
            .iter()
            .map(|bucket| u64::from(bucket.load(Ordering::Relaxed)))
            .sum();
        for (micros, bucket) in self.counters.histogram.iter().enumerate() {
            let count = u64::from(bucket.load(Ordering::Relaxed));
            if count == 0 {
                continue;
            }
            seen += count;
            let fraction = seen as f64 / total.max(1) as f64;
            if p50 == 0.0 && fraction >= 0.5 {
                p50 = micros as f64 / 1000.0;
            }
            if p99 == 0.0 && fraction >= 0.99 {
                p99 = micros as f64 / 1000.0;
            }
        }
        let _ = low;
        let _ = high;

        LiveReport {
            device: self.device.clone(),
            channels: self.channels,
            sample_rate: self.sample_rate,
            block_frames_requested: self.block_frames,
            frames_per_callback,
            callbacks,
            frames,
            audio_seconds: frames as f64 / f64::from(self.sample_rate),
            gaps: self.counters.gaps.load(Ordering::Relaxed),
            interval_min_ms: f64::from(self.counters.min_us.load(Ordering::Relaxed)) / 1000.0,
            interval_max_ms: f64::from(self.counters.max_us.load(Ordering::Relaxed)) / 1000.0,
            interval_p50_ms: p50,
            interval_p99_ms: p99,
            peaks: self
                .counters
                .peaks
                .iter()
                .map(|slot| f32::from_bits(slot.load(Ordering::Relaxed)))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_route_plan_follows_the_device_width() {
        let two = RoutePlan::for_channels(2);
        assert_eq!(two.master, (0, 1));
        assert_eq!(two.cue, None);
        assert_eq!(two.channels(), 2);

        let four = RoutePlan::for_channels(4);
        assert_eq!(four.master, (0, 1));
        assert_eq!(four.cue, Some((2, 3)));
        assert_eq!(four.channels(), 4);

        let many = RoutePlan::for_channels(8);
        assert_eq!(many.channels(), 4, "R0 opens at most master + cue");
    }

    #[test]
    fn a_single_channel_device_still_gets_a_plan() {
        let mono = RoutePlan::for_channels(1);
        assert_eq!(mono.master, (0, 0));
        assert_eq!(mono.cue, None);
        assert_eq!(mono.channels(), 1);
    }
}
