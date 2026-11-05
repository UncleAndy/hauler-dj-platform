//! Bounded command queue with two classes of traffic.
//!
//! Critical commands (Play/Stop/Cue/Seek/Loop) never collapse and never push each other
//! out; continuous controls (tempo, gain, crossfader) share one slot per deck, so a fader
//! flood costs memory the queue already owns instead of starving transport commands
//! (DJP-ARCH-002/003). Nothing here allocates after construction.

use dj_domain::{Ack, COALESCE_SLOTS, CommandId, DjCommand, RejectReason, ScheduledCommand};

/// Capacity of the critical lane. At one drain per audio block this is several hundred
/// milliseconds of command burst headroom.
pub const CRITICAL_CAPACITY: usize = 32;

/// Probe into the queue for tests and telemetry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// Commands that ever entered a lane.
    pub accepted: u64,
    /// Critical commands dropped because the lane was full.
    pub rejected_critical: u64,
    /// Coalesced values that replaced an older value in the same slot.
    pub coalesced_replaced: u64,
}

/// Fixed-capacity, allocation-free command queue.
#[derive(Debug)]
pub struct CommandQueue {
    critical: [Option<ScheduledCommand>; CRITICAL_CAPACITY],
    coalesced: [Option<ScheduledCommand>; COALESCE_SLOTS],
    stats: QueueStats,
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandQueue {
    /// An empty queue.
    pub fn new() -> Self {
        Self {
            critical: [const { None }; CRITICAL_CAPACITY],
            coalesced: [const { None }; COALESCE_SLOTS],
            stats: QueueStats::default(),
        }
    }

    /// Enqueue a command, returning the ack the issuer should receive.
    ///
    /// A critical command that cannot be stored is rejected explicitly — a silent drop of
    /// Stop is exactly the failure the contract forbids.
    pub fn push(&mut self, scheduled: ScheduledCommand) -> Ack {
        let id = scheduled.id;
        match scheduled.command.coalesce_slot() {
            Some(slot) => {
                if self.coalesced[slot].is_some() {
                    self.stats.coalesced_replaced += 1;
                }
                self.coalesced[slot] = Some(scheduled);
                self.stats.accepted += 1;
                Ack::accepted(id)
            }
            None => {
                if let Some(free) = self.critical.iter_mut().find(|slot| slot.is_none()) {
                    *free = Some(scheduled);
                    self.stats.accepted += 1;
                    Ack::accepted(id)
                } else {
                    self.stats.rejected_critical += 1;
                    Ack::rejected(id, RejectReason::QueueFull)
                }
            }
        }
    }

    /// Sample of the earliest pending command, if any.
    pub fn next_sample(&self) -> Option<u64> {
        self.critical
            .iter()
            .flatten()
            .chain(self.coalesced.iter().flatten())
            .map(|command| command.effective_sample)
            .min()
    }

    /// Remove and return the earliest command due at or before `clock`.
    ///
    /// A command whose sample is still in the future stays in the queue — dropping it here
    /// would silently lose transport commands.
    pub fn pop_due(&mut self, clock: u64) -> Option<ScheduledCommand> {
        let critical = self
            .critical
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.as_ref()
                    .map(|command| (index, command.effective_sample))
            })
            .min_by_key(|(_, sample)| *sample);
        let coalesced = self
            .coalesced
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.as_ref()
                    .map(|command| (index, command.effective_sample))
            })
            .min_by_key(|(_, sample)| *sample);

        let (slot, sample) = match (critical, coalesced) {
            (Some((critical, critical_sample)), Some((coalesced, coalesced_sample))) => {
                if critical_sample <= coalesced_sample {
                    (Slot::Critical(critical), critical_sample)
                } else {
                    (Slot::Coalesced(coalesced), coalesced_sample)
                }
            }
            (Some((critical, sample)), None) => (Slot::Critical(critical), sample),
            (None, Some((coalesced, sample))) => (Slot::Coalesced(coalesced), sample),
            (None, None) => return None,
        };

        if sample > clock {
            return None;
        }

        match slot {
            Slot::Critical(index) => self.critical[index].take(),
            Slot::Coalesced(index) => self.coalesced[index].take(),
        }
    }

    /// Number of commands currently waiting in both lanes.
    pub fn len(&self) -> usize {
        self.critical.iter().flatten().count() + self.coalesced.iter().flatten().count()
    }

    /// True when no command is pending.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Queue statistics.
    pub fn stats(&self) -> QueueStats {
        self.stats
    }
}

#[derive(Debug, Clone, Copy)]
enum Slot {
    Critical(usize),
    Coalesced(usize),
}

/// Helper for building scheduled commands in tests and callers.
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
    use dj_domain::DeckId;

    fn play(id: u64, at: u64) -> ScheduledCommand {
        scheduled(
            id,
            at,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true,
            },
        )
    }

    fn gain(id: u64, at: u64, deck: DeckId, value: f32) -> ScheduledCommand {
        scheduled(
            id,
            at,
            DjCommand::SetChannelGain {
                deck,
                linear_gain: value,
            },
        )
    }

    #[test]
    fn a_flood_of_controls_does_not_drop_transport() {
        let mut queue = CommandQueue::new();
        queue.push(play(1, 100));
        for step in 0..1_000u64 {
            queue.push(gain(step + 2, step, DeckId::A, step as f32 / 1_000.0));
        }
        // Exactly one coalesced value survives, and the critical command is still there.
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.next_sample(), Some(100));
        assert_eq!(queue.stats().rejected_critical, 0);
        assert_eq!(queue.stats().coalesced_replaced, 999);
    }

    #[test]
    fn several_decks_keep_their_own_coalesced_value() {
        let mut queue = CommandQueue::new();
        queue.push(gain(1, 10, DeckId::A, 0.5));
        queue.push(gain(2, 20, DeckId::B, 0.25));
        assert_eq!(queue.len(), 2);
        let first = queue.pop_due(10).unwrap_or_else(|| panic!("A due"));
        assert_eq!(first.id, 1);
        let second = queue.pop_due(20).unwrap_or_else(|| panic!("B due"));
        assert_eq!(second.id, 2);
    }

    #[test]
    fn commands_are_returned_in_sample_order() {
        let mut queue = CommandQueue::new();
        queue.push(play(3, 300));
        queue.push(play(1, 100));
        queue.push(play(2, 200));
        let order: Vec<u64> = std::iter::from_fn(|| queue.pop_due(1_000))
            .map(|command| command.id)
            .collect();
        assert_eq!(order, vec![1, 2, 3]);
    }

    #[test]
    fn a_command_in_the_future_is_not_returned_early() {
        let mut queue = CommandQueue::new();
        queue.push(play(1, 500));
        assert!(queue.pop_due(499).is_none());
        assert_eq!(queue.len(), 1, "a future command must stay queued");
        assert!(queue.pop_due(500).is_some());
    }

    #[test]
    fn a_full_critical_lane_rejects_explicitly() {
        let mut queue = CommandQueue::new();
        for id in 0..CRITICAL_CAPACITY as u64 {
            queue.push(play(id, 10));
        }
        let ack = queue.push(play(999, 10));
        assert!(matches!(
            ack.status,
            dj_domain::AckStatus::Rejected(RejectReason::QueueFull)
        ));
        assert_eq!(queue.stats().rejected_critical, 1);
    }
}
