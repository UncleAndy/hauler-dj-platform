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

use dj_domain::{CueAction, DeckId, DjCommand, SyncMode};

/// The slot this program treats as the deck's cue point: hotcue 0, the one the panel's own Cue button writes.
pub const CUE_SLOT: u8 = 0;

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
    /// Go back to the cue point and carry on from there.
    CueJump,
    /// Store the cue point where the deck stands.
    CueSet,
    /// The tempo fader: an absolute position over the deck's pitch range.
    Tempo,
    /// A channel fader: the level a hand rides during a mix.
    ChannelFader,
    /// The crossfader: 0 is deck A, 1 is deck B.
    Crossfader,
    /// Put the channel into the headphones, or take it out.
    ChannelCue,
    /// One band of the channel's equaliser.
    Eq(EqBand),
    /// A hotcue pad: jump to its slot, or store the cue there when the slot is empty.
    Hotcue(u8),
}

/// Which band of the equaliser a knob turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqBand {
    /// Low band.
    Low,
    /// Mid band.
    Mid,
    /// High band.
    High,
}

impl EqBand {
    /// The band the engine's command names.
    #[must_use]
    pub fn as_domain(self) -> dj_domain::EqBand {
        match self {
            Self::Low => dj_domain::EqBand::Low,
            Self::Mid => dj_domain::EqBand::Mid,
            Self::High => dj_domain::EqBand::High,
        }
    }
}

/// The deck's pitch range, either side of unity: what a fader at its end asks for.
pub const PITCH_RANGE: f32 = 0.16;

/// Where a knob stands, in decibels, on the channel strip's own scale.
///
/// The same convention the window's knobs use: the middle is 0 dB, the top is +6 dB, the far end of the cut is −26 dB and
/// the last of it is the detent that kills the band. Written here once so the panel and the window cannot drift apart.
#[must_use]
pub fn eq_decibels(position: f32) -> f32 {
    let position = position.clamp(0.0, 1.0);
    if position <= 0.02 {
        return dj_domain::EQ_FLOOR_DB;
    }
    if position >= 0.5 {
        (position - 0.5) * 2.0 * dj_domain::EQ_BOOST_DB
    } else {
        (position - 0.5) * 2.0 * 26.0
    }
}

// The cue button: this program's cue point **is** hotcue slot 0 — the deck's own Cue button sets that slot with a
// hold and jumps to it with a tap (owner's decision), and the engine's `DjCommand::Cue` works on slots. So the two
// halves of the gesture have names of their own here rather than one name and a guess: `cue` jumps, `cue_set` stores.

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
            // Knobs, faders and pads carry a value or a slot, not a press: `Mapping::resolve_control` turns those into
            // commands, and this door stays about buttons.
            Self::Tempo
            | Self::ChannelFader
            | Self::Crossfader
            | Self::ChannelCue
            | Self::Eq(_)
            | Self::Hotcue(_) => None,
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
            Self::CueJump if pressed => Some(DjCommand::Cue {
                deck,
                action: CueAction::Jump(CUE_SLOT),
            }),
            Self::CueJump => None,
            Self::CueSet if pressed => Some(DjCommand::Cue {
                deck,
                action: CueAction::Set(CUE_SLOT),
            }),
            Self::CueSet => None,
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
    if let Some(slot) = tail.strip_prefix("hotcue")
        && let Ok(number) = slot.parse::<u8>()
        && (1..=8).contains(&number)
    {
        return Some(Intent::Hotcue(number - 1));
    }
    match tail {
        "play" => Some(Intent::Play),
        "pause" => Some(Intent::Pause),
        "play_pause" => Some(Intent::PlayPause),
        "sync" => Some(Intent::Sync),
        "cue" => Some(Intent::CueJump),
        "cue_set" => Some(Intent::CueSet),
        "tempo" => Some(Intent::Tempo),
        "fader" => Some(Intent::ChannelFader),
        "pfl" => Some(Intent::ChannelCue),
        "eq_low" => Some(Intent::Eq(EqBand::Low)),
        "eq_mid" => Some(Intent::Eq(EqBand::Mid)),
        "eq_high" => Some(Intent::Eq(EqBand::High)),
        _ => None,
    }
}

/// The meaning of a control that belongs to no deck: the crossfader is the mixer's, not a channel's.
#[must_use]
pub fn mixer_intent_for(control: &str) -> Option<Intent> {
    match control {
        "mixer.crossfader" => Some(Intent::Crossfader),
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

impl Mapping {
    /// Turns one event into the command it asks for, for controls that carry a value or a slot.
    ///
    /// `running` is what the transport is doing, `cue_present` whether the pad's slot already holds a cue: a pad with a
    /// cue jumps to it, an empty pad stores the cue where the deck stands.
    #[must_use]
    pub fn resolve_control(
        &self,
        event: &crate::ControlEvent,
        running: bool,
        cue_present: bool,
    ) -> Option<(Option<DeckId>, DjCommand)> {
        let deck = BindableDeck::from_control(&event.control).map(|(deck, _)| deck.id());
        let intent = intent_for(&event.control).or_else(|| mixer_intent_for(&event.control))?;
        let pressed = event.value > 0.5;
        let command = match intent {
            Intent::Tempo => {
                let ratio = 1.0
                    + (f64::from(event.value.clamp(0.0, 1.0)) * 2.0 - 1.0) * f64::from(PITCH_RANGE);
                Some(DjCommand::SetTempoRatio { deck: deck?, ratio })
            }
            Intent::ChannelFader => Some(DjCommand::SetChannelFader {
                deck: deck?,
                position: event.value.clamp(0.0, 1.0),
            }),
            Intent::Crossfader => Some(DjCommand::SetCrossfader {
                position: event.value.clamp(0.0, 1.0),
            }),
            Intent::ChannelCue => Some(DjCommand::SetChannelCue {
                deck: deck?,
                on: pressed,
            }),
            Intent::Eq(band) => Some(DjCommand::SetChannelEq {
                deck: deck?,
                band: band.as_domain(),
                gain_db: eq_decibels(event.value),
            }),
            Intent::Hotcue(slot) => {
                if !pressed {
                    return None;
                }
                Some(DjCommand::Cue {
                    deck: deck?,
                    action: if cue_present {
                        CueAction::Jump(slot)
                    } else {
                        CueAction::Set(slot)
                    },
                })
            }
            _ => {
                let command = intent.command(deck?, pressed, running)?;
                return Some((Some(deck?), command));
            }
        };
        command.map(|command| (deck, command))
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
    fn a_tempo_fader_asks_for_the_rate_it_stands_at() {
        let mapping = Mapping::new();
        let middle = ControlEvent::level("deck_a.tempo", 0.5);
        match mapping.resolve_control(&middle, false, false) {
            Some((Some(DeckId::A), DjCommand::SetTempoRatio { ratio, .. })) => {
                assert!((ratio - 1.0).abs() < 1e-9, "{ratio}");
            }
            other => panic!("{other:?}"),
        }
        let top = ControlEvent::level("deck_a.tempo", 1.0);
        match mapping.resolve_control(&top, false, false) {
            Some((Some(DeckId::A), DjCommand::SetTempoRatio { ratio, .. })) => {
                assert!(
                    (ratio - f64::from(1.0 + PITCH_RANGE)).abs() < 1e-6,
                    "{ratio}"
                );
            }
            other => panic!("{other:?}"),
        }
        let bottom = ControlEvent::level("deck_a.tempo", 0.0);
        match mapping.resolve_control(&bottom, false, false) {
            Some((Some(DeckId::A), DjCommand::SetTempoRatio { ratio, .. })) => {
                assert!(
                    (ratio - f64::from(1.0 - PITCH_RANGE)).abs() < 1e-6,
                    "{ratio}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn faders_and_the_crossfader_carry_their_position() {
        let mapping = Mapping::new();
        let fader = ControlEvent::level("deck_b.fader", 0.7);
        assert_eq!(
            mapping.resolve_control(&fader, false, false),
            Some((
                Some(DeckId::B),
                DjCommand::SetChannelFader {
                    deck: DeckId::B,
                    position: 0.7
                }
            ))
        );
        let crossfader = ControlEvent::level("mixer.crossfader", 0.25);
        assert_eq!(
            mapping.resolve_control(&crossfader, false, false),
            Some((None, DjCommand::SetCrossfader { position: 0.25 })),
            "the crossfader belongs to the mixer, not to a deck"
        );
    }

    #[test]
    fn an_equaliser_knob_follows_the_channel_strip() {
        assert!((eq_decibels(0.5) - 0.0).abs() < 1e-6, "the middle is unity");
        assert!(
            (eq_decibels(1.0) - dj_domain::EQ_BOOST_DB).abs() < 1e-6,
            "the top is the boost"
        );
        assert!(
            (eq_decibels(0.25) + 13.0).abs() < 1e-6,
            "a quarter down is half the cut"
        );
        assert_eq!(
            eq_decibels(0.0),
            dj_domain::EQ_FLOOR_DB,
            "the end of the cut kills the band"
        );
        let mapping = Mapping::new();
        let knob = ControlEvent::level("deck_a.eq_low", 0.75);
        match mapping.resolve_control(&knob, false, false) {
            Some((Some(DeckId::A), DjCommand::SetChannelEq { band, gain_db, .. })) => {
                assert_eq!(band, dj_domain::EqBand::Low);
                assert!((gain_db - 3.0).abs() < 1e-6, "{gain_db}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_headphone_button_says_whether_the_channel_is_in_the_headphones() {
        let mapping = Mapping::new();
        let press = ControlEvent::button("deck_a.pfl", true);
        assert_eq!(
            mapping.resolve_control(&press, false, false),
            Some((
                Some(DeckId::A),
                DjCommand::SetChannelCue {
                    deck: DeckId::A,
                    on: true
                }
            ))
        );
        let release = ControlEvent::button("deck_a.pfl", false);
        assert_eq!(
            mapping.resolve_control(&release, false, false),
            Some((
                Some(DeckId::A),
                DjCommand::SetChannelCue {
                    deck: DeckId::A,
                    on: false
                }
            ))
        );
    }

    #[test]
    fn a_hotcue_pad_jumps_to_its_slot_or_stores_one() {
        let mapping = Mapping::new();
        let press = ControlEvent::button("deck_a.hotcue3", true);
        assert_eq!(
            mapping.resolve_control(&press, false, true),
            Some((
                Some(DeckId::A),
                DjCommand::Cue {
                    deck: DeckId::A,
                    action: dj_domain::CueAction::Jump(2)
                }
            )),
            "a slot with a cue is jumped to"
        );
        assert_eq!(
            mapping.resolve_control(&press, false, false),
            Some((
                Some(DeckId::A),
                DjCommand::Cue {
                    deck: DeckId::A,
                    action: dj_domain::CueAction::Set(2)
                }
            )),
            "an empty slot is stored"
        );
        let release = ControlEvent::button("deck_a.hotcue3", false);
        assert_eq!(mapping.resolve_control(&release, false, true), None);
    }

    #[test]
    fn only_the_controls_we_understand_have_a_meaning() {
        assert_eq!(intent_for("deck_a.play"), Some(Intent::Play));
        assert_eq!(intent_for("deck_b.sync"), Some(Intent::Sync));
        assert_eq!(intent_for("deck_a.tempo"), Some(Intent::Tempo));
        assert_eq!(intent_for("deck_a.hotcue9"), None);
        assert_eq!(intent_for("play"), None);
        assert_eq!(intent_for("deck_a.cue"), Some(Intent::CueJump));
        assert_eq!(intent_for("deck_a.cue_set"), Some(Intent::CueSet));
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
    fn the_cue_button_is_the_deck_s_own_cue_point() {
        assert_eq!(
            Intent::CueJump.command(DeckId::A, true, false),
            Some(DjCommand::Cue {
                deck: DeckId::A,
                action: CueAction::Jump(CUE_SLOT)
            })
        );
        assert_eq!(
            Intent::CueSet.command(DeckId::B, true, true),
            Some(DjCommand::Cue {
                deck: DeckId::B,
                action: CueAction::Set(CUE_SLOT)
            })
        );
        // The release of either says nothing: the store and the jump are one gesture each.
        assert_eq!(Intent::CueJump.command(DeckId::A, false, false), None);
        assert_eq!(Intent::CueSet.command(DeckId::A, false, false), None);
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
