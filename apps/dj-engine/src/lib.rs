//! The engine's own logic, separate from the process that hosts it.
//!
//! The binary owns threads, sockets and signals; everything that decides what a command does and
//! what a snapshot says lives here, so it can be tested without spawning a process. That
//! distinction came from a hanging integration test: spawning a server inside a test binary is a
//! fragile way to check a rule.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use dj_audio_io::Renderer;
use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::{Deck, DeckSource};
use dj_domain::CommandId;
use dj_domain::{
    Ack, AckStatus, CueAction, DeckId, DeckLoad, DeckSnapshot, DjCommand, FrameRange, Levels,
    RejectReason, ScheduledCommand, Snapshot, SourceFrame, TrackMeta,
};
use dj_mixer::{ChannelSettings, Mixer};

pub fn apply(decks: &mut [Deck], mixer: &mut Mixer, command: &ScheduledCommand, clock: u64) -> Ack {
    let id = command.id;
    let at = command.effective_sample.max(clock);

    // Everything addressed to a deck resolves the target once, so an unknown deck is reported the
    // same way for every command instead of once per variant.
    let deck_id = match &command.command {
        DjCommand::Play { deck, .. }
        | DjCommand::Cue { deck, .. }
        | DjCommand::SeekFrames { deck, .. }
        | DjCommand::SetLoop { deck, .. }
        | DjCommand::SetTempoRatio { deck, .. }
        | DjCommand::Eject { deck }
        | DjCommand::Load { deck, .. } => Some(*deck),
        _ => None,
    };

    if let Some(deck_id) = deck_id {
        let Some(target) = decks.iter_mut().find(|candidate| candidate.id() == deck_id) else {
            return Ack {
                command_id: id,
                status: AckStatus::Rejected(RejectReason::InvalidValue),
            };
        };
        return match &command.command {
            DjCommand::Play { enabled, .. } => {
                if *enabled {
                    if !target.play() {
                        return Ack {
                            command_id: id,
                            status: AckStatus::Rejected(RejectReason::NotSupported),
                        };
                    }
                } else {
                    target.pause();
                }
                Ack::applied(id, at)
            }
            DjCommand::Cue { action, .. } => {
                let done = match *action {
                    CueAction::Set(slot) => target.store_hotcue(usize::from(slot)),
                    CueAction::Delete(slot) => {
                        target.delete_hotcue(usize::from(slot));
                        true
                    }
                    CueAction::Jump(slot) => target.jump_hotcue(usize::from(slot)).is_some(),
                };
                if done {
                    Ack::applied(id, at)
                } else {
                    Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::InvalidValue),
                    }
                }
            }
            DjCommand::SeekFrames { frame, .. } => {
                target.seek(frame.0);
                Ack::applied(id, at)
            }
            DjCommand::SetLoop { region, .. } => {
                target.set_loop(*region);
                Ack::applied(id, at)
            }
            // A load never runs in the callback; if it arrives here the routing broke, and a
            // rejection is better than an ack that claims work which did not happen.
            DjCommand::Load { .. } => Ack {
                command_id: id,
                status: AckStatus::Rejected(RejectReason::NotSupported),
            },
            DjCommand::Eject { .. } => {
                target.eject();
                Ack::applied(id, at)
            }
            // Tempo needs a resampler the graph does not have yet (A04/M2).
            DjCommand::SetTempoRatio { .. } => Ack {
                command_id: id,
                status: AckStatus::Rejected(RejectReason::NotSupported),
            },
            _ => unreachable!("only deck commands reach here"),
        };
    }

    match &command.command {
        DjCommand::SetCrossfader { position } => {
            mixer.set_crossfader(*position);
            Ack::applied(id, at)
        }
        DjCommand::SetChannelGain { deck, linear_gain } => {
            let index = match deck {
                DeckId::A => 0,
                _ => 1,
            };
            if let Some(settings) = mixer.channel(index).copied() {
                mixer.set_channel(
                    index,
                    ChannelSettings {
                        fader: linear_gain.clamp(0.0, 1.0),
                        ..settings
                    },
                );
                Ack::applied(id, at)
            } else {
                Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                }
            }
        }
        _ => Ack {
            command_id: id,
            status: AckStatus::Rejected(RejectReason::NotSupported),
        },
    }
}

pub fn snapshot(
    decks: &[Deck],
    mixer: &Mixer,
    revision: u64,
    clock: u64,
    rate: u32,
    epoch: u64,
) -> Snapshot {
    Snapshot {
        protocol: dj_domain::STATE_PROTOCOL.to_string(),
        revision,
        epoch,
        audio_clock_samples: clock,
        sample_rate: rate,
        crossfader: mixer.crossfader(),
        decks: decks.iter().map(|deck| deck.snapshot()).collect(),
    }
}

/// How many decks the published state has room for.
pub const MAX_DECKS: usize = 4;

/// Hotcue slots per deck, matching the contract.
pub const HOTCUE_SLOTS: usize = 8;

/// Sentinel for "no value" inside an atomic.
const EMPTY: u64 = u64::MAX;

/// Facts about a deck that change only when a track is prepared.
///
/// These carry strings and are therefore never touched by the render thread; they are cloned when
/// the publisher builds a snapshot.
#[derive(Clone, Debug)]
pub struct DeckStatic {
    /// Which deck.
    pub deck: DeckId,
    /// Preparation state.
    pub load: DeckLoad,
    /// Track metadata, when something is loaded.
    pub track: Option<TrackMeta>,
    /// Total frames, when the duration is known.
    pub total_frames: Option<u64>,
}

impl DeckStatic {
    /// Take the static half of a deck snapshot.
    pub fn from_snapshot(snapshot: &DeckSnapshot) -> Self {
        let total_frames = snapshot
            .track
            .as_ref()
            .and_then(|track| track.duration_frames)
            .or_else(|| {
                snapshot
                    .remaining_frames
                    .map(|left| left + snapshot.source_frame.0)
            });
        Self {
            deck: snapshot.deck,
            load: snapshot.load.clone(),
            track: snapshot.track.clone(),
            total_frames,
        }
    }
}

/// Per-deck scalar state, published without a lock.
struct DeckCells {
    playing: AtomicBool,
    frame: AtomicU64,
    gain: AtomicU32,
    tempo: AtomicU64,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    hotcues: [AtomicU64; HOTCUE_SLOTS],
}

impl DeckCells {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            frame: AtomicU64::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            tempo: AtomicU64::new(1.0f64.to_bits()),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            loop_start: AtomicU64::new(EMPTY),
            loop_end: AtomicU64::new(EMPTY),
            hotcues: std::array::from_fn(|_| AtomicU64::new(EMPTY)),
        }
    }
}

/// The render thread's only way out: plain atomic stores.
///
/// A snapshot allocates (it carries track metadata and a hotcue vector), so building one in the
/// audio path would break the real-time contract. The render thread therefore stores numbers here,
/// and a separate thread reads them at display rate and assembles the message. This is the fix for
/// the mutex-in-the-render-thread that A06 shipped with.
pub struct StateCells {
    clock: AtomicU64,
    crossfader: AtomicU32,
    count: AtomicUsize,
    decks: [DeckCells; MAX_DECKS],
}

impl Default for StateCells {
    fn default() -> Self {
        Self::new()
    }
}

impl StateCells {
    /// Empty state, room for [`MAX_DECKS`] decks.
    pub fn new() -> Self {
        Self {
            clock: AtomicU64::new(0),
            crossfader: AtomicU32::new(0.5f32.to_bits()),
            count: AtomicUsize::new(0),
            decks: std::array::from_fn(|_| DeckCells::new()),
        }
    }

    /// Publish the scalars. No allocation, no lock, no syscall: callable from a callback.
    pub fn publish(&self, decks: &[Deck], mixer: &Mixer, clock: u64) {
        self.clock.store(clock, Ordering::Relaxed);
        self.crossfader
            .store(mixer.crossfader().to_bits(), Ordering::Relaxed);
        let count = decks.len().min(MAX_DECKS);
        self.count.store(count, Ordering::Release);
        for (index, deck) in decks.iter().take(MAX_DECKS).enumerate() {
            let cells = &self.decks[index];
            cells.playing.store(deck.is_playing(), Ordering::Relaxed);
            cells.frame.store(deck.position(), Ordering::Relaxed);
            cells.gain.store(deck.gain().to_bits(), Ordering::Relaxed);
            cells
                .tempo
                .store(deck.tempo_ratio().to_bits(), Ordering::Relaxed);
            let levels = deck.levels();
            cells
                .peak_left
                .store(levels.peak_left.to_bits(), Ordering::Relaxed);
            cells
                .peak_right
                .store(levels.peak_right.to_bits(), Ordering::Relaxed);
            match deck.loop_region() {
                Some(region) => {
                    cells.loop_start.store(region.start.0, Ordering::Relaxed);
                    cells.loop_end.store(region.end.0, Ordering::Relaxed);
                }
                None => {
                    cells.loop_start.store(EMPTY, Ordering::Relaxed);
                    cells.loop_end.store(EMPTY, Ordering::Relaxed);
                }
            }
            for (slot, cue) in deck.hotcues().iter().enumerate() {
                cells.hotcues[slot].store(cue.unwrap_or(EMPTY), Ordering::Relaxed);
            }
        }
    }

    /// Assemble a snapshot. Allocates, so it belongs to the publisher thread, never the callback.
    pub fn read(&self, statics: &[DeckStatic], rate: u32, epoch: u64, revision: u64) -> Snapshot {
        let count = self
            .count
            .load(Ordering::Acquire)
            .min(statics.len())
            .min(MAX_DECKS);
        let mut decks = Vec::with_capacity(count);
        for (index, fixed) in statics.iter().take(count).enumerate() {
            let cells = &self.decks[index];
            let frame = cells.frame.load(Ordering::Relaxed);
            let total = fixed.total_frames;
            let loop_start = cells.loop_start.load(Ordering::Relaxed);
            let loop_end = cells.loop_end.load(Ordering::Relaxed);
            decks.push(DeckSnapshot {
                deck: fixed.deck,
                load: fixed.load.clone(),
                playing: cells.playing.load(Ordering::Relaxed),
                source_frame: SourceFrame(frame),
                remaining_frames: total.map(|total| total.saturating_sub(frame)),
                position_ratio: total
                    .filter(|total| *total > 0)
                    .map(|total| (frame as f32 / total as f32).clamp(0.0, 1.0)),
                track: fixed.track.clone(),
                loop_region: (loop_start != EMPTY && loop_end != EMPTY).then_some(FrameRange {
                    start: SourceFrame(loop_start),
                    end: SourceFrame(loop_end),
                }),
                hotcues: cells
                    .hotcues
                    .iter()
                    .map(|slot| {
                        let value = slot.load(Ordering::Relaxed);
                        (value != EMPTY).then_some(SourceFrame(value))
                    })
                    .collect(),
                levels: Levels {
                    peak_left: f32::from_bits(cells.peak_left.load(Ordering::Relaxed)),
                    peak_right: f32::from_bits(cells.peak_right.load(Ordering::Relaxed)),
                },
                linear_gain: f32::from_bits(cells.gain.load(Ordering::Relaxed)),
                tempo_ratio: f64::from_bits(cells.tempo.load(Ordering::Relaxed)),
            });
        }
        Snapshot {
            protocol: dj_domain::STATE_PROTOCOL.to_string(),
            revision,
            epoch,
            audio_clock_samples: self.clock.load(Ordering::Acquire),
            sample_rate: rate,
            crossfader: f32::from_bits(self.crossfader.load(Ordering::Relaxed)),
            decks,
        }
    }
}

/// What a client thread hands to the render loop.
pub struct Submission {
    /// The command, with its deadline in device samples.
    pub command: ScheduledCommand,
    /// Where the ack goes; a client waits on it before reporting anything to the user.
    pub reply: Sender<Ack>,
}

/// A load the client thread hands to the loader, never to the audio callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    /// Command the ack belongs to.
    pub command_id: CommandId,
    /// Deck to fill.
    pub deck: DeckId,
    /// Absolute path as the client spelled it.
    pub path: String,
}

/// What the loader asks the graph to do at its next block boundary.
pub enum GraphLoad {
    /// A prepared track is ready to be streamed. Boxed because the alternative is a 100-byte enum
    /// where every clear carries the payload of an attach.
    Attach(Box<PreparedTrack>),
    /// Empty the deck; the loader owned the decision, the graph owns the source.
    Clear {
        /// Target deck.
        deck: DeckId,
    },
}

/// What the loader hands back to the graph once the deck is playable.
pub struct PreparedTrack {
    /// Deck to fill.
    pub deck: DeckId,
    /// Ready-to-stream source; the decode window is already warm.
    pub source: Box<dyn DeckSource>,
    /// Display metadata.
    pub track: Option<TrackMeta>,
    /// Static half of the snapshot for this deck.
    pub static_info: DeckStatic,
}

/// What the loader tells the publisher, so the UI can show `Loading` and `Failed`.
#[derive(Debug, Clone)]
pub struct DeckUpdate {
    /// Deck the update is about.
    pub deck: DeckId,
    /// New static description.
    pub static_info: DeckStatic,
}

/// Where a command has to go.
///
/// `Load` and `Eject` must not run in the audio callback: opening a file, decoding and building a
/// PCM cache take milliseconds to seconds, and the callback has 5.3 of them. The client thread
/// routes them to the loader, which is off the audio path by construction.
pub enum Routed {
    /// Applied by the render loop at its next block boundary.
    Render,
    /// Handed to the loader; the state reports `Ready` when the deck is playable.
    Load(LoadRequest),
    /// Handed to the loader, which asks the graph to clear the deck.
    Eject {
        /// Target deck.
        deck: DeckId,
    },
}

/// Decide where a command goes without applying it.
pub fn route(command: &ScheduledCommand) -> Routed {
    match &command.command {
        DjCommand::Load { deck, path } => Routed::Load(LoadRequest {
            command_id: command.id,
            deck: *deck,
            path: path.clone(),
        }),
        DjCommand::Eject { deck } => Routed::Eject { deck: *deck },
        _ => Routed::Render,
    }
}

/// Open a file, make it playable at `rate` and describe it for the display (ADR-0008).
///
/// Runs on the loader thread. The PCM-cache decision is taken here, at load, from the file's
/// format — never in the middle of playback, where it would show up as a dropout.
pub fn prepare_track(
    path: &std::path::Path,
    rate: u32,
    window_seconds: f64,
) -> Result<PreparedTrack, String> {
    let config = PrefetchConfig::for_sample_rate(rate, window_seconds);
    let mut prefetcher = Prefetcher::open(path, config).map_err(|err| err.to_string())?;
    let info = prefetcher.info().clone();
    prefetcher
        .wait_ready(config.window_frames, std::time::Duration::from_secs(60))
        .map_err(|err| err.to_string())?;

    let title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let track = TrackMeta {
        title,
        artist: String::new(),
        duration_frames: info.frames,
        bpm: None,
        key: None,
    };
    let static_info = DeckStatic {
        deck: DeckId::A, // replaced by the caller, which knows the deck
        load: DeckLoad::Ready,
        track: Some(track.clone()),
        total_frames: info.frames,
    };
    Ok(PreparedTrack {
        deck: DeckId::A,
        source: Box::new(prefetcher),
        track: Some(track),
        static_info,
    })
}

/// Decks → channel strips → master and cue, driven either by a device callback or by a null sink.
///
/// One implementation for both: the null sink paces itself and the device paces the callback, but
/// what happens inside a block — drain commands, render decks, mix, publish counters — is the same
/// code, so a bug cannot live in only one of the two paths.
pub struct EngineGraph {
    decks: Vec<Deck>,
    mixer: Mixer,
    submissions: Receiver<Submission>,
    cells: Arc<StateCells>,
    block: usize,
    clock: u64,
    scratch: Vec<f32>,
    /// Prepared tracks waiting for a block boundary; drained with `try_recv`, which allocates
    /// nothing.
    loads: Receiver<GraphLoad>,
    /// Sources taken off a deck, parked rather than freed in the callback. The vector is
    /// preallocated and released when the engine stops.
    retired: Vec<Box<dyn DeckSource>>,
    /// Loads applied since start, so the report can say whether a load ever stalled a block.
    loads_applied: u64,
}

impl EngineGraph {
    /// Build the graph for `block` frames per callback.
    pub fn new(
        decks: Vec<Deck>,
        mixer: Mixer,
        submissions: Receiver<Submission>,
        cells: Arc<StateCells>,
        block: usize,
        loads: Receiver<GraphLoad>,
    ) -> Self {
        let block = block.max(1);
        Self {
            decks,
            mixer,
            submissions,
            cells,
            block,
            clock: 0,
            scratch: vec![0.0f32; block * 2],
            loads,
            retired: Vec::with_capacity(16),
            loads_applied: 0,
        }
    }

    /// Device-clock position after the last rendered block.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// Decks, for a caller that needs to inspect them (tests, mostly).
    pub fn decks(&self) -> &[Deck] {
        &self.decks
    }

    /// How many prepared tracks were attached since start.
    pub fn loads_applied(&self) -> u64 {
        self.loads_applied
    }

    /// Render one block into `master` and `cue`, interleaved stereo.
    ///
    /// Everything here runs inside the audio callback, so it allocates nothing, locks nothing, and
    /// never waits for a decoder: a deck that has no data renders silence and counts it.
    pub fn render_pair(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize) {
        let frames = frames.min(self.block);
        // A prepared track is attached here, at a block boundary: the decode already happened on
        // the loader thread, so this costs a pointer swap and the release of the old decoder.
        while let Ok(load) = self.loads.try_recv() {
            match load {
                GraphLoad::Attach(prepared) => {
                    if let Some(index) = self
                        .decks
                        .iter()
                        .position(|deck| deck.id() == prepared.deck)
                    {
                        let old = self.decks[index].take_source();
                        self.park_old(old);
                        self.decks[index].attach(prepared.source, prepared.track);
                        self.loads_applied += 1;
                    }
                }
                GraphLoad::Clear { deck } => {
                    if let Some(index) = self
                        .decks
                        .iter()
                        .position(|candidate| candidate.id() == deck)
                    {
                        let old = self.decks[index].take_source();
                        self.park_old(old);
                        self.decks[index].eject();
                        self.loads_applied += 1;
                    }
                }
            }
        }

        // Commands first: a command due at this sample is applied at this sample.
        while let Ok(submission) = self.submissions.try_recv() {
            let ack = apply(
                &mut self.decks,
                &mut self.mixer,
                &submission.command,
                self.clock,
            );
            let _ = submission.reply.send(ack);
        }

        self.mixer.begin_block(frames);
        for index in 0..self.decks.len() {
            let deck = &mut self.decks[index];
            deck.render(&mut self.scratch[..frames * 2]);
            self.mixer.mix_channel(index, &self.scratch[..frames * 2]);
        }
        self.mixer.finish_block();

        master[..frames * 2].copy_from_slice(&self.mixer.master()[..frames * 2]);
        master[frames * 2..].fill(0.0);

        let cue_frames = (cue.len() / 2).min(self.mixer.cue().len() / 2);
        cue[..cue_frames * 2].copy_from_slice(&self.mixer.cue()[..cue_frames * 2]);
        cue[cue_frames * 2..].fill(0.0);

        self.clock += frames as u64;
        self.cells.publish(&self.decks, &self.mixer, self.clock);
    }
}

impl EngineGraph {
    /// Move the deck's source into the park, where it is freed after the run rather than inside a
    /// callback. Beyond the preallocated capacity the source is dropped instead, which is the
    /// lesser evil for an engine that ejects dozens of tracks in a session, not thousands.
    fn park_old(&mut self, old: Option<Box<dyn DeckSource>>) {
        if let Some(old) = old
            && self.retired.len() < self.retired.capacity()
        {
            self.retired.push(old);
        }
    }
}

impl Renderer for EngineGraph {
    fn render(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize) {
        self.render_pair(master, cue, frames);
    }
}
