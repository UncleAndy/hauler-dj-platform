//! The render loop and the sample-accurate command scheduler.
//!
//! `process` is the audio callback: it renders exactly the buffer it is given, applies
//! commands at the audio-clock sample they were scheduled for, and never allocates,
//! locks, logs or waits (DJP-NFR-001). Everything that needs memory — snapshots, acks,
//! offline buffers — lives outside it.

use dj_domain::{
    Ack, CommandId, DeckId, DeckSnapshot, DjCommand, RejectReason, STATE_PROTOCOL,
    ScheduledCommand, Snapshot, SourceFrame,
};

use crate::queue::CommandQueue;
use crate::sine::SineSource;

/// How many acks the engine can hold before the caller drains them.
pub const ACK_CAPACITY: usize = 64;

/// Engine state machine for the A01 scaffolding: one signal source, transport, gain,
/// tempo and crossfader. Track loading, decks B–D and the mixer arrive in A02–A04.
#[derive(Debug)]
pub struct Engine {
    sample_rate: u32,
    clock: u64,
    queue: CommandQueue,
    sine: SineSource,
    base_frequency: f32,
    playing: bool,
    source_frame: u64,
    gain: f32,
    tempo_ratio: f64,
    crossfader: f32,
    revision: u64,
    epoch: u64,
    acks: [Option<Ack>; ACK_CAPACITY],
    ack_count: usize,
    acks_dropped: u64,
}

impl Engine {
    /// A stopped engine at `sample_rate` whose source plays a `frequency_hz` sine.
    pub fn new(sample_rate: u32, frequency_hz: f32) -> Self {
        Self {
            sample_rate,
            clock: 0,
            queue: CommandQueue::new(),
            sine: SineSource::new(frequency_hz, sample_rate),
            base_frequency: frequency_hz,
            playing: false,
            source_frame: 0,
            gain: 1.0,
            tempo_ratio: 1.0,
            crossfader: 0.0,
            revision: 0,
            epoch: 1,
            acks: [const { None }; ACK_CAPACITY],
            ack_count: 0,
            acks_dropped: 0,
        }
    }

    /// Device sample rate the clock counts at.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Audio-clock position of the next sample to be rendered.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// Queue a command and return the immediate ack.
    ///
    /// Commands the engine does not implement yet are rejected here rather than queued, so
    /// the queue only ever carries work that will actually happen (`NotSupported`).
    pub fn submit(&mut self, scheduled: ScheduledCommand) -> Ack {
        let ack = match &scheduled.command {
            DjCommand::Cue { .. } | DjCommand::SeekFrames { .. } | DjCommand::SetLoop { .. } => {
                Ack::rejected(scheduled.id, RejectReason::NotSupported)
            }
            _ => self.queue.push(scheduled),
        };
        self.push_ack(ack.clone());
        ack
    }

    /// Commands currently waiting in the queue.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Number of acks waiting to be drained.
    pub fn pending_acks(&self) -> usize {
        self.ack_count
    }

    /// Acks lost because the caller never drained them.
    pub fn acks_dropped(&self) -> u64 {
        self.acks_dropped
    }

    /// Move pending acks into `out`; returns how many were written.
    pub fn drain_acks(&mut self, out: &mut [Ack]) -> usize {
        let count = self.ack_count.min(out.len());
        for (index, ack) in self.acks.iter_mut().enumerate().take(count) {
            if let Some(ack) = ack.take() {
                out[index] = ack;
            }
        }
        let remaining = self.ack_count - count;
        for index in 0..remaining {
            self.acks[index] = self.acks[index + count].take();
        }
        for slot in self.acks.iter_mut().skip(remaining).take(count) {
            *slot = None;
        }
        self.ack_count = remaining;
        count
    }

    /// Render one block.
    ///
    /// RT contract: no allocation, no locks, no logging, no I/O, no panics on the data
    /// path. Commands are applied exactly at their `effective_sample`, splitting the block
    /// where needed.
    pub fn process(&mut self, out: &mut [f32]) {
        let mut written = 0usize;
        while written < out.len() {
            while let Some(command) = self.queue.pop_due(self.clock) {
                self.apply(command);
            }

            let block_end = self.clock + (out.len() - written) as u64;
            let boundary = self
                .queue
                .next_sample()
                .map_or(block_end, |sample| sample.min(block_end));
            let count = boundary.saturating_sub(self.clock) as usize;

            if count == 0 {
                // Next command sits exactly at the block end: render the rest and let it be
                // applied at the start of the next block.
                self.render(&mut out[written..]);
                break;
            }

            self.render(&mut out[written..written + count]);
            self.clock += count as u64;
            written += count;
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        if self.playing {
            for sample in out.iter_mut() {
                *sample = self.sine.next_sample() * self.gain;
            }
            self.source_frame += out.len() as u64;
        } else {
            for sample in out.iter_mut() {
                *sample = 0.0;
            }
        }
    }

    fn apply(&mut self, scheduled: ScheduledCommand) {
        let at_sample = self.clock;
        let command_id = scheduled.id;
        let status = match scheduled.command {
            DjCommand::Play { enabled, .. } => {
                if enabled && !self.playing {
                    self.sine.reset();
                }
                self.playing = enabled;
                Some(Ack::applied(command_id, at_sample))
            }
            DjCommand::SetChannelGain { linear_gain, .. } => {
                if linear_gain.is_finite() {
                    self.gain = linear_gain.clamp(0.0, 4.0);
                    Some(Ack::applied(command_id, at_sample))
                } else {
                    Some(Ack::rejected(command_id, RejectReason::InvalidValue))
                }
            }
            DjCommand::SetTempoRatio { ratio, .. } => {
                if ratio.is_finite() && ratio > 0.0 {
                    self.tempo_ratio = ratio;
                    self.sine.set_frequency(self.base_frequency * ratio as f32);
                    Some(Ack::applied(command_id, at_sample))
                } else {
                    Some(Ack::rejected(command_id, RejectReason::InvalidValue))
                }
            }
            DjCommand::SetCrossfader { position } => {
                if position.is_finite() {
                    self.crossfader = position.clamp(0.0, 1.0);
                    Some(Ack::applied(command_id, at_sample))
                } else {
                    Some(Ack::rejected(command_id, RejectReason::InvalidValue))
                }
            }
            DjCommand::Cue { .. } | DjCommand::SeekFrames { .. } | DjCommand::SetLoop { .. } => {
                Some(Ack::rejected(command_id, RejectReason::NotSupported))
            }
        };

        self.revision += 1;
        if let Some(ack) = status {
            self.push_ack(ack);
        }
    }

    fn push_ack(&mut self, ack: Ack) {
        if self.ack_count < ACK_CAPACITY {
            self.acks[self.ack_count] = Some(ack);
            self.ack_count += 1;
        } else {
            self.acks_dropped += 1;
        }
    }

    /// Read-only state for the UI/controller. Allocates, so it is never called from the
    /// audio callback.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            protocol: STATE_PROTOCOL.to_string(),
            revision: self.revision,
            epoch: self.epoch,
            audio_clock_samples: self.clock,
            sample_rate: self.sample_rate,
            crossfader: self.crossfader,
            decks: vec![DeckSnapshot {
                deck: DeckId::A,
                // The A01 engine plays a generated tone, not a file: it is ready by construction
                // and has no track, loop or hotcues. Decks arrive in A03 (`dj-deck`).
                load: dj_domain::DeckLoad::Ready,
                playing: self.playing,
                source_frame: SourceFrame(self.source_frame),
                remaining_frames: None,
                position_ratio: None,
                track: None,
                loop_region: None,
                hotcues: vec![None; dj_domain::HOTCUE_SLOTS],
                levels: dj_domain::Levels::SILENT,
                linear_gain: self.gain,
                tempo_ratio: self.tempo_ratio,
            }],
        }
    }
}

/// Render `frames` samples offline, block by block.
///
/// Allocates the output buffer up front and then only calls `process`; this is the path
/// the headless runner and the golden tests use (DJP-NFR-001 evidence).
pub fn render_offline(engine: &mut Engine, frames: usize, block_frames: usize) -> Vec<f32> {
    let block = block_frames.max(1);
    let mut out = vec![0.0f32; frames];
    let mut written = 0usize;
    while written < frames {
        let end = (written + block).min(frames);
        let (head, _) = out.split_at_mut(end);
        engine.process(&mut head[written..end]);
        written = end;
    }
    out
}

/// Build a scheduled command without spelling out the struct every time.
pub fn scheduled(id: CommandId, effective_sample: u64, command: DjCommand) -> ScheduledCommand {
    ScheduledCommand {
        id,
        effective_sample,
        command,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(id: u64, at: u64, enabled: bool) -> ScheduledCommand {
        scheduled(
            id,
            at,
            DjCommand::Play {
                deck: DeckId::A,
                enabled,
            },
        )
    }

    #[test]
    fn play_and_stop_take_effect_at_the_exact_sample() {
        let mut engine = Engine::new(48_000, 440.0);
        engine.submit(play(1, 100, true));
        engine.submit(play(2, 400, false));
        let samples = render_offline(&mut engine, 1_000, 256);

        assert!(
            samples[..100].iter().all(|s| *s == 0.0),
            "silent before play"
        );
        assert!(
            samples[100..400].iter().any(|s| *s != 0.0),
            "audible between play and stop"
        );
        assert!(
            samples[400..].iter().all(|s| *s == 0.0),
            "silent after stop"
        );
        assert_eq!(engine.clock(), 1_000);
    }

    #[test]
    fn the_ack_reports_the_exact_application_sample() {
        let mut engine = Engine::new(48_000, 440.0);
        engine.submit(play(7, 333, true));
        let _ = render_offline(&mut engine, 512, 128);
        let mut acks: [Ack; 8] = std::array::from_fn(|_| Ack::accepted(0));
        let count = engine.drain_acks(&mut acks);
        assert!(count >= 2, "accepted + applied");
        assert!(matches!(
            acks[1].status,
            dj_domain::AckStatus::Applied { at_sample: 333 }
        ));
        assert_eq!(acks[1].command_id, 7);
    }

    #[test]
    fn the_frame_count_is_exact_for_awkward_block_sizes() {
        for frames in [0usize, 1, 255, 256, 257, 48_000] {
            let mut engine = Engine::new(48_000, 440.0);
            let samples = render_offline(&mut engine, frames, 256);
            assert_eq!(samples.len(), frames);
            assert_eq!(engine.clock(), frames as u64);
        }
    }

    #[test]
    fn two_runs_of_the_same_scenario_are_identical() {
        let render = || {
            let mut engine = Engine::new(48_000, 440.0);
            engine.submit(play(1, 10, true));
            engine.submit(scheduled(
                2,
                200,
                DjCommand::SetChannelGain {
                    deck: DeckId::A,
                    linear_gain: 0.25,
                },
            ));
            render_offline(&mut engine, 4_800, 256)
        };
        assert_eq!(render(), render());
    }

    #[test]
    fn unimplemented_commands_are_rejected_at_submit() {
        let mut engine = Engine::new(48_000, 440.0);
        let ack = engine.submit(scheduled(
            1,
            0,
            DjCommand::SeekFrames {
                deck: DeckId::A,
                frame: SourceFrame(1_000),
            },
        ));
        assert!(matches!(
            ack.status,
            dj_domain::AckStatus::Rejected(RejectReason::NotSupported)
        ));
        assert_eq!(engine.queue_len(), 0);
    }

    #[test]
    fn a_bad_ratio_is_rejected_without_changing_state() {
        let mut engine = Engine::new(48_000, 440.0);
        engine.submit(play(1, 0, true));
        engine.submit(scheduled(
            2,
            0,
            DjCommand::SetTempoRatio {
                deck: DeckId::A,
                ratio: f64::NAN,
            },
        ));
        let _ = render_offline(&mut engine, 512, 128);
        assert!((engine.snapshot().decks[0].tempo_ratio - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_snapshot_carries_revision_clock_and_deck_state() {
        let mut engine = Engine::new(48_000, 440.0);
        engine.submit(play(1, 0, true));
        let _ = render_offline(&mut engine, 480, 256);
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.protocol, STATE_PROTOCOL);
        assert_eq!(snapshot.audio_clock_samples, 480);
        assert!(snapshot.revision >= 1);
        assert!(snapshot.decks[0].playing);
        assert_eq!(snapshot.decks[0].source_frame.0, 480);
    }
}
