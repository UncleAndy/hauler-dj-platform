//! Device abstraction, capability negotiation and mapping.
//!
//! What every controller driver shares: one shape for a control event, one way to read bits out of a report, one way
//! to turn a jog wheel's absolute counter into speed, and the layer state that decides what a button means while
//! shift is held (ADR-0006).
//!
//! The crate holds no device facts. Bytes belong to a driver (`dj-controller-s4mk3`), and a driver is written from
//! `docs/hardware/<device>/protocol-notes.md`, never from somebody else's source.
//!
//! - Backlog task: A09
//! - Requirements: DJP-CTRL-001, DJP-CTRL-002, DJP-CTRL-004
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod layers;
pub mod mapping;
pub mod report;
pub mod wheel;

use std::time::Duration;

pub use layers::{DeckLayer, LayerChange, Layers};
pub use mapping::{BindableDeck, Intent, Mapping, PadState, intent_for};
pub use wheel::WheelCounter;

/// One thing the DJ did, and when the device said so.
///
/// The shape is the same for every driver and every control, because everything downstream — the mapping table, the
/// command queue, the log — must not care which device produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlEvent {
    /// A stable name, `deck_a.play` or `wheel_left`: words, not numbers, so a log line means something.
    pub control: String,
    /// `1.0` press and `0.0` release for a button; `0.0…1.0` for a fader; speed for a wheel.
    pub value: f32,
    /// The device's own clock when the report carried one. `None` for a device that does not say.
    pub at: Option<Duration>,
}

impl ControlEvent {
    /// A button going down or up.
    #[must_use]
    pub fn button(control: impl Into<String>, pressed: bool) -> Self {
        Self {
            control: control.into(),
            value: if pressed { 1.0 } else { 0.0 },
            at: None,
        }
    }

    /// Anything that is not a plain press: a fader, a knob, a wheel.
    #[must_use]
    pub fn level(control: impl Into<String>, value: f32) -> Self {
        Self {
            control: control.into(),
            value,
            at: None,
        }
    }
}

/// What a driver must do: turn reports into events.
///
/// No device is opened here, and nothing is sent: the direction of A09 is one way, from the device to us.
pub trait Driver {
    /// The driver's own name, for the log.
    fn name(&self) -> &'static str;

    /// Turns one report into the events it carries. An empty answer is normal: a report of buttons we have no
    /// verified map for yet is understood by nobody, and guessing offsets would be worse than silence.
    fn feed(&mut self, report: &[u8]) -> Vec<ControlEvent>;
}

/// A driver that finds nothing, for when controller support is switched off in settings.
///
/// It is not `Option<Box<dyn Driver>>` on purpose: a null driver keeps the call sites free of branches, so switching
/// the feature off cannot change what the rest of the program does — the difference is only that no events arrive.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullDriver;

impl Driver for NullDriver {
    fn name(&self) -> &'static str {
        "none"
    }

    fn feed(&mut self, _report: &[u8]) -> Vec<ControlEvent> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_carries_one_and_a_release_zero() {
        assert!((ControlEvent::button("deck_a.play", true).value - 1.0).abs() < f32::EPSILON);
        assert!(ControlEvent::button("deck_a.play", false).value.abs() < f32::EPSILON);
        assert!((ControlEvent::level("deck_a.tempo", 0.25).value - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn the_null_driver_answers_nothing_and_says_so() {
        let mut driver = NullDriver;
        assert_eq!(driver.name(), "none");
        assert!(driver.feed(&[3, 0, 0, 0]).is_empty());
    }
}
