//! The engine's own logic, separate from the process that hosts it.
//!
//! The binary owns threads, sockets and signals; everything that decides what a command does and
//! what a snapshot says lives here, so it can be tested without spawning a process. That
//! distinction came from a hanging integration test: spawning a server inside a test binary is a
//! fragile way to check a rule.

use dj_deck::Deck;
use dj_domain::{
    Ack, AckStatus, CueAction, DeckId, DjCommand, RejectReason, ScheduledCommand, Snapshot,
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
