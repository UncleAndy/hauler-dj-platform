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
    DeckId, DeckLoad, DeckSnapshot, FrameRange, HOTCUE_SLOTS, Levels, SourceFrame, TrackMeta,
};

/// Where a deck reads frames from. Implemented by the prefetcher; tests provide their own.
pub trait DeckSource: Send {
    /// Sample rate of the material.
    fn sample_rate(&self) -> u32;
    /// Playable frames, when known.
    fn frames(&self) -> Option<u64>;
    /// Read up to `out.len() / 2` stereo frames; returns frames written, 0 at the end.
    fn read(&mut self, out: &mut [f32]) -> Result<usize, String>;
    /// Seek to a source frame; returns the frame the source landed on.
    fn seek(&mut self, frame: u64) -> Result<u64, String>;

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
        self.seek(frame)
            .map(|_| self.info().frames.map_or(frame, |_| frame))
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
    pub fn load_state(&self) -> &DeckLoad {
        &self.load
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
        match source.seek(clamped) {
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

    /// Set channel gain, linear.
    pub fn set_gain(&mut self, gain: f32) {
        if gain.is_finite() {
            self.gain = gain.clamp(0.0, 4.0);
        }
    }

    /// Current gain.
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
            if let Some(source) = self.source.as_mut() {
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
                    self.position += read as u64;
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
                if let Some(source) = self.source.as_mut() {
                    let _ = source.seek(region.start.0);
                }
                self.position = region.start.0;
                self.stats.loop_wraps += 1;
            }
        }

        for sample in out.iter_mut() {
            *sample *= self.gain;
        }
        self.levels = measure_levels(out);
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
