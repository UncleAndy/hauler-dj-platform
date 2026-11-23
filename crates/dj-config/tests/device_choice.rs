//! What output the engine asks for when the launcher starts it.
//!
//! The launcher never knows a device name, so it passes `--device ""` and expects the settings file to answer
//! (owner's report, 2026-11-23: the controller was chosen in the window, the interface was not the system default,
//! and there was no sound — the empty name was taken literally and the engine opened the system default instead of
//! the chosen interface).

use dj_config::chosen_output_device;

/// The case from the report: the launcher's placeholder plus a device named in the window.
#[test]
fn a_placeholder_gives_the_settings_the_choice() {
    assert_eq!(
        chosen_output_device(Some(""), Some("Traktor Kontrol S4 MK3")),
        Some("Traktor Kontrol S4 MK3".to_string())
    );
}

/// No placeholder and no setting: the caller said nothing, so the engine stays on its silent null sink. A gate that
/// never asked for sound must not start making some.
#[test]
fn silence_stays_silence_when_nobody_asked() {
    assert_eq!(chosen_output_device(None, None), None);
    assert_eq!(
        chosen_output_device(None, Some("Traktor Kontrol S4 MK3")),
        None
    );
}

/// An explicit substring on the command line beats the file: it is what the person typed just now.
#[test]
fn an_explicit_name_beats_the_file() {
    assert_eq!(
        chosen_output_device(Some("ur22c"), Some("Traktor Kontrol S4 MK3")),
        Some("ur22c".to_string())
    );
}

/// Whitespace is not a name: a settings file with `device = "  "` must mean "the system's own choice", not a search
/// for a device whose name contains two spaces.
#[test]
fn whitespace_is_not_a_device_name() {
    assert_eq!(
        chosen_output_device(Some("  "), Some("  ")),
        Some(String::new())
    );
    assert_eq!(
        chosen_output_device(Some(" "), Some(" Traktor Kontrol S4 MK3 ")),
        Some("Traktor Kontrol S4 MK3".to_string())
    );
}

/// The launcher's placeholder with nothing in the file asks for the system default, which the audio layer reads from
/// an empty name — the same meaning the placeholder had before the settings file was consulted.
#[test]
fn a_placeholder_with_an_empty_file_asks_for_the_system_default() {
    assert_eq!(chosen_output_device(Some(""), None), Some(String::new()));
    assert_eq!(
        chosen_output_device(Some(""), Some("")),
        Some(String::new())
    );
}
