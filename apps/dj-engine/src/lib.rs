//! The engine's own logic, separate from the process that hosts it.
//!
//! The binary owns threads, sockets and signals; everything that decides what a command does and
//! what a snapshot says lives here, so it can be tested without spawning a process. That
//! distinction came from a hanging integration test: spawning a server inside a test binary is a
//! fragile way to check a rule.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use dj_audio_io::Renderer;
use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::{Deck, DeckSource};
use dj_domain::CommandId;
use dj_domain::{
    Ack, AckStatus, CueAction, DeckId, DeckLoad, DeckSnapshot, DjCommand, EqGains, FrameRange,
    Levels, RejectReason, ScheduledCommand, Snapshot, SourceFrame, TrackMeta,
};
use dj_mixer::{ChannelSettings, CrossAssign, CrossfaderCurve, Mixer};

pub mod meta;

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
            // The deck resamples: the rate is its own, and the pitch moves with it, as on a turntable.
            // Key lock — the rate without the pitch — is a different task (DJP-DECK-008).
            DjCommand::SetTempoRatio { ratio, .. } => {
                if ratio.is_finite()
                    && (dj_deck::MIN_TEMPO_RATIO..=dj_deck::MAX_TEMPO_RATIO).contains(ratio)
                {
                    target.set_tempo(*ratio);
                    Ack::applied(id, at)
                } else {
                    Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::InvalidValue),
                    }
                }
            }
            _ => unreachable!("only deck commands reach here"),
        };
    }

    match &command.command {
        DjCommand::SetCrossfaderCurve { curve } => {
            mixer.set_curve(*curve);
            Ack::applied(id, at)
        }
        DjCommand::SetMasterGain { linear_gain } => {
            mixer.set_master_gain(*linear_gain);
            Ack::applied(id, at)
        }
        DjCommand::SetCrossfader { position } => {
            mixer.set_crossfader(*position);
            Ack::applied(id, at)
        }
        DjCommand::SetChannelGain { deck, linear_gain } => {
            // A channel's gain is its deck's own trim: it is what the deck applies to its output and
            // what the snapshot reports back as `linear_gain`. This used to move a mixer channel
            // fader instead — and every deck but A moved channel 1, so a knob on C changed B while the
            // slider sprang back to a value nobody had set.
            match decks.iter_mut().find(|candidate| candidate.id() == *deck) {
                Some(target) => {
                    target.set_gain(*linear_gain);
                    Ack::applied(id, at)
                }
                None => Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                },
            }
        }
        DjCommand::SetChannelFader { deck, position } => {
            // The fader belongs to the mixer's strip; the gain belongs to the deck. Writing a fader
            // into the deck's gain was the old mistake: the slider sprang back and a knob moved the
            // wrong channel.
            let Some(index) = decks.iter().position(|candidate| candidate.id() == *deck) else {
                return Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                };
            };
            let Some(current) = mixer.channel(index).copied() else {
                return Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                };
            };
            if !position.is_finite() {
                return Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                };
            }
            mixer.set_channel(
                index,
                ChannelSettings {
                    fader: *position,
                    ..current
                },
            );
            Ack::applied(id, at)
        }
        DjCommand::SetChannelEq {
            deck,
            band,
            gain_db,
        } => match decks.iter_mut().find(|candidate| candidate.id() == *deck) {
            Some(target) => {
                // The coefficients for every knob position were tabulated when the deck was built, so
                // this arm is a lookup and a copy of five numbers — a knob is allowed in the callback,
                // arithmetic is not.
                let mut gains = target.eq_gains();
                gains.set_band(*band, *gain_db);
                target.set_eq_gains(gains);
                Ack::applied(id, at)
            }
            None => Ack {
                command_id: id,
                status: AckStatus::Rejected(RejectReason::InvalidValue),
            },
        },
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
        crossfader_curve: mixer.curve(),
        master: dj_domain::Levels {
            peak_left: mixer.meters().master.peak,
            peak_right: mixer.meters().master.peak,
        },
        master_gain: mixer.master_gain(),
        decks: decks
            .iter()
            .enumerate()
            .map(|(index, deck)| {
                let mut published = deck.snapshot();
                // The fader is the mixer's, keyed by the strip the deck feeds.
                published.fader = mixer.channel(index).map_or(1.0, |settings| settings.fader);
                published
            })
            .collect(),
    }
}

/// How many decks the published state has room for.
pub const MAX_DECKS: usize = 4;

/// Which side of the crossfader each deck follows.
///
/// Every channel used to be `Thru`, which bypasses the crossfader by design: the handle moved and the
/// sound never changed, because nothing was listening to it. A and C are the left side, B and D the
/// right, matching where the decks sit in the shell.
pub fn assign_crossfader_sides(mixer: &mut Mixer, decks: &[DeckId]) {
    for (index, id) in decks.iter().enumerate() {
        let Some(existing) = mixer.channel(index).copied() else {
            continue;
        };
        let assign = match id {
            DeckId::A | DeckId::C => CrossAssign::A,
            DeckId::B | DeckId::D => CrossAssign::B,
        };
        mixer.set_channel(index, ChannelSettings { assign, ..existing });
    }
}

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
    /// Waveform envelope, once the loader has built it.
    pub waveform: Option<dj_domain::Waveform>,
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
            waveform: snapshot.waveform.clone(),
        }
    }
}

/// Per-deck scalar state, published without a lock.
struct DeckCells {
    playing: AtomicBool,
    // Whether a source is attached. This one belongs here, not in the static section: eject changes
    // it, and a snapshot that kept saying "Ready" kept a track on screen that was no longer in the
    // deck.
    attached: AtomicBool,
    frame: AtomicU64,
    gain: AtomicU32,
    tempo: AtomicU64,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    hotcues: [AtomicU64; HOTCUE_SLOTS],
    // The three band gains as applied, so the publisher can say what the strip should draw without
    // borrowing the deck.
    eq_low: AtomicU32,
    eq_mid: AtomicU32,
    eq_high: AtomicU32,
    // The channel fader as applied by the mixer strip.
    fader: AtomicU32,
}

impl DeckCells {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            attached: AtomicBool::new(false),
            frame: AtomicU64::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            tempo: AtomicU64::new(1.0f64.to_bits()),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            loop_start: AtomicU64::new(EMPTY),
            loop_end: AtomicU64::new(EMPTY),
            hotcues: std::array::from_fn(|_| AtomicU64::new(EMPTY)),
            eq_low: AtomicU32::new(0),
            eq_mid: AtomicU32::new(0),
            eq_high: AtomicU32::new(0),
            fader: AtomicU32::new(1.0f32.to_bits()),
        }
    }
}

/// The law as one byte, so the render thread can publish it without allocating.
fn curve_index(curve: CrossfaderCurve) -> u8 {
    match curve {
        CrossfaderCurve::Linear => 0,
        CrossfaderCurve::ConstantPower => 1,
        CrossfaderCurve::Smooth => 2,
        CrossfaderCurve::Sharp => 3,
    }
}

/// The inverse; an unknown byte reads as the default law rather than as nothing.
fn curve_at(index: u8) -> CrossfaderCurve {
    match index {
        0 => CrossfaderCurve::Linear,
        2 => CrossfaderCurve::Smooth,
        3 => CrossfaderCurve::Sharp,
        _ => CrossfaderCurve::ConstantPower,
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
    curve: AtomicU8,
    master_peak: AtomicU32,
    master_gain: AtomicU32,
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
            curve: AtomicU8::new(curve_index(CrossfaderCurve::default())),
            master_peak: AtomicU32::new(0),
            master_gain: AtomicU32::new(1.0f32.to_bits()),
            count: AtomicUsize::new(0),
            decks: std::array::from_fn(|_| DeckCells::new()),
        }
    }

    /// Publish the scalars. No allocation, no lock, no syscall: callable from a callback.
    pub fn publish(&self, decks: &[Deck], mixer: &Mixer, clock: u64) {
        self.clock.store(clock, Ordering::Relaxed);
        self.crossfader
            .store(mixer.crossfader().to_bits(), Ordering::Relaxed);
        self.curve
            .store(curve_index(mixer.curve()), Ordering::Relaxed);
        self.master_peak
            .store(mixer.meters().master.peak.to_bits(), Ordering::Relaxed);
        self.master_gain
            .store(mixer.master_gain().to_bits(), Ordering::Relaxed);
        let count = decks.len().min(MAX_DECKS);
        self.count.store(count, Ordering::Release);
        for (index, deck) in decks.iter().take(MAX_DECKS).enumerate() {
            let cells = &self.decks[index];
            cells.playing.store(deck.is_playing(), Ordering::Relaxed);
            cells.attached.store(deck.has_source(), Ordering::Relaxed);
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
            cells.fader.store(
                mixer
                    .channel(index)
                    .map_or(1.0, |settings| settings.fader)
                    .to_bits(),
                Ordering::Relaxed,
            );
            let eq = deck.eq_gains();
            cells.eq_low.store(eq.low_db.to_bits(), Ordering::Relaxed);
            cells.eq_mid.store(eq.mid_db.to_bits(), Ordering::Relaxed);
            cells.eq_high.store(eq.high_db.to_bits(), Ordering::Relaxed);
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
            let attached = cells.attached.load(Ordering::Relaxed);
            let frame = cells.frame.load(Ordering::Relaxed);
            let total = if attached { fixed.total_frames } else { None };
            // `Ready` means the deck holds the source, which is when a client may act on it. The
            // loader knows the file, but the graph attaches it a block later; a cue sent into that
            // window would belong to nothing, so the gap reads as `Loading` rather than `Ready`.
            let load = match (attached, &fixed.load) {
                (true, state) => state.clone(),
                (false, DeckLoad::Loading { progress }) => DeckLoad::Loading {
                    progress: *progress,
                },
                (false, _) => DeckLoad::Empty,
            };
            let loop_start = cells.loop_start.load(Ordering::Relaxed);
            let loop_end = cells.loop_end.load(Ordering::Relaxed);
            decks.push(DeckSnapshot {
                deck: fixed.deck,
                load,
                playing: cells.playing.load(Ordering::Relaxed),
                source_frame: SourceFrame(frame),
                remaining_frames: total.map(|total| total.saturating_sub(frame)),
                position_ratio: total
                    .filter(|total| *total > 0)
                    .map(|total| (frame as f32 / total as f32).clamp(0.0, 1.0)),
                track: if attached { fixed.track.clone() } else { None },
                waveform: if attached {
                    fixed.waveform.clone()
                } else {
                    None
                },
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
                fader: f32::from_bits(cells.fader.load(Ordering::Relaxed)),
                eq: EqGains {
                    low_db: f32::from_bits(cells.eq_low.load(Ordering::Relaxed)),
                    mid_db: f32::from_bits(cells.eq_mid.load(Ordering::Relaxed)),
                    high_db: f32::from_bits(cells.eq_high.load(Ordering::Relaxed)),
                },
            });
        }
        Snapshot {
            protocol: dj_domain::STATE_PROTOCOL.to_string(),
            revision,
            epoch,
            audio_clock_samples: self.clock.load(Ordering::Acquire),
            sample_rate: rate,
            crossfader: f32::from_bits(self.crossfader.load(Ordering::Relaxed)),
            crossfader_curve: curve_at(self.curve.load(Ordering::Relaxed)),
            master: dj_domain::Levels {
                peak_left: f32::from_bits(self.master_peak.load(Ordering::Relaxed)),
                peak_right: f32::from_bits(self.master_peak.load(Ordering::Relaxed)),
            },
            master_gain: f32::from_bits(self.master_gain.load(Ordering::Relaxed)),
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
    /// Where the file is, so an edit knows what to write (DJP-DATA-006).
    pub path: Option<std::path::PathBuf>,
    /// Cues recorded for this file, restored at attach.
    pub cues: Vec<(u8, u64)>,
    /// Loop recorded for this file.
    pub loop_region: Option<FrameRange>,
    /// Gain the channel opens with, from the file's own loudness (DJP-MIX-001 auto-gain).
    ///
    /// 1.0 when the file was not measured: a missing number must not silently attenuate a track.
    pub auto_gain: f32,
}

/// A change worth writing to the track's metadata file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaEdit {
    /// The file the deck holds.
    pub path: std::path::PathBuf,
    /// Cues as frames of the source file, slot and position.
    pub cues: Vec<(u8, u64)>,
    /// The active loop as `(start, end)` frames.
    pub loop_region: Option<(u64, u64)>,
}

/// What the loader tells the publisher, so the UI can show `Loading` and `Failed`.
#[derive(Debug, Clone)]
pub struct DeckUpdate {
    /// Deck the update is about.
    pub deck: DeckId,
    /// New static description.
    pub static_info: DeckStatic,
}

/// Build and cache the waveform for a file, returning the reference the UI needs.
///
/// Runs on the loader thread: it is a second decode of the file, which is exactly why it cannot
/// happen where the audio callback lives.
pub fn prepare_waveform(
    path: &std::path::Path,
    cache: &dj_analysis::WaveformCache,
) -> Result<dj_domain::Waveform, String> {
    let (peaks, _from_cache) = cache
        .get_or_analyse(path, dj_analysis::OVERVIEW_FRAMES_PER_BUCKET)
        .map_err(|err| err.to_string())?;
    let entry = cache.entry_for(path, peaks.frames_per_bucket);
    Ok(dj_domain::Waveform {
        path: entry.to_string_lossy().to_string(),
        frames_per_bucket: peaks.frames_per_bucket,
        buckets: u32::try_from(peaks.buckets.len()).unwrap_or(u32::MAX),
        sample_rate: peaks.sample_rate,
    })
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
        waveform: None, // filled by the caller, which knows the cache directory
    };
    // Loudness is measured here, on the loader thread: it is a full decode of the file, and the audio
    // callback must never see it. A file that cannot be measured opens at unity.
    let auto_gain = dj_analysis::loudness::analyse(path)
        .map(|loudness| loudness.gain)
        .unwrap_or(1.0);

    Ok(PreparedTrack {
        deck: DeckId::A,
        source: Box::new(prefetcher),
        track: Some(track),
        static_info,
        path: Some(path.to_path_buf()),
        // What the file remembers is read by the caller: this function knows samples, not sessions.
        cues: Vec::new(),
        loop_region: None,
        auto_gain,
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
    /// Where an edit goes. The callback only sends; the writing happens on another thread, because
    /// a disk call in the audio path is the one thing DJP-NFR-001 forbids outright.
    meta: Option<std::sync::mpsc::Sender<MetaEdit>>,
    /// Path of the file on each deck, in deck order.
    paths: Vec<Option<std::path::PathBuf>>,
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
        let decks_len = decks.len();
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
            meta: None,
            paths: vec![None; decks_len],
        }
    }

    /// Send cue and loop edits to `sink`. Called before the stream starts: the callback never locks.
    pub fn with_meta_sink(mut self, sink: std::sync::mpsc::Sender<MetaEdit>) -> Self {
        self.meta = Some(sink);
        self
    }

    /// Hand the deck's cues and loop to the writer when a command changed them.
    ///
    /// Cheap on purpose: a lookup, a copy of eight slots and a send on a channel that never blocks,
    /// because this runs inside the callback.
    fn note_edit(&self, command: &ScheduledCommand) {
        let Some(sink) = self.meta.as_ref() else {
            return;
        };
        let deck_id = match &command.command {
            DjCommand::Cue { deck, .. } | DjCommand::SetLoop { deck, .. } => *deck,
            _ => return,
        };
        let Some(index) = self.decks.iter().position(|deck| deck.id() == deck_id) else {
            return;
        };
        let Some(path) = self.paths[index].clone() else {
            return;
        };
        let deck = &self.decks[index];
        let cues: Vec<(u8, u64)> = deck
            .hotcues()
            .iter()
            .enumerate()
            .filter_map(|(slot, frame)| frame.map(|frame| (slot as u8, frame)))
            .collect();
        let loop_region = deck
            .loop_region()
            .map(|region| (region.start.0, region.end.0));
        let _ = sink.send(MetaEdit {
            path,
            cues,
            loop_region,
        });
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
                        // What the file remembered goes back on the deck, so a reload looks like a
                        // reload and not like a fresh track (DJP-DATA-006).
                        for (slot, frame) in prepared.cues {
                            self.decks[index].set_hotcue(usize::from(slot), frame);
                        }
                        self.decks[index].set_loop(prepared.loop_region);
                        // The track opens at its own level: a hot master and a quiet one then start
                        // from the same place, and a +6 dB EQ boost has room instead of the limiter.
                        if prepared.auto_gain > 0.0 {
                            self.decks[index].set_gain(prepared.auto_gain);
                        }
                        self.paths[index] = prepared.path;
                        self.loads_applied += 1;
                    }
                }
                GraphLoad::Clear { deck } => {
                    if let Some(index) = self
                        .decks
                        .iter()
                        .position(|candidate| candidate.id() == deck)
                    {
                        self.paths[index] = None;
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
            // A cue or a loop is a decision the user made about this file, so it outlives the
            // session. The callback only hands the edit over; the writing thread does the disk work.
            self.note_edit(&submission.command);
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
