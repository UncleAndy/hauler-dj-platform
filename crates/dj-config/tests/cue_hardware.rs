//! Who turns the two headphone knobs: the window or the controller.
//!
//! The S4 MK3 has a headphone volume knob and a cue/master knob of its own, and it does not tell us where they are.
//! The window therefore hands those two over while the controller is in use, and keeps them when it is not.

use dj_config::{Config, ControllerDriver, hardware_owns_cue};

#[test]
fn a_fresh_settings_file_keeps_the_knobs_in_the_window() {
    let config = Config::default();
    assert!(!hardware_owns_cue(&config.controller));
}

#[test]
fn the_controller_takes_the_knobs_over() {
    let mut config = Config::default();
    config.controller.enabled = true;
    config.controller.driver = ControllerDriver::S4Mk3;
    assert!(hardware_owns_cue(&config.controller));
}

/// A setting that is on but claims no device is not a controller: nothing hardware is there to turn anything.
#[test]
fn a_switch_without_a_device_takes_nothing() {
    let mut config = Config::default();
    config.controller.enabled = true;
    config.controller.driver = ControllerDriver::None;
    assert!(!hardware_owns_cue(&config.controller));
}

/// A device that is not wired in yet cannot own a knob either.
#[test]
fn a_known_driver_that_is_switched_off_owns_nothing() {
    let mut config = Config::default();
    config.controller.enabled = false;
    config.controller.driver = ControllerDriver::S4Mk3;
    assert!(!hardware_owns_cue(&config.controller));
}
