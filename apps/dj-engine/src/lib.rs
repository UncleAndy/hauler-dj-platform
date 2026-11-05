//! The engine's own logic, separate from the process that hosts it.
//!
//! The binary owns threads, sockets and signals; everything that decides what a command does and
//! what a snapshot says lives here, so it can be tested without spawning a process. That
//! distinction came from a hanging integration test: spawning a server inside a test binary is a
//! fragile way to check a rule.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use dj_deck::Deck;
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
        | DjCommand::SetTempoRatio { deck, .. } => Some(*deck),
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
