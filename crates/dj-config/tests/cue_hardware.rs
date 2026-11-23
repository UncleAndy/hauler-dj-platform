//! Who turns the two headphone knobs: the window or the controller.
//!
//! The S4 MK3 has a headphone volume knob and a cue/master knob of its own, and it does not tell us where they are.
//! The window therefore hands those two over while the controller is in use, and keeps them when it is not.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

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

/// The colours of the panel are settings: a fresh file keeps the colours the first build drew, and a file that names a
/// colour the device does not have is refused rather than silently ignored.
#[test]
fn a_fresh_file_keeps_the_colours_the_panel_already_had() {
    use dj_config::{ControllerLights, LightColour};
    let lights = ControllerLights::default();
    assert!(
        lights.night,
        "the panel glows faintly instead of going dark: a hand has to find the buttons"
    );
    assert_eq!(lights.resting, LightColour::White);
    assert_eq!(lights.play, LightColour::Green);
    assert_eq!(lights.cue, LightColour::Blue);
    assert_eq!(lights.sync, LightColour::Yellow);
    assert_eq!(lights.hotcue, LightColour::Green);
    assert_eq!(lights.fx, LightColour::Magenta);
}

#[test]
fn a_colour_that_is_not_in_the_table_is_refused() {
    // An unknown colour is a typo, and a typo that is quietly ignored is a panel that does not do what the file says.
    let text = "version = 1\n\n[controller.lights]\nplay = \"chartreuse\"\n";
    let parsed: Result<dj_config::Config, _> = toml::from_str(text);
    assert!(parsed.is_err(), "chartreuse is not a colour of this panel");
}

#[test]
fn the_colours_of_a_file_are_read_by_name() {
    let text =
        "version = 1\n\n[controller.lights]\nnight = true\nresting = \"sky\"\nplay = \"red\"\n";
    let config: dj_config::Config = toml::from_str(text).expect("the file is valid");
    assert!(config.controller.lights.night);
    assert_eq!(
        config.controller.lights.resting,
        dj_config::LightColour::Sky
    );
    assert_eq!(config.controller.lights.play, dj_config::LightColour::Red);
    // The rest of the section keeps its defaults, so a file that names one colour does not reset the others.
    assert_eq!(config.controller.lights.fx, dj_config::LightColour::Magenta);
}
