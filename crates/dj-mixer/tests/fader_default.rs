//! Where a channel's fader stands when nothing has said otherwise.
//!
//! The controller is silent about its knobs until a hand touches one (`--inputs-watch` at rest shows only the jog
//! counters), so the engine's default is exactly what the window shows at startup. It must be the safe position: a channel
//! that starts at unity opens a track into the room the moment it is loaded.

use dj_mixer::ChannelSettings;

#[test]
fn a_channel_starts_with_its_fader_down() {
    assert_eq!(ChannelSettings::default().fader, 0.0);
}
