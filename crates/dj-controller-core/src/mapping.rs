//! What a control means to the engine: the binding table.
//!
//! A driver says what it saw (`deck_a.play`, pressed). This says what that asks the engine to do. The table is data
//! on purpose (ADR-0006): the next device needs a new table, not a new program, and a DJ's own remap is a table edit.
//!
//! Only controls whose meaning is a fact about the *program* live here — play, pause, cue, sync. Anything whose
//! meaning is a fact about a *device* stays in the driver and in its protocol notes: a table that guessed a byte would
//! put a command on the wrong button, and the DJ would find out on stage.
//!
//! - Backlog task: A09 (the path), A10 (the routing)
//! - Requirements: DJP-CTRL-002, DJP-CTRL-004

use dj_domain::{DeckId, DjCommand, SyncMode};

/// What a control asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Start the deck.
    Play,
    /// Stop the deck.
    Pause,
    /// Start if stopped, stop if running. The button and the transport's own state decide.
    PlayPause,
    /// Sync this deck to its master.
    Sync,
}

// The cue button of a real controller is deliberately absent: the engine has no command for the cue point, only
// hotcues (`DjCommand::Cue` works on slots). An entry that jumped to hotcue 0 instead would be a button doing
// something other than the word says, so the line is missing and the gap is the engine's, to be closed on its own.

impl Intent {
    /// The command this intent turns into, or `None` when the deck cannot act on it.
    ///
    /// `pressed` is the control's state: a button that reports its release is not a second command, so everything but
    /// the toggle answers nothing on release.
    #[must_use]
    pub fn command(self, deck: DeckId, pressed: bool, running: bool) -> Option<DjCommand> {
        match self {
            Self::Play if pressed => Some(DjCommand::Play {
                deck,
                enabled: true,
            }),
            Self::Play => None,
            Self::Pause if pressed => Some(DjCommand::Play {
                deck,
                enabled: false,
            }),
            Self::Pause => None,
            Self::PlayPause if pressed => Some(DjCommand::Play {
                deck,
                enabled: !running,
            }),
            Self::PlayPause => None,
            // A sync button is a switch: pressing it again lets the deck go back to standing on its own.
            Self::Sync if pressed => Some(DjCommand::Sync {
                deck,
                mode: if running {
                    SyncMode::Off
                } else {
                    SyncMode::Tempo
                },
            }),
            Self::Sync => None,
        }
    }
}

/// A deck a control can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindableDeck {
    /// Deck A.
    A,
    /// Deck B.
    B,
    /// Deck C.
    C,
    /// Deck D.
    D,
}

impl BindableDeck {
    /// The protocol's own name for it.
    #[must_use]
    pub fn id(self) -> DeckId {
        match self {
            Self::A => DeckId::A,
            Self::B => DeckId::B,
            Self::C => DeckId::C,
            Self::D => DeckId::D,
        }
    }

    /// Which deck a control name starts with, `deck_a.play` being deck A's.
    #[must_use]
    pub fn from_control(control: &str) -> Option<(Self, &str)> {
        let rest = control.strip_prefix("deck_")?;
        let (letter, tail) = rest.split_once('.')?;
        let deck = match letter {
            "a" => Self::A,
            "b" => Self::B,
            "c" => Self::C,
            "d" => Self::D,
            _ => return None,
        };
        Some((deck, tail))
    }
}

/// The meaning of a control's name.
///
/// Names, not numbers: the same words a log line carries, so a binding can be read out loud.
#[must_use]
pub fn intent_for(control: &str) -> Option<Intent> {
    let (_, tail) = BindableDeck::from_control(control)?;
    match tail {
        "play" => Some(Intent::Play),
        "pause" => Some(Intent::Pause),
        "play_pause" => Some(Intent::PlayPause),
        "sync" => Some(Intent::Sync),
        _ => None,
    }
}

/// The whole table: every control this program understands, and what it asks for.
#[derive(Debug, Default, Clone, Copy)]
pub struct Mapping;

impl Mapping {
    /// An empty mapping — the same table every time, because the table is the program's, not a session's.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Turns one event into the deck it is for and the command it asks for.
    ///
    /// `running` says what the transport is doing, for the intents whose meaning depends on it.
    #[must_use]
    pub fn resolve(
        &self,
        event: &crate::ControlEvent,
        running: bool,
    ) -> Option<(DeckId, DjCommand)> {
        let (deck, _) = BindableDeck::from_control(&event.control)?;
        let intent = intent_for(&event.control)?;
        let pressed = event.value > 0.5;
        intent
            .command(deck.id(), pressed, running)
            .map(|command| (deck.id(), command))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ControlEvent;

    #[test]
    fn a_control_name_carries_its_deck() {
        assert_eq!(
            BindableDeck::from_control("deck_a.play"),
            Some((BindableDeck::A, "play"))
        );
        assert_eq!(
            BindableDeck::from_control("deck_d.sync"),
            Some((BindableDeck::D, "sync"))
        );
        assert_eq!(BindableDeck::from_control("deck_e.play"), None);
        assert_eq!(BindableDeck::from_control("wheel_left"), None);
    }

    #[test]
    fn only_the_controls_we_understand_have_a_meaning() {
        assert_eq!(intent_for("deck_a.play"), Some(Intent::Play));
        assert_eq!(intent_for("deck_b.sync"), Some(Intent::Sync));
        assert_eq!(intent_for("deck_a.tempo"), None);
        assert_eq!(intent_for("play"), None);
        // The cue button has no engine command yet, and an entry for it would be a button doing something else.
        assert_eq!(intent_for("deck_a.cue"), None);
    }

    #[test]
    fn a_press_plays_and_a_release_says_nothing() {
        let pressed = Intent::Play.command(DeckId::A, true, false);
        assert_eq!(
            pressed,
            Some(DjCommand::Play {
                deck: DeckId::A,
                enabled: true
            })
        );
        assert_eq!(Intent::Play.command(DeckId::A, false, true), None);
    }

    #[test]
    fn the_toggle_reads_the_transport_not_the_button() {
        assert_eq!(
            Intent::PlayPause.command(DeckId::B, true, false),
            Some(DjCommand::Play {
                deck: DeckId::B,
                enabled: true
            })
        );
        assert_eq!(
            Intent::PlayPause.command(DeckId::B, true, true),
            Some(DjCommand::Play {
                deck: DeckId::B,
                enabled: false
            })
        );
    }

    #[test]
    fn sync_is_a_switch() {
        assert_eq!(
            Intent::Sync.command(DeckId::C, true, false),
            Some(DjCommand::Sync {
                deck: DeckId::C,
                mode: SyncMode::Tempo
            })
        );
        assert_eq!(
            Intent::Sync.command(DeckId::C, true, true),
            Some(DjCommand::Sync {
                deck: DeckId::C,
                mode: SyncMode::Off
            })
        );
    }

    #[test]
    fn the_table_resolves_an_event_into_a_deck_and_a_command() {
        let mapping = Mapping::new();
        let event = ControlEvent::button("deck_a.play", true);
        assert_eq!(
            mapping.resolve(&event, false),
            Some((
                DeckId::A,
                DjCommand::Play {
                    deck: DeckId::A,
                    enabled: true
                }
            ))
        );
        let unknown = ControlEvent::level("wheel_left", 1.0);
        assert_eq!(mapping.resolve(&unknown, false), None);
    }
}
