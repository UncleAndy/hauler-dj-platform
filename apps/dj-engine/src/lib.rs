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
use dj_codec::{PcmCache, PrefetchConfig, Prefetcher};
use dj_deck::{Deck, DeckSource};
use dj_domain::CommandId;
use dj_domain::{
    Ack, AckStatus, BeatGrid, BeatSegment, CueAction, DeckId, DeckLoad, DeckSnapshot, DjCommand,
    EqGains, FrameRange, GridEdit, Levels, RejectReason, ScheduledCommand, Snapshot, SourceFrame,
    TrackMeta,
};
use dj_mixer::{ChannelSettings, CrossAssign, CrossfaderCurve, Mixer};
use dj_sync::{DeckClock, Phase, PitchLimits, RateTrim, Reading, SyncMode, plan as plan_sync};

pub mod meta;
pub mod metronome;

/// What a pad carries besides its frame: a name and a colour, each optional.
type CuePad = (Option<String>, Option<String>);

/// One deck's eight pads.
type DeckPads = [CuePad; HOTCUE_SLOTS];

/// A loop on a pad, as frames of the source file.
type PadLoop = Option<(u64, u64)>;

/// One deck's eight loop pads.
type DeckLoops = [PadLoop; HOTCUE_SLOTS];

pub fn apply(decks: &mut [Deck], mixer: &mut Mixer, command: &ScheduledCommand, clock: u64) -> Ack {
    let id = command.id;
    let at = command.effective_sample.max(clock);

    // Everything addressed to a deck resolves the target once, so an unknown deck is reported the
    // same way for every command instead of once per variant.
    let deck_id = match &command.command {
        DjCommand::Play { deck, .. }
        | DjCommand::Cue { deck, .. }
        | DjCommand::SetCueMeta { deck, .. }
        | DjCommand::SetPadLoop { deck, .. }
        | DjCommand::SetLoopMode { deck, .. }
        | DjCommand::SeekFrames { deck, .. }
        | DjCommand::SetLoop { deck, .. }
        | DjCommand::LoopBeats { deck, .. }
        | DjCommand::LoopMove { deck, .. }
        | DjCommand::SetTempoRatio { deck, .. }
        | DjCommand::Nudge { deck, .. }
        | DjCommand::BeatJump { deck, .. }
        | DjCommand::SetSlip { deck, .. }
        | DjCommand::SetReverse { deck, .. }
        | DjCommand::Sync { deck, .. }
        | DjCommand::Metronome { deck, .. }
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
            DjCommand::SetLoopMode { enabled, .. } => {
                target.set_loop_mode(*enabled);
                Ack::applied(id, at)
            }
            // A click is a reference beside the music, so the decision lives on the deck and the
            // generator lives beside the mix: the deck knows its grid, the graph knows the sample.
            DjCommand::Metronome { enabled, .. } => {
                target.set_metronome(*enabled);
                Ack::applied(id, at)
            }
            DjCommand::SetPadLoop { slot, .. } => {
                let region = target.loop_region();
                if target.set_pad_loop(usize::from(*slot), region) {
                    Ack::applied(id, at)
                } else {
                    Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::InvalidValue),
                    }
                }
            }
            DjCommand::SetCueMeta {
                slot, label, color, ..
            } => {
                if target.set_cue_meta(usize::from(*slot), label.clone(), color.clone()) {
                    Ack::applied(id, at)
                } else {
                    Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::InvalidValue),
                    }
                }
            }
            DjCommand::Cue { action, .. } => {
                let done = match *action {
                    CueAction::Set(slot) => target.store_hotcue(usize::from(slot)),
                    CueAction::Delete(slot) => {
                        target.delete_hotcue(usize::from(slot));
                        true
                    }
                    CueAction::Jump(slot) => target.jump_pad(usize::from(slot)).is_some(),
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
                // A loop that would sit on another one is refused here, not drawn and then ignored:
                // loops never overlap, so the request has no meaning and the DJ has to know.
                if target.set_loop(*region) {
                    Ack::applied(id, at)
                } else {
                    Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::InvalidValue),
                    }
                }
            }
            // A loop of a musical length (DJP-DECK-005): built from the grid, so the seam is on the bar
            // rather than on the frame the hand happened to hit. The play head steps into the new loop
            // when it is outside it — pressing "4" is a request to hear those four beats.
            DjCommand::LoopBeats { beats, .. } => {
                let at = target.position();
                match target.loop_by_beats(at, *beats) {
                    Some(region) => {
                        if !region.contains(SourceFrame(target.position())) {
                            target.seek(region.start.0);
                        }
                        Ack::applied(id, at)
                    }
                    None => Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::InvalidValue),
                    },
                }
            }
            // Moving the active loop keeps its length and takes the play head with it: a loop that moves
            // out from under the sound is a jump, and a jump is not what was asked for.
            DjCommand::LoopMove { beats, .. } => match target.move_loop(*beats) {
                Some(region) => {
                    if !region.contains(SourceFrame(target.position())) {
                        target.seek(region.start.0);
                    }
                    Ack::applied(id, at)
                }
                None => Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                },
            },
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
            // A hand on the platter: the deck bends its own rate, the fader keeps its value, and the
            // state says the bend is in force. The deck clamps the deflection, so a client cannot ask
            // for more bend than the range promises.
            DjCommand::Nudge { amount, .. } => {
                if amount.is_finite() {
                    target.set_nudge(f64::from(*amount));
                    Ack::applied(id, at)
                } else {
                    Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::InvalidValue),
                    }
                }
            }
            DjCommand::SetSlip { enabled, .. } => {
                target.set_slip(*enabled);
                Ack::applied(id, at)
            }
            DjCommand::Sync { mode, .. } => {
                target.set_sync(*mode);
                Ack::applied(id, at)
            }
            // Reverse walks the material backwards from the playhead. A track still streaming from disk
            // has no cheap seek, so the deck refuses — and the client is told, rather than left waiting
            // for a callback that would have to block on a decoder.
            DjCommand::SetReverse { enabled, .. } => {
                if target.set_reverse(*enabled) == *enabled {
                    Ack::applied(id, at)
                } else {
                    Ack {
                        command_id: id,
                        status: AckStatus::Rejected(RejectReason::NotSupported),
                    }
                }
            }
            // A jump is measured on the deck's own grid: the DJ asks for beats, not for frames, and a
            // deck without a grid has no beats to give — that refusal is the honest answer.
            DjCommand::BeatJump {
                beats, quantize, ..
            } => match target.beat_jump(*beats, *quantize) {
                Some(_) => Ack::applied(id, at),
                None => Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                },
            },
            _ => unreachable!("only deck commands reach here"),
        };
    }

    match &command.command {
        // One master at a time: making a deck lead takes the flag off every other deck, so two decks
        // can never both claim to be the beat everything else follows (DJP-SYNC-001).
        DjCommand::MasterOff => {
            for deck in decks.iter_mut() {
                deck.set_master(false);
                // Following nothing is not a speed: the deck keeps the tempo it was given and plays on.
                if !deck.sync().is_off() {
                    deck.set_sync(SyncMode::Off);
                }
            }
            Ack::applied(id, at)
        }
        DjCommand::Master { deck: chosen } => {
            if !decks.iter().any(|candidate| candidate.id() == *chosen) {
                return Ack {
                    command_id: id,
                    status: AckStatus::Rejected(RejectReason::InvalidValue),
                };
            }
            for deck in decks.iter_mut() {
                deck.set_master(deck.id() == *chosen);
            }
            Ack::applied(id, at)
        }
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
    let mut published_decks: Vec<DeckSnapshot> = decks.iter().map(Deck::snapshot).collect();
    fill_phase_errors(&mut published_decks, beats_per_bar(decks));
    Snapshot {
        protocol: dj_domain::STATE_PROTOCOL.to_string(),
        // A snapshot taken from the mixer alone says nothing about the loader's cache.
        cache_budget_bytes: 0,
        revision,
        epoch,
        audio_clock_samples: clock,
        sample_rate: rate,
        crossfader: mixer.crossfader(),
        crossfader_curve: mixer.curve(),
        master: dj_domain::Levels {
            peak_left: mixer.meters().master.peak,
            peak_right: mixer.meters().master.peak,
            held_left: mixer.meters().master.held,
            held_right: mixer.meters().master.held,
        },
        master_gain: mixer.master_gain(),
        master_latency_frames: mixer.latency_frames(),
        decks: published_decks
            .into_iter()
            .enumerate()
            .map(|(index, mut published)| {
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
    /// Correction measured from the file's loudness, linear; applied with the trim, shown beside the
    /// clock, and never written into the knob.
    pub auto_gain: f32,
    /// The same correction in decibels, `None` when the level of this file was never measured.
    pub auto_gain_db: Option<f32>,
    /// True while the tempo of this deck's file is being measured again (B02).
    pub analyzing: bool,
    /// The beat grid in source frames, one segment per tempo (DJP-SYNC-003).
    pub grid: Option<BeatGrid>,
    /// Sample rate of the source material: the timebase the grid is expressed in.
    pub source_rate: u32,
    /// The file the deck holds, as it was opened. A client that reconnects does not load anything and
    /// therefore does not know this path; the engine does, and an edit can name the deck alone.
    pub path: Option<String>,
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
            auto_gain: snapshot.auto_gain,
            auto_gain_db: snapshot.auto_gain_db,
            analyzing: snapshot.analyzing,
            grid: snapshot.track.as_ref().and_then(|track| track.grid.clone()),
            source_rate: snapshot.source_rate,
            // The snapshot says nothing about the file it came from: only a load knows that.
            path: None,
        }
    }
}

/// Per-deck scalar state, published without a lock.
struct DeckCells {
    playing: AtomicBool,
    /// Whether the metronome clicks this deck's grid.
    metronome: AtomicBool,
    /// Clicks played since start: the proof that a click reached the audio path.
    clicks: AtomicU64,
    // Whether a source is attached. This one belongs here, not in the static section: eject changes
    // it, and a snapshot that kept saying "Ready" kept a track on screen that was no longer in the
    // deck.
    attached: AtomicBool,
    frame: AtomicU64,
    gain: AtomicU32,
    tempo: AtomicU64,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    held_left: AtomicU32,
    held_right: AtomicU32,
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
    // The nudge in force, `-1 … 1`, so the state can say the hand is still on the platter.
    nudge: AtomicU32,
    // Slip and its virtual head: the ghost marker a client draws beside the play head.
    slip: AtomicBool,
    slip_frame: AtomicU64,
    reverse: AtomicBool,
    /// Blocks this deck asked its source for and did not get: a starved deck is not a drifted one.
    underruns: AtomicU64,
    cached: AtomicBool,
    cached_bytes: AtomicU64,
    // What the deck follows (DJP-SYNC-002), as one byte: off, tempo, beat.
    sync: AtomicU8,
    /// The follower's distance from the master's beat, in milliseconds, measured at the end of a render
    /// block where both decks are in hand. `u32::MAX` means it has not been measured yet.
    phase_error_ms: AtomicU32,
    // Whether the deck leads the others (DJP-SYNC-001).
    master: AtomicBool,
}

impl DeckCells {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            metronome: AtomicBool::new(false),
            clicks: AtomicU64::new(0),
            attached: AtomicBool::new(false),
            frame: AtomicU64::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            tempo: AtomicU64::new(1.0f64.to_bits()),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            held_left: AtomicU32::new(0),
            held_right: AtomicU32::new(0),
            loop_start: AtomicU64::new(EMPTY),
            loop_end: AtomicU64::new(EMPTY),
            hotcues: std::array::from_fn(|_| AtomicU64::new(EMPTY)),
            eq_low: AtomicU32::new(0),
            eq_mid: AtomicU32::new(0),
            eq_high: AtomicU32::new(0),
            fader: AtomicU32::new(1.0f32.to_bits()),
            nudge: AtomicU32::new(0.0f32.to_bits()),
            slip: AtomicBool::new(false),
            slip_frame: AtomicU64::new(EMPTY),
            reverse: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            cached: AtomicBool::new(false),
            cached_bytes: AtomicU64::new(0),
            sync: AtomicU8::new(0),
            phase_error_ms: AtomicU32::new(u32::MAX),
            master: AtomicBool::new(false),
        }
    }
}

/// The sync mode as one byte, so the render thread can publish it without allocating.
fn sync_index(mode: SyncMode) -> u8 {
    match mode {
        SyncMode::Off => 0,
        SyncMode::Tempo => 1,
        SyncMode::Beat => 2,
    }
}

/// The mode a published byte stands for; anything unknown is `off`, because a client cannot have asked
/// for a mode this build does not have.
fn sync_from_index(index: u8) -> SyncMode {
    match index {
        1 => SyncMode::Tempo,
        2 => SyncMode::Beat,
        _ => SyncMode::Off,
    }
}

/// Every following deck's distance from the master's beat, measured where both decks are in hand.
///
/// A phase is a distance between two positions at *one* instant. Measured later, from a snapshot, it is a
/// distance between two positions taken from different render blocks, and one block is 5,3 ms at 256
/// frames: a deck standing on the beat then reads as five milliseconds off, at random, and a soak of
/// perfectly good behaviour looks broken. So the number is taken here, at the end of the block, where the
/// master and the follower have both been advanced.
fn publish_phase_errors(decks: &[Deck], cells: &[DeckCells]) {
    let master = decks.iter().position(Deck::master).or_else(|| {
        decks
            .iter()
            .position(|deck| deck.is_playing() && deck.beat_grid().is_some())
    });
    let Some(master_index) = master else {
        return;
    };
    let Some(leading) = master_reading(&decks[master_index]) else {
        return;
    };
    for (index, deck) in decks.iter().enumerate() {
        let Some(cell) = cells.get(index) else {
            return;
        };
        if index == master_index || deck.sync().is_off() {
            cell.phase_error_ms.store(u32::MAX, Ordering::Relaxed);
            continue;
        }
        let Some(reading) = master_reading(deck) else {
            cell.phase_error_ms.store(u32::MAX, Ordering::Relaxed);
            continue;
        };
        let beats = dj_sync::error_beats(reading.phase, leading.phase);
        let ms = beats * 60_000.0 / leading.tempo_bpm;
        #[allow(clippy::cast_possible_truncation)]
        let ms = ms as f32;
        cell.phase_error_ms.store(ms.to_bits(), Ordering::Relaxed);
    }
}

/// Blocks between two sync steps: at 256 frames a block this is a step every 43 ms, which is the tick
/// the planner was measured at.
const SYNC_BLOCKS: u32 = 8;

/// Every following deck's distance from the master's beat, in milliseconds (DJP-SYNC-002).
///
/// A phase is a distance between two decks at one instant, so it is filled in where both are in hand:
/// by the publisher for the wire, and by the free snapshot for tests. Positive means the deck has to
/// move forward to stand on the master's beat.
pub fn fill_phase_errors(decks: &mut [DeckSnapshot], beats_per_bar: u8) {
    let master = decks.iter().position(|deck| deck.master).or_else(|| {
        decks
            .iter()
            .position(|deck| deck.playing && deck.beat.is_some())
    });
    let Some(master_index) = master else {
        return;
    };
    let Some((master_beat, master_bpm)) = beat_of(&decks[master_index]) else {
        return;
    };
    let master_phase = Phase::of(master_beat);
    let bar = f64::from(beats_per_bar.max(1));
    for (index, deck) in decks.iter_mut().enumerate() {
        if index == master_index || deck.sync.is_off() {
            continue;
        }
        // A deck that stands still publishes its distance too, and the number moves as the master plays:
        // that is what it means. "Where the head stands against the master's beat right now" is the
        // question a hand asks before pressing play, and a meter parked in the centre answers it wrongly
        // — it reads as "aligned" over a deck that is a beat and a half off. The loop never acts on this
        // number for a stopped deck; it is a reading, not an instruction.
        if let Some((beat, _)) = beat_of(deck) {
            deck.phase_error_ms =
                Some(dj_sync::error_beats(Phase::of(beat), master_phase) * 60_000.0 / master_bpm);
            #[allow(clippy::cast_possible_truncation)]
            {
                deck.bar_error_beats =
                    Some(dj_sync::bar_error_beats(beat, master_beat, bar) as f32);
            }
        }
    }
}

/// Bar length of the deck that leads: the bar is a property of the music the master is playing.
fn beats_per_bar(decks: &[Deck]) -> u8 {
    let master = decks
        .iter()
        .find(|deck| deck.master())
        .or_else(|| {
            decks
                .iter()
                .find(|deck| deck.is_playing() && deck.beat_grid().is_some())
        })
        .or_else(|| decks.iter().find(|deck| deck.beat_grid().is_some()));
    master
        .and_then(dj_deck::Deck::beat_grid)
        .map_or(4, |grid| grid.beats_per_bar())
}

/// Phase and heard tempo of a deck, when it has a beat to speak of.
fn beat_of(deck: &DeckSnapshot) -> Option<(f64, f64)> {
    let beat = deck.beat?;
    let bpm = deck.local_bpm?;
    let heard = bpm * deck.tempo_ratio;
    (heard.is_finite() && heard > 0.0).then_some((beat, heard))
}

/// What the sync loop remembers between two steps (DJP-SYNC-002).
///
/// A jump does not land in the same block it is asked for: a deck reading from disk is silent until its
/// decoder catches up, and asking again before that happens keeps it silent for good. So the loop
/// remembers how long ago each deck was moved, and waits.
#[derive(Debug, Clone)]
pub struct SyncMemory {
    steps_since_jump: [u16; MAX_DECKS],
    /// The mode each deck had at the last step: a mode that has just come on is a hand asking for the
    /// tempo now, not a deck that is still trimming its way there.
    last_mode: [SyncMode; MAX_DECKS],
    /// The file tempo each deck was last matched against. A deck under a new track has a new tempo, and
    /// the ratio of the track before it is a wrong speed, not a starting point.
    matched_bpm: [f64; MAX_DECKS],
}

impl Default for SyncMemory {
    /// A deck that was never moved may be moved at once: the wait exists for a jump that is still
    /// landing, and at the start there is none.
    fn default() -> Self {
        Self {
            steps_since_jump: [Self::QUIET_STEPS; MAX_DECKS],
            last_mode: [SyncMode::Off; MAX_DECKS],
            matched_bpm: [0.0; MAX_DECKS],
        }
    }
}

impl SyncMemory {
    /// Steps to wait after a jump: at one step per 43 ms this is half a second. A seek needs that long
    /// before the material arrives and the time-stretch stage settles; a jump asked for inside that
    /// window measures a transient and lands on a deck that has not yet played from the new place.
    const QUIET_STEPS: u16 = 12;

    /// Whether a deck may be moved onto the beat now.
    fn allows_jump(&self, index: usize) -> bool {
        self.steps_since_jump[index] >= Self::QUIET_STEPS
    }

    /// Remember that a deck was just moved.
    fn note_jump(&mut self, index: usize) {
        self.steps_since_jump[index] = 0;
    }

    /// One control step has passed.
    fn tick(&mut self) {
        for steps in &mut self.steps_since_jump {
            *steps = steps.saturating_add(1);
        }
    }
}

/// Which deck leads: the one a hand made master, else the first playing deck with a beat, else the
/// first deck that has one. Deterministic, so the phase meter cannot flicker between two masters.
/// What the master's beat is, read once: the followers are compared against one instant.
fn master_reading(deck: &Deck) -> Option<Reading> {
    let grid = deck.beat_grid()?;
    let rate = f64::from(deck.source_rate());
    if rate <= 0.0 {
        return None;
    }
    let beat = grid.beat_at(deck.position(), rate);
    let bpm = grid.bpm_at(deck.position()) * deck.tempo_ratio();
    bpm.is_finite().then_some(Reading {
        name: "the master deck",
        tempo_bpm: bpm,
        phase: Phase::of(beat),
        age_ms: 0.0,
        // A paused deck still has a beat: it is the beat the DJ lined the next track up against, and
        // a sync that dies whenever the master is stopped is a sync nobody can trust at the moment
        // it is needed. A file with no grid is the case that has nothing to follow, and it has
        // already returned above.
        running: true,
    })
}

/// One step of the sync control loop: match the followers' tempo, and stand them on the master's beat
/// when that is what they were asked for (DJP-SYNC-001/002).
///
/// Pure arithmetic on decks: no allocation, no locks, no I/O, so it runs inside the audio callback. It
/// is idempotent — a second step moves the tempo no further than the trim allows and a deck already on
/// the beat asks for no seek.
pub fn sync_step(decks: &mut [Deck], memory: &mut SyncMemory, seconds: f64) {
    // A tempo match is not a fader: the sync reaches the master's tempo whatever the fader's travel.
    let limits = PitchLimits::beyond_the_fader();
    let trim = RateTrim::default();
    memory.tick();
    // Resolved before the loop and per follower: the master is a deck, and a deck cannot lead itself.
    // Fixed arrays and no allocation: this runs in the audio callback, where a Vec is a defect measured
    // by `the_callback_path_does_not_allocate`.
    let explicit = decks.iter().position(|deck| deck.master());
    let mut readings: [Option<(Reading, bool)>; MAX_DECKS] = [None; MAX_DECKS];
    for (index, deck) in decks.iter().enumerate().take(MAX_DECKS) {
        if deck.has_source() && deck.beat_grid().is_some() {
            // A stopped deck still says what the tempo is, and following its speed is legal. Its beat,
            // though, stands still: a follower pulled onto a beat that is not moving is pulled back
            // every step, and a deck asked to seek four times a second never gets to play its material.
            // So the beat is only followed while the master is running.
            readings[index] = master_reading(deck).map(|reading| (reading, deck.is_playing()));
        }
    }
    for (index, deck) in decks.iter_mut().enumerate() {
        let leader = explicit.filter(|master| *master != index).or_else(|| {
            (0..readings.len()).find(|master| *master != index && readings[*master].is_some())
        });
        let Some((master, master_running)) =
            leader.and_then(|master| readings.get(master).copied().flatten())
        else {
            continue;
        };
        let mode = deck.sync();
        // A deck that has just been put into sync, or has just had a track loaded under it, is a deck
        // that needs the tempo now: the trim is how a tempo is held, not how it is reached.
        let fresh = !mode.is_off() && memory.last_mode[index] != mode;
        memory.last_mode[index] = mode;
        if mode.is_off() || !deck.has_source() {
            continue;
        }
        // The plan is made while the deck is read only; the deck is changed after it, so a refusal can
        // leave it exactly as it was.
        let planned = {
            let Some(grid) = deck.beat_grid() else {
                continue;
            };
            let follower = DeckClock {
                grid,
                source_rate: f64::from(deck.source_rate()),
                frame: deck.position(),
                ratio: deck.tempo_ratio(),
            };
            plan_sync(&master, &follower, mode, limits)
        };
        let Ok(planned) = planned else {
            continue;
        };
        let new_track = (memory.matched_bpm[index] - planned.follower_bpm).abs() > 0.01;
        memory.matched_bpm[index] = planned.follower_bpm;
        if fresh || new_track {
            // The hand asked for this tempo, or the file under the deck changed: the deck goes to the
            // master's tempo in one piece.
            deck.set_tempo_ratio(planned.ratio);
        } else {
            // Holding a tempo is another matter: the trim's travel is slow enough to be inaudible and
            // fast enough to land inside a bar.
            let next = trim.step(deck.tempo_ratio(), planned.ratio, seconds);
            deck.set_tempo(next);
        }
        // A beat is a jump, not a pull: the hand asked for the beat, and a slow correction is heard as a
        // deck losing the mix. One frame is below what anybody can hear, so it is not worth a seek.
        // A deck that is not playing is not chasing anything. A stopped follower next to a playing
        // master has a phase that runs away from it at the master's tempo, and jumping onto that beat
        // every half second drags the play head backwards in steps a person can both see and hear. It
        // is aligned on the first step after it starts playing, and not before.
        if mode == SyncMode::Beat
            && master_running
            && deck.is_playing()
            && memory.allows_jump(index)
            && let Some(jump) = planned.jump
        {
            // One millisecond is the smallest error a person can hear in a mix, and a deck cannot be
            // moved by less than a frame: below the band a jump is noise, not a correction.
            let audible = f64::from(deck.source_rate()) / 1000.0;
            if jump.frames.unsigned_abs() as f64 > audible {
                let target = i128::from(deck.position()) + i128::from(jump.frames);
                if let Ok(target) = u64::try_from(target.max(0)) {
                    deck.seek(target);
                    memory.note_jump(index);
                }
            }
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
    /// Beats in the bar of the music that leads; four until a deck says otherwise.
    beats_per_bar: AtomicU8,
    master_held: AtomicU32,
    master_latency: AtomicU32,
    master_gain: AtomicU32,
    count: AtomicUsize,
    /// Bytes the whole-track cache may hold: the whole a deck's cost is a fraction of.
    cache_budget: AtomicU64,
    decks: [DeckCells; MAX_DECKS],
    /// Hotcue names and colours, per deck and slot.
    ///
    /// Strings cannot live in an atomic cell, and a name changes once in a while rather than every
    /// block: the command thread writes here, the publisher thread reads. Neither is the callback.
    cue_meta: std::sync::Mutex<Vec<DeckPads>>,
    /// Loops on pads, per deck: a range is not an atomic word either.
    pad_loops: std::sync::Mutex<Vec<DeckLoops>>,
    /// Loops loop mode is watching, per deck: as many as the track has, not one.
    /// Whether loop mode repeats each deck's loops, one flag per deck.
    loop_mode: std::sync::Mutex<Vec<bool>>,
    active_loops: std::sync::Mutex<Vec<Vec<(u64, u64)>>>,
}

impl Default for StateCells {
    fn default() -> Self {
        Self::new()
    }
}

impl StateCells {
    /// Put a hotcue's name and colour where the publisher will find them.
    pub fn set_cue_meta(
        &self,
        deck: usize,
        slot: usize,
        label: Option<String>,
        color: Option<String>,
    ) {
        if slot >= HOTCUE_SLOTS {
            return;
        }
        let mut guard = self
            .cue_meta
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while guard.len() <= deck {
            guard.push(std::array::from_fn(|_| (None, None)));
        }
        if let Some(name) = label {
            guard[deck][slot].0 = Some(name);
        }
        if let Some(hex) = color {
            guard[deck][slot].1 = Some(hex);
        }
    }

    /// Put a loop on a pad where the publisher will find it.
    pub fn set_pad_loop(&self, deck: usize, slot: usize, region: PadLoop) {
        if slot >= HOTCUE_SLOTS {
            return;
        }
        let mut guard = self
            .pad_loops
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while guard.len() <= deck {
            guard.push(std::array::from_fn(|_| None));
        }
        guard[deck][slot] = region;
    }

    /// Put the mode where the publisher will find it: the Loop button draws this, not a copy.
    pub fn set_loop_mode(&self, deck: usize, on: bool) {
        let mut guard = self
            .loop_mode
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while guard.len() <= deck {
            guard.push(false);
        }
        guard[deck] = on;
    }

    /// What the publisher reads for one deck.
    fn loop_mode_of(&self, deck: usize) -> bool {
        let guard = self
            .loop_mode
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.get(deck).copied().unwrap_or(false)
    }

    /// Put the armed loops where the publisher will find them.
    pub fn set_active_loops(&self, deck: usize, regions: Vec<(u64, u64)>) {
        let mut guard = self
            .active_loops
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while guard.len() <= deck {
            guard.push(Vec::new());
        }
        guard[deck] = regions;
    }

    /// What the publisher reads for one deck.
    fn active_loops_of(&self, deck: usize) -> Vec<(u64, u64)> {
        let guard = self
            .active_loops
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.get(deck).cloned().unwrap_or_default()
    }

    /// Forget a deck's loops: a new track brings its own.
    pub fn clear_pad_loops(&self, deck: usize) {
        let mut guard = self
            .pad_loops
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(slots) = guard.get_mut(deck) {
            *slots = std::array::from_fn(|_| None);
        }
    }

    /// What the publisher reads for one deck.
    fn pad_loops_of(&self, deck: usize) -> DeckLoops {
        let guard = self
            .pad_loops
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard
            .get(deck)
            .cloned()
            .unwrap_or_else(|| std::array::from_fn(|_| None))
    }

    /// Write one pad's name and colour as the deck holds it now, including "nothing at all".
    ///
    /// `set_cue_meta` cannot do this: there `None` means "leave this half alone", which is what a rename
    /// needs. Clearing a pad needs the opposite — the client must stop showing a name that is gone.
    pub fn write_cue_meta(
        &self,
        deck: usize,
        slot: usize,
        label: Option<String>,
        color: Option<String>,
    ) {
        if slot >= HOTCUE_SLOTS {
            return;
        }
        let mut guard = self
            .cue_meta
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while guard.len() <= deck {
            guard.push(std::array::from_fn(|_| (None, None)));
        }
        guard[deck][slot] = (label, color);
    }

    /// Forget a deck's names and colours: a new track brings its own.
    pub fn clear_cue_meta(&self, deck: usize) {
        let mut guard = self
            .cue_meta
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(slots) = guard.get_mut(deck) {
            *slots = std::array::from_fn(|_| (None, None));
        }
    }

    /// What the publisher reads for one deck.
    fn cue_meta_of(&self, deck: usize) -> DeckPads {
        let guard = self
            .cue_meta
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard
            .get(deck)
            .cloned()
            .unwrap_or_else(|| std::array::from_fn(|_| (None, None)))
    }

    /// Empty state, room for [`MAX_DECKS`] decks.
    pub fn new() -> Self {
        Self {
            clock: AtomicU64::new(0),
            crossfader: AtomicU32::new(0.5f32.to_bits()),
            curve: AtomicU8::new(curve_index(CrossfaderCurve::default())),
            master_peak: AtomicU32::new(0),
            beats_per_bar: AtomicU8::new(4),
            master_held: AtomicU32::new(0),
            master_latency: AtomicU32::new(0),
            master_gain: AtomicU32::new(1.0f32.to_bits()),
            count: AtomicUsize::new(0),
            cache_budget: AtomicU64::new(0),
            decks: std::array::from_fn(|_| DeckCells::new()),
            cue_meta: std::sync::Mutex::new(Vec::new()),
            pad_loops: std::sync::Mutex::new(Vec::new()),
            loop_mode: std::sync::Mutex::new(Vec::new()),
            active_loops: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Publish the scalars. No allocation, no lock, no syscall: callable from a callback.
    /// Record the cache budget: the snapshot says how full the cache is, and "full" needs a whole.
    pub fn set_cache_budget(&self, bytes: u64) {
        self.cache_budget.store(bytes, Ordering::Relaxed);
    }

    /// Record the bar length of the music that leads: the publisher works from the cells and cannot see
    /// a grid, and the bar is what the phase meter draws.
    pub fn set_beats_per_bar(&self, beats: u8) {
        self.beats_per_bar.store(beats, Ordering::Relaxed);
    }

    pub fn publish(&self, decks: &[Deck], mixer: &Mixer, clock: u64) {
        self.clock.store(clock, Ordering::Relaxed);
        // The phases first, at the instant the block ended. Everything else here is a reading of one deck;
        // a phase is a distance between two, and it can only be honest while both are in hand.
        publish_phase_errors(decks, &self.decks);
        self.crossfader
            .store(mixer.crossfader().to_bits(), Ordering::Relaxed);
        self.curve
            .store(curve_index(mixer.curve()), Ordering::Relaxed);
        self.master_held
            .store(mixer.meters().master.held.to_bits(), Ordering::Relaxed);
        self.master_latency
            .store(mixer.latency_frames(), Ordering::Relaxed);
        self.master_peak
            .store(mixer.meters().master.peak.to_bits(), Ordering::Relaxed);
        self.master_gain
            .store(mixer.master_gain().to_bits(), Ordering::Relaxed);
        let count = decks.len().min(MAX_DECKS);
        self.count.store(count, Ordering::Release);
        for (index, deck) in decks.iter().take(MAX_DECKS).enumerate() {
            let cells = &self.decks[index];
            cells.playing.store(deck.is_playing(), Ordering::Relaxed);
            cells.metronome.store(deck.metronome(), Ordering::Relaxed);
            cells.clicks.store(deck.clicks(), Ordering::Relaxed);
            cells
                .underruns
                .store(deck.stats().underruns, Ordering::Relaxed);
            cells.attached.store(deck.has_source(), Ordering::Relaxed);
            cells.frame.store(deck.position(), Ordering::Relaxed);
            cells.gain.store(deck.gain().to_bits(), Ordering::Relaxed);
            cells
                .tempo
                .store(deck.tempo_ratio().to_bits(), Ordering::Relaxed);
            #[allow(clippy::cast_possible_truncation)]
            cells
                .nudge
                .store((deck.nudge() as f32).to_bits(), Ordering::Relaxed);
            cells.slip.store(deck.slip(), Ordering::Relaxed);
            cells.reverse.store(deck.reverse(), Ordering::Relaxed);
            cells.sync.store(sync_index(deck.sync()), Ordering::Relaxed);
            cells.master.store(deck.master(), Ordering::Relaxed);
            cells
                .cached
                .store(deck.reads_from_memory(), Ordering::Relaxed);
            cells
                .cached_bytes
                .store(deck.memory_bytes().unwrap_or(0), Ordering::Relaxed);
            cells
                .slip_frame
                .store(deck.slip_frame().unwrap_or(EMPTY), Ordering::Relaxed);
            let levels = deck.levels();
            cells
                .peak_left
                .store(levels.peak_left.to_bits(), Ordering::Relaxed);
            cells
                .peak_right
                .store(levels.peak_right.to_bits(), Ordering::Relaxed);
            cells
                .held_left
                .store(levels.held_left.to_bits(), Ordering::Relaxed);
            cells
                .held_right
                .store(levels.held_right.to_bits(), Ordering::Relaxed);
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
            // Where the play head is in beats, and the tempo there: a track that changes tempo has no
            // single number to show, and a client must not have to guess which segment it is looking at.
            let (beat, local_bpm) = match fixed.grid.as_ref() {
                Some(grid) if attached && fixed.source_rate > 0 => {
                    let grid_rate = f64::from(fixed.source_rate);
                    (
                        Some(grid.beat_at(frame, grid_rate)),
                        Some(grid.bpm_at(frame)),
                    )
                }
                _ => (None, None),
            };
            decks.push(DeckSnapshot {
                underruns: cells.underruns.load(Ordering::Relaxed),
                deck: fixed.deck,
                load,
                playing: cells.playing.load(Ordering::Relaxed),
                metronome: cells.metronome.load(Ordering::Relaxed),
                clicks: cells.clicks.load(Ordering::Relaxed),
                beat,
                local_bpm,
                source_rate: fixed.source_rate,
                path: if attached { fixed.path.clone() } else { None },
                source_frame: SourceFrame(frame),
                remaining_frames: total.map(|total| total.saturating_sub(frame)),
                position_ratio: total
                    .filter(|total| *total > 0)
                    .map(|total| (frame as f32 / total as f32).clamp(0.0, 1.0)),
                track: if attached { fixed.track.clone() } else { None },
                analyzing: fixed.analyzing,
                waveform: if attached {
                    fixed.waveform.clone()
                } else {
                    None
                },
                loop_region: (loop_start != EMPTY && loop_end != EMPTY).then_some(FrameRange {
                    start: SourceFrame(loop_start),
                    end: SourceFrame(loop_end),
                }),
                loop_mode: self.loop_mode_of(index),
                active_loops: self
                    .active_loops_of(index)
                    .into_iter()
                    .map(|(start, end)| FrameRange {
                        start: SourceFrame(start),
                        end: SourceFrame(end),
                    })
                    .collect(),
                pad_loops: {
                    let loops = self.pad_loops_of(index);
                    loops
                        .iter()
                        .map(|region| {
                            region.map(|(start, end)| FrameRange {
                                start: SourceFrame(start),
                                end: SourceFrame(end),
                            })
                        })
                        .collect()
                },
                cue_labels: {
                    let meta = self.cue_meta_of(index);
                    meta.iter().map(|(label, _)| label.clone()).collect()
                },
                cue_colors: {
                    let meta = self.cue_meta_of(index);
                    meta.iter().map(|(_, color)| color.clone()).collect()
                },
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
                    held_left: f32::from_bits(cells.held_left.load(Ordering::Relaxed)),
                    held_right: f32::from_bits(cells.held_right.load(Ordering::Relaxed)),
                },
                linear_gain: f32::from_bits(cells.gain.load(Ordering::Relaxed)),
                auto_gain: fixed.auto_gain,
                auto_gain_db: fixed.auto_gain_db,
                tempo_ratio: f64::from_bits(cells.tempo.load(Ordering::Relaxed)),
                nudge: f32::from_bits(cells.nudge.load(Ordering::Relaxed)),
                slip: cells.slip.load(Ordering::Relaxed),
                reverse: cells.reverse.load(Ordering::Relaxed),
                cached: cells.cached.load(Ordering::Relaxed),
                cached_bytes: cells.cached_bytes.load(Ordering::Relaxed),
                slip_frame: {
                    let frame = cells.slip_frame.load(Ordering::Relaxed);
                    (frame != EMPTY).then_some(SourceFrame(frame))
                },
                fader: f32::from_bits(cells.fader.load(Ordering::Relaxed)),
                eq: EqGains {
                    low_db: f32::from_bits(cells.eq_low.load(Ordering::Relaxed)),
                    mid_db: f32::from_bits(cells.eq_mid.load(Ordering::Relaxed)),
                    high_db: f32::from_bits(cells.eq_high.load(Ordering::Relaxed)),
                },
                master: cells.master.load(Ordering::Relaxed),
                sync: sync_from_index(cells.sync.load(Ordering::Relaxed)),
                // Filled in below, where the master and the followers are both in hand.
                phase_error_ms: None,
                bar_error_beats: None,
            });
        }
        fill_phase_errors(&mut decks, self.beats_per_bar.load(Ordering::Relaxed));
        // What the render loop measured at the end of a block wins over what the snapshot works out from
        // numbers read at different moments: the first is a phase, the second is a phase plus a block.
        for (index, deck) in decks.iter_mut().enumerate() {
            let Some(cells) = self.decks.get(index) else {
                break;
            };
            let bits = cells.phase_error_ms.load(Ordering::Relaxed);
            if bits != u32::MAX && !deck.sync.is_off() {
                deck.phase_error_ms = Some(f64::from(f32::from_bits(bits)));
            }
        }
        Snapshot {
            protocol: dj_domain::STATE_PROTOCOL.to_string(),
            cache_budget_bytes: self.cache_budget.load(Ordering::Relaxed),
            revision,
            epoch,
            audio_clock_samples: self.clock.load(Ordering::Acquire),
            sample_rate: rate,
            crossfader: f32::from_bits(self.crossfader.load(Ordering::Relaxed)),
            crossfader_curve: curve_at(self.curve.load(Ordering::Relaxed)),
            master: dj_domain::Levels {
                peak_left: f32::from_bits(self.master_peak.load(Ordering::Relaxed)),
                peak_right: f32::from_bits(self.master_peak.load(Ordering::Relaxed)),
                held_left: f32::from_bits(self.master_held.load(Ordering::Relaxed)),
                held_right: f32::from_bits(self.master_held.load(Ordering::Relaxed)),
            },
            master_gain: f32::from_bits(self.master_gain.load(Ordering::Relaxed)),
            master_latency_frames: self.master_latency.load(Ordering::Relaxed),
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
    /// Material behind the playhead, reversed by the worker, for a deck that cannot walk its source
    /// backwards (a decoded track too large for the cache).
    ReverseWindow {
        /// Target deck.
        deck: DeckId,
        /// The file the window was read from: a deck that has moved on keeps its own material.
        path: std::path::PathBuf,
        /// Source frame the window begins at.
        start: u64,
        /// Forward material from `start`; the deck reverses it on the command thread.
        material: Vec<f32>,
    },
    /// The cache finished building this track after the deck had already started streaming it.
    ///
    /// The loader attaches the stream first and builds the cache behind it, so a long file starts playing
    /// at once instead of after two decodes. The deck swaps sources on a block boundary without a reset.
    CacheReady {
        /// Target deck.
        deck: DeckId,
        /// The file the entry was built from: a deck that has moved on keeps its own source.
        path: std::path::PathBuf,
        /// The reader that plays the entry.
        source: Box<dyn DeckSource>,
    },
    /// A measurement started or finished for this deck (B02).
    SetAnalyzing {
        /// Target deck.
        deck: DeckId,
        /// Whether the deck is being measured now.
        analyzing: bool,
    },
    /// A new beat grid for a loaded track: the DJ asked for the tempo again (B02).
    SetGrid {
        /// Target deck.
        deck: DeckId,
        /// The grid, in source frames. Boxed: a grid of segments outweighs the other variants.
        grid: Box<BeatGrid>,
    },
}

/// A reversed window the graph asks the worker for.
///
/// Born in the audio callback, so it carries no owned buffers beyond the path it already knows: the
/// worker opens the file, seeks, reads and answers with [`GraphLoad::ReverseWindow`].
pub struct ReverseRequest {
    /// Deck that wants the material.
    pub deck: DeckId,
    /// File to read.
    pub path: std::path::PathBuf,
    /// Source frame the window begins at.
    pub start: u64,
    /// How many frames to read forward from `start`.
    pub frames: u64,
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
    /// Beat grid from the analysis, in source frames, replaced by the stored one when the file
    /// carries it (DJP-SYNC-003).
    pub grid: Option<BeatGrid>,
    /// Names and colours of those cues, restored at attach (DJP-DECK-003).
    pub cue_meta: Vec<(u8, Option<String>, Option<String>)>,
    /// Loops stored on pads, restored at attach (DJP-DECK-004).
    pub pad_loops: Vec<(u8, u64, u64)>,
    /// Loop recorded for this file.
    pub loop_region: Option<FrameRange>,
    /// Gain the channel opens with, from the file's own loudness (DJP-MIX-001 auto-gain).
    ///
    /// 1.0 when the file was not measured: a missing number must not silently attenuate a track.
    pub auto_gain: f32,
    /// The same correction in decibels; `None` when the level was not measured.
    pub auto_gain_db: Option<f32>,
    /// Key of this track in the PCM cache, when the deck reads from memory.
    ///
    /// The loader pins the entry while the deck holds the track: eviction must never take the material
    /// out from under a deck that is playing it, and an unpinned entry is what a closed deck releases.
    pub cache_key: Option<String>,
}

/// A change worth writing to the track's metadata file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaEdit {
    /// The file the deck holds.
    pub path: std::path::PathBuf,
    /// Cues as frames of the source file, slot and position.
    pub cues: Vec<(u8, u64)>,
    /// Names and colours of those cues, slot first; an absent half means "keep what is on disk".
    pub cue_meta: Vec<(u8, Option<String>, Option<String>)>,
    /// Loops on pads: slot, start and end in frames.
    pub pad_loops: Vec<(u8, u64, u64)>,
    /// The active loop as `(start, end)` frames.
    pub loop_region: Option<(u64, u64)>,
}

/// A beat grid: the tempo and where its first beat sits (B02, DJP-SYNC-003).
///
/// Apply one edit to a grid, or refuse it.
///
/// One place, so the meaning of "half" is the same in the engine, in the file and in a test. The edits
/// are the DJ's: they are applied to the whole track, because a grid is how the whole track is read, and
/// a refusal (a shift off the start of the file, a tempo that is not a number) leaves the grid alone.
pub fn edit_grid(grid: &BeatGrid, edit: &GridEdit, source_rate: u32) -> Option<BeatGrid> {
    match edit {
        // The rate is the grid's own timebase, and a shift needs it only to know how wide a square of
        // four bars is: a grid that is shifted past the start of the file is rebased by whole squares
        // instead of stopping at zero.
        GridEdit::Shift { frames } => grid.shifted(*frames, f64::from(source_rate)),
        GridEdit::Tempo { bpm } => grid.retimed(*bpm),
        GridEdit::Double => grid.retimed(grid.bpm() * 2.0),
        GridEdit::Half => grid.retimed(grid.bpm() / 2.0),
        // The nearest beat, moved onto the frame: the shift is what the grid already knows how to do, so
        // the tempo, the numbering and the bar all stay where they are and only the place changes.
        GridEdit::Here { frame } => {
            let rate = f64::from(source_rate);
            let beat = grid.beat_at(*frame, rate).round();
            let at = grid.frame_of_beat(beat, rate);
            let delta = *frame as f64 - at;
            if !delta.is_finite() || delta.abs() > u64::MAX as f64 {
                return None;
            }
            grid.shifted(delta.round() as i64, rate)
        }
    }
}

/// What the loader tells the publisher, so the UI can show `Loading` and `Failed`.
#[derive(Debug, Clone)]
pub struct DeckUpdate {
    /// Deck the update is about.
    pub deck: DeckId,
    /// New static description.
    pub static_info: DeckStatic,
}

/// What the publisher learns between two frames, from the threads that own the slow work.
///
/// The render thread knows nothing of this: a measurement is a decode, so its answers arrive from
/// the worker, exactly like a load does. A grid update carries two numbers rather than a whole
/// description, because everything else about the track is unchanged.
#[derive(Debug, Clone)]
pub enum Published {
    /// A deck was loaded, emptied or failed: the whole description changes. Boxed, like an attach:
    /// the description carries strings and a waveform reference, and the other variants are two words.
    Deck(Box<DeckUpdate>),
    /// A tempo was measured again (B02): only the grid changes.
    Grid {
        /// Deck whose grid was replaced.
        deck: DeckId,
        /// The grid the measurement produced.
        grid: Box<BeatGrid>,
    },
    /// A measurement started or finished (B02): the deck says so while it runs.
    Analyzing {
        /// Deck being measured.
        deck: DeckId,
        /// Whether a measurement is running now.
        analyzing: bool,
    },
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
    /// Handed to the worker that measures tempo: a full decode, so never the callback.
    Analyze {
        /// Target deck.
        deck: DeckId,
        /// The file to measure.
        path: String,
    },
    /// Handed to the worker that edits grids: it owns the file write and the publication.
    EditGrid {
        /// Target deck.
        deck: DeckId,
        /// The file the deck holds.
        path: String,
        /// The edit the DJ made.
        edit: GridEdit,
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
        DjCommand::AnalyzeTempo { deck, path } => Routed::Analyze {
            deck: *deck,
            path: path.clone(),
        },
        DjCommand::EditGrid { deck, path, edit } => Routed::EditGrid {
            deck: *deck,
            path: path.clone(),
            edit: *edit,
        },
        _ => Routed::Render,
    }
}

/// Open a file, make it playable at `rate` and describe it for the display (ADR-0008).
///
/// Runs on the loader thread. The PCM-cache decision is taken here, at load, from the file's
/// format — never in the middle of playback, where it would show up as a dropout.
/// The reader that plays a track from the cache, and the entry's key.
///
/// `None` when the cache does not want the file (disabled, lossless not asked for, over budget) or cannot
/// hold it. A refusal is not an error: the deck keeps streaming, which is what it was doing anyway.
pub fn cached_source(
    path: &std::path::Path,
    cache: &mut dj_codec::PcmCache,
) -> Option<(Box<dyn DeckSource>, String)> {
    match cache.prepare(path) {
        Ok(dj_codec::CacheOutcome::Built | dj_codec::CacheOutcome::Hit) => {}
        Ok(_) => return None,
        Err(error) => {
            eprintln!("dj-engine: the cache refused {}: {error}", path.display());
            return None;
        }
    }
    let info = match dj_codec::StreamDecoder::open(path) {
        Ok(decoder) => decoder.info().clone(),
        Err(error) => {
            eprintln!("dj-engine: no cache key for {}: {error}", path.display());
            return None;
        }
    };
    let key = match PcmCache::key_for(path, &info, cache.config()) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("dj-engine: no cache key for {}: {error}", path.display());
            return None;
        }
    };
    match cache.open_reader(&key) {
        Ok(reader) => Some((
            Box::new(dj_deck::CachedReaderSource::new(reader)) as Box<dyn DeckSource>,
            key,
        )),
        Err(error) => {
            eprintln!(
                "dj-engine: the cache entry for {} cannot be read: {error}",
                path.display()
            );
            None
        }
    }
}

pub fn prepare_track(
    path: &std::path::Path,
    rate: u32,
    window_seconds: f64,
    auto_gain_share: f32,
    cache: Option<&mut dj_codec::PcmCache>,
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
        first_beat_frame: None,
        grid: None,
        key: None,
    };
    let mut static_info = DeckStatic {
        deck: DeckId::A, // replaced by the caller, which knows the deck
        load: DeckLoad::Ready,
        track: Some(track.clone()),
        total_frames: info.frames,
        waveform: None, // filled by the caller, which knows the cache directory
        auto_gain: 1.0, // the loader fills it in below: this function knows samples, not levels
        auto_gain_db: None,
        analyzing: false,
        grid: None, // filled in below, once the tempo is measured
        source_rate: info.sample_rate,
        path: Some(path.display().to_string()),
    };
    // Loudness is measured here, on the loader thread: it is a full decode of the file, and the audio
    // callback must never see it. A file that cannot be measured opens at unity and says nothing.
    let measured = dj_analysis::loudness::analyse(path, auto_gain_share).ok();
    let auto_gain = measured.map_or(1.0, |loudness| loudness.gain);
    // The published number is the gain in force, not the correction the file asked for: the correction is
    // applied whole and the matched level is then taken down by the configured factor, so a snapshot that
    // showed only the correction would describe a gain the deck does not run at.
    let auto_gain_db = measured.map(|loudness| 20.0 * loudness.gain.max(1e-6).log10());
    // Tempo is measured here too: another full decode, on the thread that already owns one, and the
    // audio callback never sees it. A file whose tempo cannot be measured plays without a grid.
    // The measurement is expressed in the file's own frames — the decoder reads at the file's rate and
    // so does the deck, so a 44.1 kHz track keeps its grid in 44.1 kHz frames whatever the device runs at.
    let measured_tempo = dj_analysis::tempo::analyse(path).ok().flatten();
    let measured_grid = measured_tempo.and_then(|found| {
        let segments: Vec<BeatSegment> = found
            .segments
            .iter()
            .map(|segment| BeatSegment {
                frame: segment.first_beat_frame,
                beat: segment.beat,
                bpm: segment.bpm,
            })
            .collect();
        BeatGrid::from_segments(segments).map(|grid| grid.with_beats_per_bar(found.metre))
    });
    let mut track = track;
    if let Some(grid) = measured_grid.clone() {
        track.bpm = Some(grid.bpm());
        track.first_beat_frame = Some(grid.first_beat_frame());
        track.grid = Some(grid);
    }
    static_info.track = Some(track.clone());
    static_info.grid = track.grid.clone();
    // The published deck description carries the correction too: the shell reads the snapshot, and a
    // snapshot that says "1.0" about a trimmed track hides the only place the number could be seen.
    static_info.auto_gain = auto_gain;
    static_info.auto_gain_db = auto_gain_db;

    // Where this deck will read from: memory when the cache accepts the file, the decode window
    // otherwise. The decision is taken here, on the loader thread, after the measurements that already
    // decoded the file — never in the middle of playback, where it would be a dropout.
    //
    // A cache that refuses (disabled, too large, not wanted) is not a failure: the file streams.
    let (source, cache_key): (Box<dyn DeckSource>, Option<String>) = match cache {
        Some(cache) => match cache.prepare(path) {
            Ok(dj_codec::CacheOutcome::Built | dj_codec::CacheOutcome::Hit) => {
                match PcmCache::key_for(path, &info, cache.config()) {
                    Ok(key) => match cache.open_reader(&key) {
                        Ok(reader) => (
                            Box::new(dj_deck::CachedReaderSource::new(reader)),
                            Some(key),
                        ),
                        Err(error) => {
                            eprintln!(
                                "dj-engine: the cache entry for {} cannot be read: {error}",
                                path.display()
                            );
                            (Box::new(prefetcher), None)
                        }
                    },
                    Err(error) => {
                        eprintln!("dj-engine: no cache key for {}: {error}", path.display());
                        (Box::new(prefetcher), None)
                    }
                }
            }
            Ok(_) => (Box::new(prefetcher), None),
            Err(error) => {
                eprintln!("dj-engine: the cache refused {}: {error}", path.display());
                (Box::new(prefetcher), None)
            }
        },
        None => (Box::new(prefetcher), None),
    };

    Ok(PreparedTrack {
        deck: DeckId::A,
        source,
        track: Some(track),
        grid: measured_grid,
        static_info,
        path: Some(path.to_path_buf()),
        // What the file remembers is read by the caller: this function knows samples, not sessions.
        cues: Vec::new(),
        cue_meta: Vec::new(),
        pad_loops: Vec::new(),
        loop_region: None,
        auto_gain,
        auto_gain_db,
        cache_key,
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
    cache_swaps: u64,
    /// Where an edit goes. The callback only sends; the writing happens on another thread, because
    /// a disk call in the audio path is the one thing DJP-NFR-001 forbids outright.
    meta: Option<std::sync::mpsc::Sender<MetaEdit>>,
    /// Path of the file on each deck, in deck order.
    paths: Vec<Option<std::path::PathBuf>>,
    /// Where a request for reversed material goes. `None` in a renderer with no worker (the A01
    /// tone engine, most tests): a deck then simply has to wait for material it will never get.
    reverse_worker: Option<std::sync::mpsc::Sender<ReverseRequest>>,
    /// Bytes the whole-track cache may hold, as configured. Zero when the cache is off.
    cache_budget: u64,
    /// The metronome's click generator: four voices mixed into the master bus.
    metronome: metronome::Metronome,
    /// Blocks since the last sync step: the loop runs every [`SYNC_BLOCKS`], not every block.
    sync_blocks: u32,
    /// What the sync loop remembers between steps: which decks were just moved.
    sync_memory: SyncMemory,
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
        let mixer_sample_rate = mixer.sample_rate();
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
            cache_swaps: 0,
            meta: None,
            reverse_worker: None,
            cache_budget: 0,
            paths: vec![None; decks_len],
            metronome: metronome::Metronome::new(mixer_sample_rate),
            sync_blocks: 0,
            sync_memory: SyncMemory::default(),
        }
    }

    /// Fire a click on the beat every deck with a metronome crossed in this block, and mix the clicks
    /// in flight into the master bus.
    ///
    /// The bus is taken before the master gain and the limiter: a reference played beside the music is
    /// heard where the music is heard, and a loud mix does not clip because of it.
    fn click_on_the_beat(&mut self, before: &[u64; MAX_DECKS], frames: usize) {
        for (index, stood) in before.iter().enumerate().take(self.decks.len()) {
            let deck = &self.decks[index];
            if !deck.metronome() {
                continue;
            }
            let Some(grid) = deck.beat_grid() else {
                continue;
            };
            let rate = f64::from(deck.source_rate());
            let Some((offset, accent)) =
                metronome::beat_crossing(grid, rate, *stood, deck.position(), frames)
            else {
                continue;
            };
            if offset < frames {
                self.decks[index].note_click();
            }
            self.metronome.fire(offset, accent);
        }
        if !self.metronome.is_idle() {
            let master = self.mixer.master_bus();
            self.metronome.mix(master, frames);
        }
    }

    /// Send cue and loop edits to `sink`. Called before the stream starts: the callback never locks.
    /// Tell the graph the cache budget, so the state can say how full it is.
    pub fn with_cache_budget(mut self, bytes: u64) -> Self {
        self.cache_budget = bytes;
        self
    }

    /// Give the graph the worker that fills reversed windows for streamed tracks.
    pub fn with_reverse_worker(mut self, worker: std::sync::mpsc::Sender<ReverseRequest>) -> Self {
        self.reverse_worker = Some(worker);
        self
    }

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
            // A name or a colour is part of what the file remembers, so it is written the same way a
            // cue position is.
            DjCommand::Cue { deck, .. }
            | DjCommand::SetCueMeta { deck, .. }
            | DjCommand::SetPadLoop { deck, .. }
            | DjCommand::SetLoop { deck, .. } => *deck,
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
        let labels = deck.cue_labels();
        let colors = deck.cue_colors();
        let cue_meta: Vec<(u8, Option<String>, Option<String>)> = cues
            .iter()
            .map(|(slot, _)| {
                let index = usize::from(*slot);
                (
                    *slot,
                    labels.get(index).cloned().flatten(),
                    colors.get(index).cloned().flatten(),
                )
            })
            .collect();
        let loop_region = deck
            .loop_region()
            .map(|region| (region.start.0, region.end.0));
        let pad_loops: Vec<(u8, u64, u64)> = deck
            .pad_loops()
            .iter()
            .enumerate()
            .filter_map(|(slot, region)| {
                region.map(|region| (slot as u8, region.start.0, region.end.0))
            })
            .collect();
        let _ = sink.send(MetaEdit {
            path,
            cues,
            cue_meta,
            pad_loops,
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

    /// How many decks took their material from the track cache after starting on the stream.
    pub fn cache_swaps(&self) -> u64 {
        self.cache_swaps
    }

    /// Render one block into `master` and `cue`, interleaved stereo.
    ///
    /// Everything here runs inside the audio callback, so it allocates nothing, locks nothing, and
    /// never waits for a decoder: a deck that has no data renders silence and counts it.
    pub fn render_pair(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize) {
        let frames = frames.min(self.block);
        // The budget is a setting, not a measurement: one atomic store keeps the state honest.
        self.cells.set_cache_budget(self.cache_budget);
        self.cells.set_beats_per_bar(beats_per_bar(&self.decks));
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
                        self.cells.clear_cue_meta(index);
                        self.cells.clear_pad_loops(index);
                        for (slot, start, end) in &prepared.pad_loops {
                            self.decks[index].set_pad_loop(
                                usize::from(*slot),
                                Some(FrameRange {
                                    start: SourceFrame(*start),
                                    end: SourceFrame(*end),
                                }),
                            );
                            self.cells.set_pad_loop(
                                index,
                                usize::from(*slot),
                                Some((*start, *end)),
                            );
                        }
                        for (slot, label, color) in &prepared.cue_meta {
                            self.decks[index].set_cue_meta(
                                usize::from(*slot),
                                label.clone(),
                                color.clone(),
                            );
                            self.cells.set_cue_meta(
                                index,
                                usize::from(*slot),
                                label.clone(),
                                color.clone(),
                            );
                        }
                        let _ = self.decks[index].set_loop(prepared.loop_region);
                        // The track opens at its own level: a hot master and a quiet one then start
                        // from the same place, and a +6 dB EQ boost has room instead of the limiter.
                        // The measured correction is applied on top of the trim, not written into it.
                        self.decks[index].set_auto_gain(prepared.auto_gain, prepared.auto_gain_db);
                        self.paths[index] = prepared.path;
                        self.loads_applied += 1;
                    }
                }
                GraphLoad::ReverseWindow {
                    deck,
                    path,
                    start,
                    material,
                } => {
                    if let Some(index) = self.decks.iter().position(|held| held.id() == deck)
                        && self.paths[index].as_ref() == Some(&path)
                    {
                        self.decks[index].fill_reverse_window(start, &material);
                    }
                }
                GraphLoad::CacheReady { deck, path, source } => {
                    // Only the track that is still loaded: a DJ who skipped on must not hear the previous
                    // track's memory under the new one.
                    if let Some(index) = self.decks.iter().position(|held| held.id() == deck)
                        && self.paths[index].as_ref() == Some(&path)
                        && self.decks[index].swap_source(source)
                    {
                        self.cache_swaps += 1;
                    }
                }
                GraphLoad::SetAnalyzing { deck, analyzing } => {
                    if let Some(index) = self.decks.iter().position(|held| held.id() == deck) {
                        self.decks[index].set_analyzing(analyzing);
                    }
                }
                GraphLoad::SetGrid { deck, grid } => {
                    if let Some(index) = self.decks.iter().position(|held| held.id() == deck) {
                        self.decks[index].set_beat_grid(*grid);
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

        // A deck going backwards on a decoded source asks for the next window here, at a block
        // boundary: the send is the only thing that touches another thread, and it allocates nothing.
        if let Some(worker) = self.reverse_worker.clone() {
            for index in 0..self.decks.len() {
                let Some((start, frames)) = self.decks[index].reverse_window_needed() else {
                    continue;
                };
                let Some(path) = self.paths.get(index).cloned().flatten() else {
                    continue;
                };
                let _ = worker.send(ReverseRequest {
                    deck: self.decks[index].id(),
                    path,
                    start,
                    frames,
                });
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
            // A pad's name and colour are strings, so they cannot ride in a cell: the publisher gets
            // them from the cells' own map, written here on the command path.
            // A delete clears the pad's loop as well, so the published copy has to follow that too.
            if let DjCommand::Cue {
                deck,
                action: CueAction::Delete(slot),
            } = &submission.command.command
                && let Some(index) = self
                    .decks
                    .iter()
                    .position(|candidate| candidate.id() == *deck)
            {
                let region = self.decks[index]
                    .pad_loops()
                    .get(usize::from(*slot))
                    .copied()
                    .flatten()
                    .map(|region| (region.start.0, region.end.0));
                self.cells.set_pad_loop(index, usize::from(*slot), region);
                // A deleted pad loses its name and colour too, and the snapshot has to say so.
                let label = self.decks[index].cue_labels()[usize::from(*slot)].clone();
                let color = self.decks[index].cue_colors()[usize::from(*slot)].clone();
                self.cells
                    .write_cue_meta(index, usize::from(*slot), label, color);
            }
            if let DjCommand::SetPadLoop { deck, slot } = &submission.command.command
                && let Some(index) = self
                    .decks
                    .iter()
                    .position(|candidate| candidate.id() == *deck)
            {
                let region = self.decks[index]
                    .pad_loops()
                    .get(usize::from(*slot))
                    .copied()
                    .flatten()
                    .map(|region| (region.start.0, region.end.0));
                self.cells.set_pad_loop(index, usize::from(*slot), region);
            }
            // An emptied deck advertises nothing: pads, names and loops all belong to the track that
            // just left, and a strip that keeps painting them is lying about what is loaded.
            if let DjCommand::Eject { deck } = &submission.command.command
                && let Some(index) = self
                    .decks
                    .iter()
                    .position(|candidate| candidate.id() == *deck)
            {
                self.cells.clear_pad_loops(index);
                self.cells.clear_cue_meta(index);
            }
            // The armed set moves with the mode, with a pad, and with the button's own loop, and it is
            // the whole set that the strip paints — not one region.
            if let Some(deck) = match &submission.command.command {
                DjCommand::SetLoopMode { deck, .. }
                | DjCommand::SetPadLoop { deck, .. }
                | DjCommand::SetLoop { deck, .. }
                | DjCommand::LoopBeats { deck, .. }
                | DjCommand::LoopMove { deck, .. }
                | DjCommand::Cue { deck, .. }
                | DjCommand::Load { deck, .. }
                | DjCommand::Eject { deck } => Some(*deck),
                _ => None,
            } && let Some(index) = self
                .decks
                .iter()
                .position(|candidate| candidate.id() == deck)
            {
                let armed = self.decks[index]
                    .active_loops()
                    .iter()
                    .map(|region| (region.start.0, region.end.0))
                    .collect();
                self.cells.set_active_loops(index, armed);
                self.cells
                    .set_loop_mode(index, self.decks[index].loop_mode());
            }
            if let DjCommand::SetCueMeta {
                deck,
                slot,
                label,
                color,
            } = &submission.command.command
                && let Some(index) = self
                    .decks
                    .iter()
                    .position(|candidate| candidate.id() == *deck)
            {
                self.cells
                    .set_cue_meta(index, usize::from(*slot), label.clone(), color.clone());
            }
            let _ = submission.reply.send(ack);
        }

        // Sync is arithmetic on two decks and it has to see both at the same instant, which only the
        // callback can promise. A step every few blocks is the tick the planner was measured at, and it
        // is cheap enough not to need a thread of its own (DJP-SYNC-001/002).
        self.sync_blocks = (self.sync_blocks + 1) % SYNC_BLOCKS;
        if self.sync_blocks == 0 {
            let rate = f64::from(self.mixer.sample_rate());
            let seconds = if rate > 0.0 {
                (self.block * SYNC_BLOCKS as usize) as f64 / rate
            } else {
                0.05
            };
            sync_step(&mut self.decks, &mut self.sync_memory, seconds);
        }

        self.mixer.begin_block(frames);
        // Where each deck stood when the block began: the metronome is placed by the beat the deck
        // crossed inside this block, and the crossing is only known from both ends of it.
        let mut before = [0u64; MAX_DECKS];
        for (index, stood) in before.iter_mut().enumerate().take(self.decks.len()) {
            *stood = self.decks[index].position();
            let deck = &mut self.decks[index];
            deck.render(&mut self.scratch[..frames * 2]);
            self.mixer.mix_channel(index, &self.scratch[..frames * 2]);
        }
        self.click_on_the_beat(&before, frames);
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
