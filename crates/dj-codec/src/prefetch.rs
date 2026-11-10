//! Bounded PCM prefetch between the decoder and the audio callback.
//!
//! The producer decodes in its own thread into a fixed ring; the consumer (the audio thread) only
//! copies frames out. No locks on the read path, no allocation, and no decoder work there — the
//! ring uses atomics over `f32` bits so the whole thing stays in safe Rust (`unsafe_code = "deny"`).
//!
//! Seeks come in two flavours, and the difference matters:
//!
//! * **inside the window** — the cursor moves, the decoder is not touched at all
//!   ([`SeekOutcome::InsideWindow`]);
//! * **outside the window** — the producer re-seeks and refills; this is the only case with a
//!   measurable cost, and the one the 30 ms budget in ADR-0008 talks about.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::cache::{CachedReader, PcmCache};
use crate::decode::StreamDecoder;
use crate::info::{MediaError, MediaInfo};

/// Sentinel for "no seek pending".
const NO_SEEK: u64 = u64::MAX;

/// Window and refill granularity, in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefetchConfig {
    /// How much decoded audio the ring holds per deck.
    pub window_frames: usize,
    /// How many frames the producer decodes per wake-up.
    pub block_frames: usize,
}

impl PrefetchConfig {
    /// Window of `seconds` at `sample_rate`, refilled in 1024-frame blocks.
    pub fn for_sample_rate(sample_rate: u32, seconds: f64) -> Self {
        let window_frames = ((f64::from(sample_rate) * seconds).round() as usize).max(2048);
        Self {
            window_frames,
            block_frames: 1024,
        }
    }
}

impl Default for PrefetchConfig {
    fn default() -> Self {
        Self::for_sample_rate(48_000, 2.0)
    }
}

/// Where a seek landed relative to the prefetched window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekOutcome {
    /// Target was already in the ring: only the cursor moved.
    InsideWindow,
    /// Target was outside the ring and the producer re-filled from there.
    Refilled {
        /// Wall-clock time until the first frame at the new position was available.
        waited: Duration,
    },
    /// Target was outside the ring and the producer was asked to refill, without waiting.
    ///
    /// The audio thread gets this: it must not wait for a decoder. The ring stays empty until the
    /// producer answers, so the next read underruns and the block is rendered as silence.
    Requested,
}

/// Counters and state of the prefetch thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrefetchStats {
    /// Frames currently decoded and ready to read.
    pub buffered_frames: usize,
    /// Calls into the codec made by the producer.
    pub decoder_calls: u64,
    /// Seeks served by the producer (outside-window only).
    pub seeks: u64,
    /// Reads that found the ring empty.
    pub starved_reads: u64,
    /// True once the decoder reached the end of the file.
    pub eof: bool,
    /// First error reported by the producer, if any.
    pub error: Option<String>,
}

struct Ring {
    /// Interleaved stereo samples stored as raw bits.
    data: Box<[AtomicU32]>,
    capacity: usize,
    produced: AtomicU64,
    consumed: AtomicU64,
    seek_request: AtomicU64,
    stopped: AtomicBool,
    decoder_calls: AtomicU64,
    seeks: AtomicU64,
    starved: AtomicU64,
    eof: AtomicBool,
    error: Mutex<Option<String>>,
}

impl Ring {
    fn new(window_frames: usize) -> Self {
        Self {
            data: (0..window_frames * 2)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            capacity: window_frames,
            produced: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            seek_request: AtomicU64::new(NO_SEEK),
            stopped: AtomicBool::new(false),
            decoder_calls: AtomicU64::new(0),
            seeks: AtomicU64::new(0),
            starved: AtomicU64::new(0),
            eof: AtomicBool::new(false),
            error: Mutex::new(None),
        }
    }

    fn record_error(&self, message: String) {
        if let Ok(mut slot) = self.error.lock()
            && slot.is_none()
        {
            *slot = Some(message);
        }
    }

    fn write_frames(&self, start: u64, samples: &[f32]) {
        let frames = samples.len() / 2;
        let mut written = 0usize;
        while written < frames {
            let slot = ((start as usize + written) % self.capacity) * 2;
            let remaining = (frames - written).min(self.capacity - slot / 2);
            for index in 0..remaining {
                self.data[slot + index * 2]
                    .store(samples[(written + index) * 2].to_bits(), Ordering::Relaxed);
                self.data[slot + index * 2 + 1].store(
                    samples[(written + index) * 2 + 1].to_bits(),
                    Ordering::Relaxed,
                );
            }
            written += remaining;
        }
    }

    /// Copy frames out of the ring straight into the caller's buffer.
    ///
    /// No intermediate allocation: this runs on the audio thread.
    fn copy_out(&self, start: u64, out: &mut [f32]) {
        let frames = out.len() / 2;
        let mut read = 0usize;
        while read < frames {
            let slot = ((start as usize + read) % self.capacity) * 2;
            let remaining = (frames - read).min(self.capacity - slot / 2);
            for index in 0..remaining {
                out[(read + index) * 2] =
                    f32::from_bits(self.data[slot + index * 2].load(Ordering::Relaxed));
                out[(read + index) * 2 + 1] =
                    f32::from_bits(self.data[slot + index * 2 + 1].load(Ordering::Relaxed));
            }
            read += remaining;
        }
    }
}

/// Where the producer thread gets its frames from.
///
/// The audio side never learns the difference: both variants fill the same ring. Lossless files
/// stream from the decoder with an index, lossy files read from the PCM cache entry built at load
/// (ADR-0008).
enum Source {
    Stream(StreamDecoder),
    Cached(CachedReader),
}

impl Source {
    fn read_frames(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        match self {
            Source::Stream(decoder) => decoder.read_frames(out),
            Source::Cached(reader) => reader.read_frames(out),
        }
    }

    fn seek(&mut self, frame: u64) -> Result<u64, MediaError> {
        match self {
            Source::Stream(decoder) => decoder.seek(frame),
            Source::Cached(reader) => reader.seek(frame),
        }
    }

    fn decoder_calls(&self) -> u64 {
        match self {
            Source::Stream(decoder) => decoder.stats().decoder_calls,
            // A cached source has no decoder at all; that is the point of the cache.
            Source::Cached(_) => 0,
        }
    }
}

/// A deck-side reader over a decoded window of a media file.
pub struct Prefetcher {
    ring: Arc<Ring>,
    handle: Option<JoinHandle<()>>,
    info: MediaInfo,
    window_frames: usize,
}

impl std::fmt::Debug for Prefetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prefetcher")
            .field("info", &self.info)
            .field("window_frames", &self.window_frames)
            .field("stats", &self.stats())
            .finish()
    }
}

impl Prefetcher {
    /// Open `path` and start decoding into a window.
    ///
    /// The file is opened (and the codec built) synchronously so that a broken file fails here,
    /// not silently in the producer thread.
    pub fn open(path: &Path, config: PrefetchConfig) -> Result<Self, MediaError> {
        if config.window_frames < config.block_frames * 2 {
            return Err(MediaError::Prefetch(
                "window must hold at least two blocks".to_string(),
            ));
        }
        let decoder = StreamDecoder::open(path)?;
        let info = decoder.info().clone();
        Self::spawn(Source::Stream(decoder), info, config)
    }

    /// Open a file through the PCM cache policy (ADR-0008).
    ///
    /// Lossy files are decoded into `cache` once and then read from the entry; lossless files
    /// stream from the decoder exactly as [`Prefetcher::open`] does. The decision comes from the
    /// file alone — never from how the machine happens to be behaving.
    pub fn open_with_cache(
        path: &Path,
        config: PrefetchConfig,
        cache: &mut PcmCache,
    ) -> Result<Self, MediaError> {
        let probe = StreamDecoder::open(path)?;
        let info = probe.info().clone();
        drop(probe);

        let source = if info.format.needs_pcm_cache() && cache.config().enabled {
            cache.prepare(path)?;
            let key = PcmCache::key_for(path, &info, cache.config())?;
            let reader = cache.open_reader(&key)?;
            Source::Cached(reader)
        } else {
            Source::Stream(StreamDecoder::open(path)?)
        };

        Self::spawn(source, info, config)
    }

    fn spawn(
        mut decoder: Source,
        info: MediaInfo,
        config: PrefetchConfig,
    ) -> Result<Self, MediaError> {
        let ring = Arc::new(Ring::new(config.window_frames));

        let producer_ring = Arc::clone(&ring);
        let block_frames = config.block_frames;
        let handle = thread::Builder::new()
            .name(format!("dj-prefetch-{}", info.format.as_str()))
            .spawn(move || {
                let mut scratch = vec![0.0f32; block_frames * 2];
                loop {
                    if producer_ring.stopped.load(Ordering::Acquire) {
                        break;
                    }

                    let request = producer_ring.seek_request.load(Ordering::Acquire);
                    if request != NO_SEEK {
                        match decoder.seek(request) {
                            Ok(landed) => {
                                producer_ring.consumed.store(landed, Ordering::Release);
                                producer_ring.produced.store(landed, Ordering::Release);
                                producer_ring.eof.store(false, Ordering::Release);
                                producer_ring.seeks.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(err) => producer_ring.record_error(err.to_string()),
                        }
                        producer_ring
                            .decoder_calls
                            .store(decoder.decoder_calls(), Ordering::Relaxed);
                        producer_ring.seek_request.store(NO_SEEK, Ordering::Release);
                        continue;
                    }

                    let consumed = producer_ring.consumed.load(Ordering::Acquire);
                    let produced = producer_ring.produced.load(Ordering::Acquire);
                    let filled = produced.saturating_sub(consumed);
                    let free = (producer_ring.capacity as u64).saturating_sub(filled);
                    if free == 0 {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }

                    let want =
                        usize::try_from(free.min(block_frames as u64)).unwrap_or(block_frames);
                    match decoder.read_frames(&mut scratch[..want * 2]) {
                        Ok(0) => {
                            producer_ring.eof.store(true, Ordering::Release);
                            thread::sleep(Duration::from_millis(5));
                        }
                        Ok(frames) => {
                            producer_ring.write_frames(produced, &scratch[..frames * 2]);
                            producer_ring
                                .produced
                                .store(produced + frames as u64, Ordering::Release);
                        }
                        Err(err) => {
                            producer_ring.record_error(err.to_string());
                            break;
                        }
                    }
                    producer_ring
                        .decoder_calls
                        .store(decoder.decoder_calls(), Ordering::Relaxed);
                }
            })
            .map_err(|err| MediaError::Prefetch(err.to_string()))?;

        Ok(Self {
            ring,
            handle: Some(handle),
            info,
            window_frames: config.window_frames,
        })
    }

    /// What the container says about the file.
    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    /// Read up to `out.len() / 2` stereo frames from the window. Returns frames written.
    pub fn read_frames(&mut self, out: &mut [f32]) -> usize {
        let want = out.len() / 2;
        let start = self.ring.consumed.load(Ordering::Acquire);
        let end = self.ring.produced.load(Ordering::Acquire);
        let available = usize::try_from(end.saturating_sub(start)).unwrap_or(0);
        let take = want.min(available);
        if take == 0 {
            self.ring.starved.fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        self.ring.copy_out(start, &mut out[..take * 2]);
        self.ring
            .consumed
            .store(start + take as u64, Ordering::Release);
        take
    }

    /// Ask the producer for `frame` without waiting for it.
    ///
    /// This is the audio-thread seek. [`Prefetcher::seek`] answers the same question but spins
    /// until the producer replies — inside a callback that spin is a stall of milliseconds, which
    /// is exactly what the RT contract forbids. A request that leaves the window therefore returns
    /// immediately and the deck plays silence (and counts an underrun) until the ring refills.
    pub fn request_seek(&mut self, frame: u64) -> Result<SeekOutcome, MediaError> {
        let start = self.ring.consumed.load(Ordering::Acquire);
        let end = self.ring.produced.load(Ordering::Acquire);
        if frame >= start && frame < end {
            self.ring.consumed.store(frame, Ordering::Release);
            return Ok(SeekOutcome::InsideWindow);
        }
        self.ring.seek_request.store(frame, Ordering::Release);
        // The window holds the place the play head is leaving. Keeping it would hand the deck the
        // material the jump was away from — a loop moved by a beat played the beat outside itself, and
        // the same window would be read as if it were the new position's. Dropping it makes the reader
        // wait for the producer, which is silence for a block and then the right material.
        self.ring.consumed.store(end, Ordering::Release);
        Ok(SeekOutcome::Requested)
    }

    /// Move the read cursor to `frame`, waiting for the producer when the target is outside the
    /// window. Offline rendering uses this for determinism; the audio callback must not.
    pub fn seek(&mut self, frame: u64) -> Result<SeekOutcome, MediaError> {
        if let SeekOutcome::InsideWindow = self.request_seek(frame)? {
            return Ok(SeekOutcome::InsideWindow);
        }

        let requested = Instant::now();
        self.ring.seek_request.store(frame, Ordering::Release);
        let deadline = requested + Duration::from_secs(2);
        while self.ring.seek_request.load(Ordering::Acquire) != NO_SEEK {
            if let Some(message) = self.error() {
                return Err(MediaError::Prefetch(message));
            }
            if Instant::now() > deadline {
                return Err(MediaError::Prefetch(
                    "the producer did not serve the seek in time".to_string(),
                ));
            }
            thread::sleep(Duration::from_micros(200));
        }
        Ok(SeekOutcome::Refilled {
            waited: requested.elapsed(),
        })
    }

    /// Wait until at least `frames` are buffered, or the file ends. Returns frames available.
    pub fn wait_ready(&mut self, frames: usize, timeout: Duration) -> Result<usize, MediaError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.error() {
                return Err(MediaError::Prefetch(message));
            }
            let start = self.ring.consumed.load(Ordering::Acquire);
            let end = self.ring.produced.load(Ordering::Acquire);
            let available = usize::try_from(end.saturating_sub(start)).unwrap_or(0);
            if available >= frames || self.ring.eof.load(Ordering::Acquire) {
                return Ok(available);
            }
            if Instant::now() > deadline {
                return Ok(available);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Counters and current fill level.
    pub fn stats(&self) -> PrefetchStats {
        let start = self.ring.consumed.load(Ordering::Acquire);
        let end = self.ring.produced.load(Ordering::Acquire);
        PrefetchStats {
            buffered_frames: usize::try_from(end.saturating_sub(start)).unwrap_or(0),
            decoder_calls: self.ring.decoder_calls.load(Ordering::Relaxed),
            seeks: self.ring.seeks.load(Ordering::Relaxed),
            starved_reads: self.ring.starved.load(Ordering::Relaxed),
            eof: self.ring.eof.load(Ordering::Acquire),
            error: self.error(),
        }
    }

    /// Window size in frames.
    pub fn window_frames(&self) -> usize {
        self.window_frames
    }

    fn error(&self) -> Option<String> {
        self.ring.error.lock().ok().and_then(|slot| slot.clone())
    }
}

impl Drop for Prefetcher {
    fn drop(&mut self) {
        self.ring.stopped.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
