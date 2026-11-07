//! Deck: transport, loop and hotcues over a decoded source (A03).
//!
//! A deck never decodes — it reads stereo frames from a [`DeckSource`] and decides *which* frames
//! to read. That keeps the loop and hotcue logic testable without files, and keeps the cache
//! policy where it belongs (in `dj-codec`, ADR-0008).
//!
//! The output contract is at deck level: interleaved stereo `f32`, exactly the buffer it is given,
//! with the loop wrapped at the sample the loop ends on (DJP-DECK-003/004).
//!
//! Ownership: audio-core (`docs/architecture/crate-map.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use dj_codec::Prefetcher;
use dj_domain::{
    DeckId, DeckLoad, DeckSnapshot, EqGains, FrameRange, HOTCUE_SLOTS, Levels, SourceFrame,
    TrackMeta,
};
use dj_dsp::ChannelEq;

/// Sample rate the equaliser's table is built for.
///
/// Files in this project are required to be 48 kHz (no resampling yet), and the device runs at the
/// same rate, so one table serves every deck. A 44.1 kHz source would need its own table; that
/// arrives with the rest of the 44.1 kHz work in A05, and is listed as a limitation of A14.
pub const EQ_SAMPLE_RATE: f64 = 48_000.0;

/// Where a deck reads frames from. Implemented by the prefetcher; tests provide their own.
pub trait DeckSource: Send {
    /// Sample rate of the material.
    fn sample_rate(&self) -> u32;
    /// Playable frames, when known.
    fn frames(&self) -> Option<u64>;
    /// Read up to `out.len() / 2` stereo frames; returns frames written, 0 at the end.
    fn read(&mut self, out: &mut [f32]) -> Result<usize, String>;
    /// Seek to a source frame; returns the frame the source landed on.
    ///
    /// This may wait for the source to be ready. Offline rendering needs that; the audio callback
    /// does not — see [`DeckSource::request_seek`].
    fn seek(&mut self, frame: u64) -> Result<u64, String>;

    /// Ask for a seek without waiting for it.
    ///
    /// Sources that refill asynchronously (the prefetcher) return at once and leave the data to
    /// arrive later; the deck then plays silence for the blocks it did not get. Sources that read
    /// straight through fall back to [`DeckSource::seek`], where waiting costs nothing.
    fn request_seek(&mut self, frame: u64) -> Result<u64, String> {
        self.seek(frame)
    }

    /// Wait until at least `frames` are available, returning how many actually are.
    ///
    /// Sources that cannot block (a file read straight through) return `frames`. The prefetcher
    /// waits on its ring. Rendering is deterministic only if a momentary empty ring is waited out
    /// instead of being rendered as silence — see `Deck::render`.
    fn wait_for(&mut self, frames: usize) -> Result<usize, String> {
        Ok(frames)
    }
}

impl DeckSource for Prefetcher {
    fn sample_rate(&self) -> u32 {
        self.info().sample_rate
    }

    fn frames(&self) -> Option<u64> {
        self.info().frames
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        Ok(self.read_frames(out))
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        Prefetcher::seek(self, frame)
            .map(|_| frame)
            .map_err(|err| err.to_string())
    }

    fn request_seek(&mut self, frame: u64) -> Result<u64, String> {
        self.request_seek(frame)
            .map(|_| frame)
            .map_err(|err| err.to_string())
    }

    fn wait_for(&mut self, frames: usize) -> Result<usize, String> {
        self.wait_ready(frames, std::time::Duration::from_secs(5))
            .map_err(|err| err.to_string())
    }
}

/// Counters a deck keeps for reports and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeckStats {
    /// Reads that came back empty while the transport was running.
    pub underruns: u64,
    /// Times the loop wrapped.
    pub loop_wraps: u64,
    /// Hotcue jumps.
    pub hotcue_jumps: u64,
}

/// One deck.
/// How loops are played back.
///
/// A loop that wraps by seeking gives up one block of silence at the seam (ADR-0009): the start of
/// the loop lies behind the read cursor, so it has to come from the decoder again. Capturing the
/// loop's audio while it first plays through turns the seam into a plain copy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopConfig {
    /// Longest loop that is captured into memory, in seconds. Zero disables capturing.
    ///
    /// A setting, not a constant: the cost is memory (`seconds × 2 × 4 B` per deck, so the default
    /// 30 s is ~11.5 MB) and the benefit stops mattering once the loop is long enough that the seam
    /// falls where nobody plays.
    pub buffer_max_seconds: f64,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            buffer_max_seconds: 30.0,
        }
    }
}

impl LoopConfig {
    /// Longest captured loop in frames at `sample_rate`.
    pub fn max_frames(&self, sample_rate: u32) -> u64 {
        if self.buffer_max_seconds <= 0.0 {
            return 0;
        }
        (self.buffer_max_seconds * f64::from(sample_rate)) as u64
    }
}

/// Audio of one loop, captured while it plays through, so the seam costs a copy instead of a seek.
#[derive(Debug, Clone)]
struct LoopBuffer {
    /// Loop start in source frames; the buffer covers `start .. start + frames`.
    start: u64,
    /// Interleaved stereo audio, `frames * 2` samples.
    data: Vec<f32>,
    /// Frames captured so far, from the start of the loop.
    captured: u64,
}

impl LoopBuffer {
    fn frames(&self) -> u64 {
        (self.data.len() / 2) as u64
    }

    fn is_complete(&self) -> bool {
        self.captured >= self.frames()
    }
}

pub struct Deck {
    id: DeckId,
    source: Option<Box<dyn DeckSource>>,
    load: DeckLoad,
    playing: bool,
    position: u64,
    gain: f32,
    tempo_ratio: f64,
    loop_region: Option<FrameRange>,
    hotcues: [Option<u64>; HOTCUE_SLOTS],
    track: Option<TrackMeta>,
    levels: Levels,
    stats: DeckStats,
    /// Whether rendering may wait for the prefetch window.
    blocking: bool,
    /// Loop capture policy (ADR-0009).
    loop_config: LoopConfig,
    /// Captured loop, when one is being played back from memory.
    loop_buffer: Option<LoopBuffer>,
    /// True while the playhead reads the captured loop instead of the source.
    reading_loop_buffer: bool,
    /// The channel's three-band EQ, applied after the trim and before the mixer (A14).
    eq: ChannelEq,
}

impl std::fmt::Debug for Deck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deck")
            .field("id", &self.id)
            .field("load", &self.load)
            .field("playing", &self.playing)
            .field("position", &self.position)
            .field("stats", &self.stats)
            .finish()
    }
}

impl Deck {
    /// An empty deck.
    pub fn new(id: DeckId) -> Self {
        Self {
            id,
            source: None,
            load: DeckLoad::Empty,
            playing: false,
            position: 0,
            gain: 1.0,
            tempo_ratio: 1.0,
            loop_region: None,
            hotcues: [None; HOTCUE_SLOTS],
            track: None,
            levels: Levels::SILENT,
            stats: DeckStats::default(),
            blocking: true,
            loop_config: LoopConfig::default(),
            loop_buffer: None,
            reading_loop_buffer: false,
            eq: ChannelEq::new(EQ_SAMPLE_RATE),
        }
    }

    /// Which deck this is.
    pub fn id(&self) -> DeckId {
        self.id
    }

    /// Source position of the next frame to render.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Preparation state.
    /// True while a source is attached: what `eject` clears and what makes a deck playable.
    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }

    pub fn load_state(&self) -> &DeckLoad {
        &self.load
    }

    /// Allow or forbid waiting for the prefetch window while rendering.
    ///
    /// Offline rendering waits (otherwise the same scenario would produce different bytes
    /// depending on thread timing). The audio callback must not: there a short ring is an
    /// underrun to count and fill with silence, never a stall.
    pub fn set_blocking(&mut self, blocking: bool) {
        self.blocking = blocking;
    }

    /// True while the transport runs.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Counters.
    pub fn stats(&self) -> DeckStats {
        self.stats
    }

    /// Attach a prepared source (the loader has already waited for `Ready`).
    pub fn attach(&mut self, source: Box<dyn DeckSource>, track: Option<TrackMeta>) {
        self.source = Some(source);
        self.track = track;
        self.load = DeckLoad::Ready;
        self.position = 0;
        self.playing = false;
        self.loop_region = None;
        self.levels = Levels::SILENT;
        // A new track starts with an empty filter history: the tail of the last one is not part of
        // this one, and a stale state produces a click on the first block.
        self.eq.reset();
    }

    /// Take the source out, leaving the deck empty and playable-ready.
    ///
    /// The caller owns the old source, which matters on the audio path: freeing a decoder's
    /// buffers there can take longer than a block, so the engine parks it instead.
    pub fn take_source(&mut self) -> Option<Box<dyn DeckSource>> {
        self.source.take()
    }

    /// Remove the source and return the deck to `Empty`.
    pub fn eject(&mut self) {
        self.source = None;
        self.track = None;
        self.playing = false;
        self.position = 0;
        self.loop_region = None;
        self.loop_buffer = None;
        self.reading_loop_buffer = false;
        self.levels = Levels::SILENT;
        self.load = DeckLoad::Empty;
        self.eq.reset();
    }

    /// Record a failed load; the deck stays empty but the reason is visible.
    pub fn fail(&mut self, message: impl Into<String>) {
        self.source = None;
        self.track = None;
        self.playing = false;
        self.load = DeckLoad::Failed {
            message: message.into(),
        };
    }

    /// Report loading progress (0.0 … 1.0).
    pub fn set_progress(&mut self, progress: f32) {
        if self.source.is_none() {
            self.load = DeckLoad::Loading {
                progress: progress.clamp(0.0, 1.0),
            };
        }
    }

    /// Start playing. Returns false when nothing is loaded.
    pub fn play(&mut self) -> bool {
        if self.source.is_none() {
            return false;
        }
        self.playing = true;
        true
    }

    /// Stop the transport; the position is kept.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Seek to a source frame, clamped to the track.
    pub fn seek(&mut self, frame: u64) -> u64 {
        let Some(source) = self.source.as_mut() else {
            return 0;
        };
        let clamped = match source.frames() {
            Some(total) => frame.min(total.saturating_sub(1)),
            None => frame,
        };
        // A command seeks now; the audio thread only asks, so a decoder outside the prefetch
        // window cannot stall the callback for however long the file needs to be re-read.
        self.reading_loop_buffer = false;
        let result = if self.blocking {
            source.seek(clamped)
        } else {
            source.request_seek(clamped)
        };
        match result {
            Ok(landed) => {
                self.position = landed;
                landed
            }
            Err(_) => self.position,
        }
    }

    /// Seek by playing position as a fraction of the track.
    pub fn seek_ratio(&mut self, ratio: f32) -> u64 {
        let total = self.source.as_ref().and_then(|source| source.frames());
        match total {
            Some(frames) => {
                let clamped = f64::from(ratio.clamp(0.0, 1.0));
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let target = (clamped * frames as f64) as u64;
                self.seek(target)
            }
            None => self.position,
        }
    }

    /// Activate or clear a loop. The region is in source frames.
    ///
    /// An active loop engages when the playhead reaches its end and then repeats `start..end`. A
    /// region entirely behind the playhead is ignored until a seek brings the playhead back into
    /// it — activating a loop must never move the playhead by itself.
    pub fn set_loop(&mut self, region: Option<FrameRange>) {
        self.loop_region = region;
        self.reading_loop_buffer = false;
        self.loop_buffer = match region {
            Some(region) => self.buffer_for(region),
            None => None,
        };
    }

    /// Capture policy in force.
    pub fn loop_config(&self) -> LoopConfig {
        self.loop_config
    }

    /// Change the capture policy. An active loop is re-evaluated against the new limit.
    pub fn set_loop_config(&mut self, config: LoopConfig) {
        self.loop_config = config;
        let region = self.loop_region;
        self.loop_buffer = region.and_then(|region| self.buffer_for(region));
        self.reading_loop_buffer = false;
    }

    /// Whether the active loop is played from memory (no seam).
    pub fn loop_is_buffered(&self) -> bool {
        self.loop_buffer
            .as_ref()
            .is_some_and(LoopBuffer::is_complete)
    }

    /// Allocate the capture buffer for `region`, if the loop is short enough to justify it.
    ///
    /// Allocation happens here, on the command path, never in the render.
    fn buffer_for(&self, region: FrameRange) -> Option<LoopBuffer> {
        let frames = region.end.0.saturating_sub(region.start.0);
        if frames == 0 {
            return None;
        }
        let sample_rate = self
            .source
            .as_ref()
            .map_or(0, |source| source.sample_rate());
        if frames > self.loop_config.max_frames(sample_rate) {
            return None;
        }
        let samples = usize::try_from(frames).ok()?.checked_mul(2)?;
        Some(LoopBuffer {
            start: region.start.0,
            data: vec![0.0; samples],
            captured: 0,
        })
    }

    /// Record freshly read source audio into the loop buffer.
    ///
    /// Capture is what makes the seam free: the first pass through the loop records the audio, and
    /// every later pass reads it back. A loop whose first pass has not finished still wraps by
    /// seeking, exactly as before.
    fn capture_loop(&mut self, from_frame: u64, samples: &[f32]) {
        let Some(region) = self.loop_region else {
            return;
        };
        let Some(buffer) = self.loop_buffer.as_mut() else {
            return;
        };
        if from_frame < region.start.0 {
            return;
        }
        let offset = from_frame - region.start.0;
        let frames_here = (samples.len() / 2) as u64;
        if offset + frames_here > buffer.frames() {
            return;
        }
        let start = (offset * 2) as usize;
        buffer.data[start..start + samples.len()].copy_from_slice(samples);
        buffer.captured = buffer.captured.max(offset + frames_here);
    }

    /// Active loop, if any.
    pub fn loop_region(&self) -> Option<FrameRange> {
        self.loop_region
    }

    /// Store the current position in `slot` (0-based).
    pub fn store_hotcue(&mut self, slot: usize) -> bool {
        if slot >= HOTCUE_SLOTS {
            return false;
        }
        self.hotcues[slot] = Some(self.position);
        true
    }

    /// Put `slot` at an explicit frame, which is how a stored cue comes back after a reload.
    pub fn set_hotcue(&mut self, slot: usize, frame: u64) -> bool {
        if slot >= HOTCUE_SLOTS {
            return false;
        }
        let clamped = match self.source.as_ref().and_then(|source| source.frames()) {
            Some(total) => frame.min(total.saturating_sub(1)),
            None => frame,
        };
        self.hotcues[slot] = Some(clamped);
        true
    }

    /// Delete a hotcue.
    pub fn delete_hotcue(&mut self, slot: usize) -> bool {
        match self.hotcues.get_mut(slot) {
            Some(entry) => entry.take().is_some(),
            None => false,
        }
    }

    /// Jump to a stored hotcue. Returns the new position, or `None` when the slot is empty.
    pub fn jump_hotcue(&mut self, slot: usize) -> Option<u64> {
        let frame = *self.hotcues.get(slot)?;
        let target = frame?;
        self.stats.hotcue_jumps += 1;
        Some(self.seek(target))
    }

    /// Stored hotcues, in slot order.
    pub fn hotcues(&self) -> [Option<u64>; HOTCUE_SLOTS] {
        self.hotcues
    }

    /// Set the three band gains. Cheap by construction: the coefficients were tabulated when the
    /// deck was built, so this is safe to call from the audio callback.
    pub fn set_eq_gains(&mut self, gains: EqGains) {
        self.eq.set_gains(gains);
    }

    /// The band gains in force, as applied — already on the knob's grid.
    pub fn eq_gains(&self) -> EqGains {
        self.eq.gains()
    }

    /// Set channel gain, linear.
    pub fn set_gain(&mut self, gain: f32) {
        if gain.is_finite() {
            self.gain = gain.clamp(0.0, 4.0);
        }
    }

    /// Current gain.
    /// Current output peaks, without building a snapshot.
    pub fn levels(&self) -> Levels {
        self.levels
    }

    /// Current tempo ratio, without building a snapshot.
    pub fn tempo_ratio(&self) -> f64 {
        self.tempo_ratio
    }

    /// Track metadata, without building a snapshot.
    pub fn track(&self) -> Option<&TrackMeta> {
        self.track.as_ref()
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Tempo ratio is stored here and applied by the time-stretch stage (M2); it does not change
    /// which source frames are read yet, and the snapshot says so honestly.
    pub fn set_tempo_ratio(&mut self, ratio: f64) {
        if ratio.is_finite() && ratio > 0.0 {
            self.tempo_ratio = ratio;
        }
    }

    /// Render one block of interleaved stereo frames, looping at the sample the loop ends on.
    ///
    /// Silence when paused or empty. Gain is applied here, peaks are measured here — this is the
    /// deck's contribution to the mix, not the mix itself (that is A04).
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        if !self.playing || self.source.is_none() {
            self.levels = Levels::SILENT;
            return;
        }

        let frames = out.len() / 2;
        let mut written = 0usize;
        while written < frames {
            // Once the loop has been captured, the seam is a copy: no seek, no gap (ADR-0009).
            if self.reading_loop_buffer
                && let Some(filled) = self.read_loop_buffer(&mut out[written * 2..])
            {
                written += filled;
                if filled == 0 {
                    break;
                }
                continue;
            }

            let total = self.source.as_ref().and_then(|source| source.frames());
            let until_end = total
                .map(|total| total.saturating_sub(self.position))
                .unwrap_or(u64::MAX);
            // A loop caps the read at its end whether the playhead is inside the region or still
            // before it: the wrap happens when the end is reached, not when it is entered.
            let until_loop = match self.loop_region {
                Some(region) if self.position < region.end.0 => {
                    region.end.0.saturating_sub(self.position)
                }
                _ => u64::MAX,
            };
            // Only a read that was cut by the loop end wraps; a playhead that merely sits past an
            // active region (after a seek beyond it) plays on.
            let clamped_by_loop = until_loop != u64::MAX;
            let request = until_end
                .min(until_loop)
                .clamp(1, (frames - written) as u64);
            let mut take = usize::try_from(request).unwrap_or(frames - written);

            // Wait for the prefetch window instead of rendering a momentarily empty ring as
            // silence: otherwise the same scenario would produce different bytes depending on
            // thread timing. A short wait is not an underrun; zero frames is.
            if self.blocking
                && let Some(source) = self.source.as_mut()
            {
                match source.wait_for(take) {
                    Ok(available) => take = take.min(available.max(1)),
                    Err(_) => {
                        self.stats.underruns += 1;
                        break;
                    }
                }
            }
            if take == 0 {
                self.stats.underruns += 1;
                break;
            }

            let result = self
                .source
                .as_mut()
                .map(|source| source.read(&mut out[written * 2..(written + take) * 2]));
            match result {
                Some(Ok(0)) => {
                    self.stats.underruns += 1;
                    break;
                }
                Some(Ok(read)) => {
                    let from = self.position;
                    self.position += read as u64;
                    self.capture_loop(from, &out[written * 2..written * 2 + read * 2]);
                    written += read;
                }
                Some(Err(_)) => {
                    self.stats.underruns += 1;
                    break;
                }
                None => break,
            }

            if clamped_by_loop
                && let Some(region) = self.loop_region
                && self.position >= region.end.0
            {
                let buffered = self
                    .loop_buffer
                    .as_ref()
                    .is_some_and(LoopBuffer::is_complete);
                if buffered {
                    // Nothing to ask the decoder: the loop is in memory from here on.
                    self.reading_loop_buffer = true;
                } else if let Some(source) = self.source.as_mut() {
                    // Measured on the S4 run: a blocking wrap seek cost 3.8 ms inside the callback
                    // and produced late intervals. Offline waits (deterministic WAV), live asks.
                    let _ = if self.blocking {
                        source.seek(region.start.0)
                    } else {
                        source.request_seek(region.start.0)
                    };
                }
                self.position = region.start.0;
                self.stats.loop_wraps += 1;
            }
        }

        for sample in out.iter_mut() {
            *sample *= self.gain;
        }
        // Trim first, then the EQ: that is the order of a channel strip, and the order the mixer's
        // contract names when it says a deck arrives already gain-staged and equalised.
        self.eq.process(out);
        self.levels = measure_levels(out);
    }

    /// Fill `out` from the captured loop, wrapping at its end. `None` when there is nothing to read.
    fn read_loop_buffer(&mut self, out: &mut [f32]) -> Option<usize> {
        let region = self.loop_region?;
        let buffer = self.loop_buffer.as_ref()?;
        if !buffer.is_complete() {
            return None;
        }
        let want = out.len() / 2;
        if want == 0 {
            return Some(0);
        }
        if self.position < region.start.0 || self.position >= region.end.0 {
            self.position = region.start.0;
        }
        let offset = usize::try_from(self.position.saturating_sub(buffer.start)).unwrap_or(0);
        let frames = usize::try_from(buffer.frames()).unwrap_or(0);
        if offset >= frames {
            self.position = region.start.0;
            return Some(0);
        }
        let take = want.min(frames - offset);
        out[..take * 2].copy_from_slice(&buffer.data[offset * 2..(offset + take) * 2]);
        self.position += take as u64;
        if self.position >= region.end.0 {
            self.position = region.start.0;
            self.stats.loop_wraps += 1;
        }
        Some(take)
    }

    /// Read-only state for displays and the UI (fields ordered by display priority, ADR-0006).
    pub fn snapshot(&self) -> DeckSnapshot {
        let total = self.source.as_ref().and_then(|source| source.frames());
        let remaining = total.map(|total| total.saturating_sub(self.position));
        let ratio = total.and_then(|total| {
            if total == 0 {
                None
            } else {
                Some((self.position as f32 / total as f32).clamp(0.0, 1.0))
            }
        });
        DeckSnapshot {
            waveform: None,
            deck: self.id,
            load: self.load.clone(),
            playing: self.playing,
            source_frame: SourceFrame(self.position),
            remaining_frames: remaining,
            position_ratio: ratio,
            track: self.track.clone(),
            loop_region: self.loop_region,
            hotcues: self
                .hotcues
                .iter()
                .map(|slot| slot.map(SourceFrame))
                .collect(),
            levels: self.levels,
            linear_gain: self.gain,
            tempo_ratio: self.tempo_ratio,
            eq: self.eq.gains(),
        }
    }
}

fn measure_levels(block: &[f32]) -> Levels {
    let mut left = 0.0f32;
    let mut right = 0.0f32;
    for pair in block.chunks_exact(2) {
        left = left.max(pair[0].abs());
        right = right.max(pair[1].abs());
    }
    Levels {
        peak_left: left.min(1.0),
        peak_right: right.min(1.0),
    }
}
