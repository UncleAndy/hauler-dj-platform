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
    /// Take the lead the others follow, or give it up (DJP-SYNC-001).
    Master,
    /// Play the material backwards, or forwards again (DJP-DECK-007).
    Reverse,
    /// Keep the sound where it is while the track's own head runs on (DJP-DECK-007).
    Slip,
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
    /// A channel's gain (trim): lines a track's level up once, before the EQ.
    ChannelGain,
    /// The mixer's master level: a volume, not a trim, and it only attenuates.
    MasterGain,
    /// Put the channel into the headphones, or take it out.
    ChannelCue,
    /// One band of the channel's equaliser.
    Eq(EqBand),
    /// A hotcue pad: jump to its slot, or store the cue there when the slot is empty.
    Hotcue(u8),
}

/// What the deck is doing as the button goes down.
///
/// Three switches on the panel read the deck rather than the button: `PLAY` starts and stops, `SYNC` follows and lets go,
/// `MASTER` leads and lets go. Reading the deck's own state is what makes them switches instead of buttons that lie — a
/// SYNC that asked the transport whether the deck was playing turned sync *off* on every playing deck, so it could not be
/// turned on during a mix at all (owner's report, 25.11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeckStanding {
    /// The deck is playing.
    pub running: bool,
    /// The deck follows a master: its sync mode is not off.
    pub synced: bool,
    /// The deck is the master the others follow.
    pub master: bool,
    /// The deck plays its material backwards.
    pub reversed: bool,
    /// The deck is slipping: the sound stays where it was put while the track's own head runs on.
    pub slipped: bool,
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

/// What a deck has on one pad, as the window sees it.
///
/// A pad holds a point or a loop, never both, and calling "the pad is empty" a pad that holds a **loop** made the panel
/// write a cue into it: the loop vanished from the panel while the window still showed it (owner's report, 24.11). So the
/// three facts travel together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadState {
    /// The slot holds a cue point.
    pub cue: bool,
    /// The slot holds a loop.
    pub looped: bool,
    /// The deck has a loop of its own that an empty pad would take.
    pub active_loop: bool,
}

/// The deck's pitch range, either side of unity: what a fader at its end asks for.
pub const PITCH_RANGE: f32 = 0.16;

/// The value a channel fader's handle position is worth, on the window's own law.
///
/// The window's fader is not linear: `UiSettings::faderCurveValue` puts the handle at
/// `1 − curve(1 − where)`, where `curve` is the hyperbola `1,167·t / (0,167 + t)` through (0, 0), (½, 0,875) and
/// (1, 1) — the B-taper of a channel strip, steep at the bottom, gentle at the top. A hardware fader's travel *is* the
/// handle position, so the panel has to apply the same law; sending the travel straight through made the handle crawl at
/// the bottom and leap at the top (owner's report, 24.11: "the faders move unevenly").
#[must_use]
pub fn fader_value(handle: f32) -> f32 {
    let t = 1.0 - handle.clamp(0.0, 1.0);
    let curve = 1.166_666_7 * t / (0.166_666_67 + t);
    (1.0 - curve).clamp(0.0, 1.0)
}

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

/// Where a channel's gain (trim) knob stands, in decibels, on the window's own law.
///
/// The knob stands at unity (0 dB) in the middle, cuts to −40 dB on the left and boosts to +6 dB on the right — the same
/// ends as the window's `TrimKnob`. The travel is not the range: the cubic `0,1·t + 0,9·t²` (the window's `gainCurveValue`
/// with the trim's gradient 0.1) spends the fine control around unity and lets the ends move fast, so a hand turning the
/// panel knob and one turning the window knob reach the same gain.
#[must_use]
pub fn gain_decibels(position: f32) -> f32 {
    let position = position.clamp(0.0, 1.0);
    let angle = (position - 0.5) * 270.0;
    let share = angle.abs() / 135.0;
    let curved = 0.1 * share + 0.9 * share * share;
    if angle >= 0.0 {
        curved * 6.0
    } else {
        -curved * 40.0
    }
}

/// The channel gain as a linear multiplier, from the knob's position.
#[must_use]
pub fn gain_linear(position: f32) -> f32 {
    10.0_f32.powf(gain_decibels(position) / 20.0)
}

/// The master knob's law: a volume, not a trim, so it only attenuates — linear in decibels from −60 dB at the foot to
/// 0 dB at the top (the window's `MasterKnob`). The panel's master knob is the same one-sided scale.
#[must_use]
pub fn master_gain(position: f32) -> f32 {
    let db = -60.0 * (1.0 - position.clamp(0.0, 1.0));
    10.0_f32.powf(db / 20.0)
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
    pub fn command(self, deck: DeckId, pressed: bool, standing: DeckStanding) -> Option<DjCommand> {
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
                enabled: !standing.running,
            }),
            // Knobs, faders and pads carry a value or a slot, not a press: `Mapping::resolve_control` turns those into
            // commands, and this door stays about buttons.
            Self::Tempo
            | Self::ChannelFader
            | Self::Crossfader
            | Self::ChannelGain
            | Self::MasterGain
            | Self::ChannelCue
            | Self::Eq(_)
            | Self::Hotcue(_) => None,
            Self::PlayPause => None,
            // A sync button is a switch, and it reads whether the deck *follows*, not whether it plays: a deck that is
            // playing is the normal case, so asking the transport made every press a "let go" (owner's report, 25.11).
            // What it asks for is the beat — the same tempo and the same beat — which is what one SYNC button means on
            // the hardware this panel copies.
            Self::Sync if pressed => Some(DjCommand::Sync {
                deck,
                mode: if standing.synced {
                    SyncMode::Off
                } else {
                    SyncMode::Beat
                },
            }),
            Self::Sync => None,
            // Reverse and slip are switches of the same kind: the lamp and the press are one decision, and a press that
            // guessed the state would leave the deck in the one the DJ is not looking at.
            Self::Reverse if pressed => Some(DjCommand::SetReverse {
                deck,
                enabled: !standing.reversed,
            }),
            Self::Reverse => None,
            Self::Slip if pressed => Some(DjCommand::SetSlip {
                deck,
                enabled: !standing.slipped,
            }),
            Self::Slip => None,
            // The lead is a deck, not a number: the others need its beat, not only its tempo (DJP-SYNC-001). The same
            // button gives it up, which is what the window's MASTER does (`isMaster ? clearMaster : setMaster`).
            Self::Master if pressed => Some(if standing.master {
                DjCommand::MasterOff
            } else {
                DjCommand::Master { deck }
            }),
            Self::Master => None,
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
        "master" => Some(Intent::Master),
        "reverse" => Some(Intent::Reverse),
        "slip" => Some(Intent::Slip),
        "cue" => Some(Intent::CueJump),
        "cue_set" => Some(Intent::CueSet),
        "tempo" => Some(Intent::Tempo),
        "fader" => Some(Intent::ChannelFader),
        "gain" => Some(Intent::ChannelGain),
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
        "mixer.master" => Some(Intent::MasterGain),
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
            .command(
                deck.id(),
                pressed,
                DeckStanding {
                    running,
                    ..DeckStanding::default()
                },
            )
            .map(|command| (deck.id(), command))
    }
}

impl Mapping {
    /// Turns one event into the command it asks for, for controls that carry a value or a slot.
    ///
    /// `standing` is what the deck is doing — playing, following, leading —, `pad` whether the pad's slot already holds a
    /// cue: a pad with a cue jumps to it, an empty pad stores the cue where the deck stands.
    #[must_use]
    pub fn resolve_control(
        &self,
        event: &crate::ControlEvent,
        standing: DeckStanding,
        pad: PadState,
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
                // The travel of the hardware fader is the handle's position; the law turns it into the value the engine
                // wants.
                position: fader_value(event.value),
            }),
            Intent::Crossfader => Some(DjCommand::SetCrossfader {
                position: event.value.clamp(0.0, 1.0),
            }),
            Intent::ChannelGain => Some(DjCommand::SetChannelGain {
                deck: deck?,
                linear_gain: gain_linear(event.value),
            }),
            Intent::MasterGain => Some(DjCommand::SetMasterGain {
                linear_gain: master_gain(event.value),
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
                let deck = deck?;
                // The window's own rule, button for button: a pad with something on it jumps to it — a loop is jumped to
                // the same way a cue is —, an empty pad takes the deck's active loop when there is one, and only a pad
                // with neither stores a cue.
                if pad.cue || pad.looped {
                    Some(DjCommand::Cue {
                        deck,
                        action: CueAction::Jump(slot),
                    })
                } else if pad.active_loop {
                    Some(DjCommand::SetPadLoop { deck, slot })
                } else {
                    Some(DjCommand::Cue {
                        deck,
                        action: CueAction::Set(slot),
                    })
                }
            }
            _ => {
                let command = intent.command(deck?, pressed, standing)?;
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
        match mapping.resolve_control(&middle, DeckStanding::default(), PadState::default()) {
            Some((Some(DeckId::A), DjCommand::SetTempoRatio { ratio, .. })) => {
                assert!((ratio - 1.0).abs() < 1e-9, "{ratio}");
            }
            other => panic!("{other:?}"),
        }
        let top = ControlEvent::level("deck_a.tempo", 1.0);
        match mapping.resolve_control(&top, DeckStanding::default(), PadState::default()) {
            Some((Some(DeckId::A), DjCommand::SetTempoRatio { ratio, .. })) => {
                assert!(
                    (ratio - f64::from(1.0 + PITCH_RANGE)).abs() < 1e-6,
                    "{ratio}"
                );
            }
            other => panic!("{other:?}"),
        }
        let bottom = ControlEvent::level("deck_a.tempo", 0.0);
        match mapping.resolve_control(&bottom, DeckStanding::default(), PadState::default()) {
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
        match mapping.resolve_control(&fader, DeckStanding::default(), PadState::default()) {
            Some((Some(DeckId::B), DjCommand::SetChannelFader { position, .. })) => {
                assert!((position - fader_value(0.7)).abs() < 1e-6, "{position}");
                assert!(
                    position > 0.2 && position < 0.5,
                    "the law, not the travel: {position}"
                );
            }
            other => panic!("{other:?}"),
        }
        let crossfader = ControlEvent::level("mixer.crossfader", 0.25);
        assert_eq!(
            mapping.resolve_control(&crossfader, DeckStanding::default(), PadState::default()),
            Some((None, DjCommand::SetCrossfader { position: 0.25 })),
            "the crossfader belongs to the mixer, not to a deck"
        );
    }

    #[test]
    fn a_channel_fader_follows_the_window_s_law() {
        // The handle's travel is even; the value it is worth is not, and the law is the window's own.
        assert!((fader_value(1.0) - 1.0).abs() < 1e-6, "the top is unity");
        assert!(fader_value(0.0) < 1e-6, "the bottom is silence");
        assert!(
            (fader_value(0.5) - 0.125).abs() < 1e-3,
            "half the travel is a quarter of the way up the range: {}",
            fader_value(0.5)
        );
        // Monotone: a hand going up always goes up.
        let mut previous = -1.0;
        for step in 0..=100 {
            let value = fader_value(step as f32 / 100.0);
            assert!(value >= previous, "step {step}: {value} < {previous}");
            previous = value;
        }
        let mapping = Mapping::new();
        let event = ControlEvent::level("deck_a.fader", 0.5);
        match mapping.resolve_control(&event, DeckStanding::default(), PadState::default()) {
            Some((Some(DeckId::A), DjCommand::SetChannelFader { position, .. })) => {
                assert!((position - 0.125).abs() < 1e-3, "{position}");
            }
            other => panic!("{other:?}"),
        }
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
        match mapping.resolve_control(&knob, DeckStanding::default(), PadState::default()) {
            Some((Some(DeckId::A), DjCommand::SetChannelEq { band, gain_db, .. })) => {
                assert_eq!(band, dj_domain::EqBand::Low);
                assert!((gain_db - 3.0).abs() < 1e-6, "{gain_db}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_gain_knob_and_the_master_follow_the_window_s_law() {
        // The gain knob stands at unity in the middle, cuts to −40 dB and boosts to +6 dB through the window's cubic.
        assert!(
            (gain_decibels(0.5) - 0.0).abs() < 1e-6,
            "the middle is unity"
        );
        assert!(
            (gain_decibels(1.0) - 6.0).abs() < 1e-6,
            "the top is the boost"
        );
        assert!(
            (gain_decibels(0.0) + 40.0).abs() < 1e-6,
            "the bottom is the cut"
        );
        assert!(
            (gain_linear(0.5) - 1.0).abs() < 1e-6,
            "unity is a gain of one"
        );
        // The master only attenuates: unity at the top, silence (10^-3) at the foot, linear in decibels.
        assert!((master_gain(1.0) - 1.0).abs() < 1e-6);
        assert!(master_gain(0.0) < 0.01 && master_gain(0.0) > 0.0);
        let mapping = Mapping::new();
        let gain = ControlEvent::level("deck_a.gain", 0.5);
        match mapping.resolve_control(&gain, DeckStanding::default(), PadState::default()) {
            Some((Some(DeckId::A), DjCommand::SetChannelGain { linear_gain, .. })) => {
                assert!((linear_gain - 1.0).abs() < 1e-6, "{linear_gain}");
            }
            other => panic!("{other:?}"),
        }
        let master = ControlEvent::level("mixer.master", 1.0);
        assert_eq!(
            mapping.resolve_control(&master, DeckStanding::default(), PadState::default()),
            Some((None, DjCommand::SetMasterGain { linear_gain: 1.0 })),
        );
    }

    #[test]
    fn a_headphone_button_says_whether_the_channel_is_in_the_headphones() {
        let mapping = Mapping::new();
        let press = ControlEvent::button("deck_a.pfl", true);
        assert_eq!(
            mapping.resolve_control(&press, DeckStanding::default(), PadState::default()),
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
            mapping.resolve_control(&release, DeckStanding::default(), PadState::default()),
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
    fn a_pad_jumps_to_what_it_holds_and_never_overwrites_a_loop() {
        let mapping = Mapping::new();
        let press = ControlEvent::button("deck_a.hotcue3", true);
        let with_cue = PadState {
            cue: true,
            looped: false,
            active_loop: false,
        };
        let with_loop = PadState {
            cue: false,
            looped: true,
            active_loop: false,
        };
        let with_active_loop = PadState {
            cue: false,
            looped: false,
            active_loop: true,
        };
        // A slot with a cue jumps to it.
        assert_eq!(
            mapping.resolve_control(&press, DeckStanding::default(), with_cue),
            Some((
                Some(DeckId::A),
                DjCommand::Cue {
                    deck: DeckId::A,
                    action: dj_domain::CueAction::Jump(2)
                }
            ))
        );
        // A slot with a loop jumps to it the same way. Calling it empty wrote a cue into the slot and the loop vanished
        // from the panel (owner's report, 24.11).
        assert_eq!(
            mapping.resolve_control(&press, DeckStanding::default(), with_loop),
            Some((
                Some(DeckId::A),
                DjCommand::Cue {
                    deck: DeckId::A,
                    action: dj_domain::CueAction::Jump(2)
                }
            )),
            "a loop pad is jumped to, not overwritten"
        );
        // An empty pad takes the deck's own loop when one is active.
        assert_eq!(
            mapping.resolve_control(&press, DeckStanding::default(), with_active_loop),
            Some((
                Some(DeckId::A),
                DjCommand::SetPadLoop {
                    deck: DeckId::A,
                    slot: 2
                }
            ))
        );
        // A pad with nothing at all and no active loop stores a cue.
        assert_eq!(
            mapping.resolve_control(&press, DeckStanding::default(), PadState::default()),
            Some((
                Some(DeckId::A),
                DjCommand::Cue {
                    deck: DeckId::A,
                    action: dj_domain::CueAction::Set(2)
                }
            ))
        );
        // Releasing asks for nothing, whatever the pad holds.
        let release = ControlEvent::button("deck_a.hotcue3", false);
        assert_eq!(
            mapping.resolve_control(&release, DeckStanding::default(), with_cue),
            None
        );
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
        let pressed = Intent::Play.command(DeckId::A, true, DeckStanding::default());
        assert_eq!(
            pressed,
            Some(DjCommand::Play {
                deck: DeckId::A,
                enabled: true
            })
        );
        assert_eq!(
            Intent::Play.command(
                DeckId::A,
                false,
                DeckStanding {
                    running: true,
                    ..DeckStanding::default()
                }
            ),
            None
        );
    }

    #[test]
    fn the_toggle_reads_the_transport_not_the_button() {
        assert_eq!(
            Intent::PlayPause.command(DeckId::B, true, DeckStanding::default()),
            Some(DjCommand::Play {
                deck: DeckId::B,
                enabled: true
            })
        );
        assert_eq!(
            Intent::PlayPause.command(
                DeckId::B,
                true,
                DeckStanding {
                    running: true,
                    ..DeckStanding::default()
                }
            ),
            Some(DjCommand::Play {
                deck: DeckId::B,
                enabled: false
            })
        );
    }

    #[test]
    fn sync_is_a_switch_that_reads_whether_the_deck_follows() {
        // A deck that plays but follows nothing is the case a SYNC is pressed in during a mix: it must turn sync on, and
        // asking the transport instead turned it off on every playing deck (owner's report, 25.11).
        let playing_alone = DeckStanding {
            running: true,
            ..DeckStanding::default()
        };
        assert_eq!(
            Intent::Sync.command(DeckId::C, true, playing_alone),
            Some(DjCommand::Sync {
                deck: DeckId::C,
                mode: SyncMode::Beat
            })
        );
        let following = DeckStanding {
            running: true,
            synced: true,
            ..DeckStanding::default()
        };
        assert_eq!(
            Intent::Sync.command(DeckId::C, true, following),
            Some(DjCommand::Sync {
                deck: DeckId::C,
                mode: SyncMode::Off
            })
        );
        // Standing still does not change what the button means: the deck's own sync state does.
        assert_eq!(
            Intent::Sync.command(
                DeckId::C,
                true,
                DeckStanding {
                    synced: true,
                    ..DeckStanding::default()
                }
            ),
            Some(DjCommand::Sync {
                deck: DeckId::C,
                mode: SyncMode::Off
            })
        );
        // A release says nothing: one press, one decision.
        assert_eq!(Intent::Sync.command(DeckId::C, false, following), None);
    }

    #[test]
    fn the_master_button_takes_the_lead_and_gives_it_up() {
        assert_eq!(
            Intent::Master.command(DeckId::B, true, DeckStanding::default()),
            Some(DjCommand::Master { deck: DeckId::B })
        );
        let leading = DeckStanding {
            master: true,
            ..DeckStanding::default()
        };
        assert_eq!(
            Intent::Master.command(DeckId::B, true, leading),
            Some(DjCommand::MasterOff)
        );
        assert_eq!(Intent::Master.command(DeckId::B, false, leading), None);
        assert_eq!(intent_for("deck_a.master"), Some(Intent::Master));
        assert_eq!(intent_for("deck_b.master"), Some(Intent::Master));
    }

    #[test]
    fn reverse_and_slip_are_switches_that_read_the_deck() {
        // Both are toggles on one button, and the lamp beside the button is the deck's own state: a press that read the
        // transport or guessed would set the state the DJ is not looking at (owner's request, 25.11).
        assert_eq!(
            Intent::Reverse.command(DeckId::A, true, DeckStanding::default()),
            Some(DjCommand::SetReverse {
                deck: DeckId::A,
                enabled: true
            })
        );
        let backwards = DeckStanding {
            reversed: true,
            ..DeckStanding::default()
        };
        assert_eq!(
            Intent::Reverse.command(DeckId::A, true, backwards),
            Some(DjCommand::SetReverse {
                deck: DeckId::A,
                enabled: false
            })
        );
        assert_eq!(Intent::Reverse.command(DeckId::A, false, backwards), None);
        assert_eq!(
            Intent::Slip.command(DeckId::B, true, DeckStanding::default()),
            Some(DjCommand::SetSlip {
                deck: DeckId::B,
                enabled: true
            })
        );
        let slipping = DeckStanding {
            slipped: true,
            ..DeckStanding::default()
        };
        assert_eq!(
            Intent::Slip.command(DeckId::B, true, slipping),
            Some(DjCommand::SetSlip {
                deck: DeckId::B,
                enabled: false
            })
        );
        // One press, one decision.
        assert_eq!(Intent::Slip.command(DeckId::B, false, slipping), None);
        assert_eq!(intent_for("deck_a.reverse"), Some(Intent::Reverse));
        assert_eq!(intent_for("deck_b.slip"), Some(Intent::Slip));
    }

    #[test]
    fn the_cue_button_is_the_deck_s_own_cue_point() {
        assert_eq!(
            Intent::CueJump.command(DeckId::A, true, DeckStanding::default()),
            Some(DjCommand::Cue {
                deck: DeckId::A,
                action: CueAction::Jump(CUE_SLOT)
            })
        );
        assert_eq!(
            Intent::CueSet.command(
                DeckId::B,
                true,
                DeckStanding {
                    running: true,
                    ..DeckStanding::default()
                }
            ),
            Some(DjCommand::Cue {
                deck: DeckId::B,
                action: CueAction::Set(CUE_SLOT)
            })
        );
        // The release of either says nothing: the store and the jump are one gesture each.
        assert_eq!(
            Intent::CueJump.command(DeckId::A, false, DeckStanding::default()),
            None
        );
        assert_eq!(
            Intent::CueSet.command(DeckId::A, false, DeckStanding::default()),
            None
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
