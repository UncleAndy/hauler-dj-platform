//! Monitoring: the cue bus is taken after the deck's own volume (trim) and before the channel fader, and the
//! headphones hear a proportion of that bus against the main mix.
//!
//! Owner's decisions of 20.11: monitoring is pre-fader but post-gain; the proportion knob decides how much of the
//! monitoring against how much of the main mix goes to the headphones.
//!
//! Every case renders four blocks and reads the last one: the master limiter has a lookahead, so the first blocks
//! come out short while the delay line fills, and a test that reads the first block measures the delay instead of
//! the mixing.

use dj_mixer::{ChannelSettings, CrossAssign, CrossfaderCurve, Mixer};

const FRAMES: usize = 256;
const BLOCKS: usize = 4;

fn channel(fader: f32, trim_db: f32, cue: bool) -> ChannelSettings {
    ChannelSettings {
        trim_db,
        fader,
        assign: CrossAssign::A,
        cue,
    }
}

/// What the two buses carry once the graph is warm: call it with the channels' settings and the two inputs.
fn render(channels: &[ChannelSettings], inputs: &[f32], cue_mix: f32, cue_gain: f32) -> (Vec<f32>, Vec<f32>) {
    let mut mixer = Mixer::new(channels.len());
    mixer.prepare(48_000, FRAMES);
    mixer.set_curve(CrossfaderCurve::ConstantPower);
    // Fully to the left: the first channel is in the room, the second is not.
    mixer.set_crossfader(0.0);
    mixer.set_cue_mix(cue_mix);
    mixer.set_cue_gain(cue_gain);
    for (index, settings) in channels.iter().enumerate() {
        // The first channel follows the left side of the crossfader, every other one the right side: with the
        // crossfader fully to the left, only the first channel is heard in the room.
        let assign = if index == 0 { CrossAssign::A } else { CrossAssign::B };
        mixer.set_channel(index, ChannelSettings { assign, ..*settings });
    }
    for _ in 0..BLOCKS {
        mixer.begin_block(FRAMES);
        for (index, level) in inputs.iter().enumerate() {
            mixer.mix_channel(index, &vec![*level; FRAMES * 2]);
        }
        mixer.finish_block();
    }
    (mixer.master().to_vec(), mixer.cue().to_vec())
}

fn peak(block: &[f32]) -> f32 {
    block.iter().fold(0.0f32, |held, sample| held.max(sample.abs()))
}

#[test]
fn cue_mix_at_one_leaves_only_the_cued_deck() {
    // Monitoring only: the headphones hear deck B, which the crossfader keeps out of the room.
    let channels = [channel(1.0, 0.0, false), channel(1.0, 0.0, true)];
    let (master, cue) = render(&channels, &[0.5, 0.25], 1.0, 1.0);
    assert!(
        (peak(&master) - 0.5).abs() < 0.01,
        "the room hears deck A at 0.5, not {}",
        peak(&master)
    );
    assert!(
        (peak(&cue) - 0.25).abs() < 0.01,
        "the headphones hear deck B at 0.25, not {}",
        peak(&cue)
    );
}

#[test]
fn cue_mix_at_zero_gives_the_main_mix() {
    // The other end of the knob: the headphones hear exactly what the room hears.
    let channels = [channel(1.0, 0.0, false), channel(1.0, 0.0, true)];
    let (master, cue) = render(&channels, &[0.5, 0.25], 0.0, 1.0);
    assert!(
        (peak(&cue) - peak(&master)).abs() < 0.01,
        "at mix 0 the headphones repeat the main mix: {} against {}",
        peak(&cue),
        peak(&master)
    );
    // Sample for sample, not just in peak: at mix 0 the headphones are the main mix, deck B included in neither.
    assert!(
        cue.iter().zip(&master).all(|(heard, room)| (heard - room).abs() < 1e-6),
        "at mix 0 the headphone bus equals the master bus"
    );
}

#[test]
fn cue_gain_scales_the_headphones_only() {
    let channels = [channel(1.0, 0.0, false), channel(1.0, 0.0, true)];
    let (master, cue) = render(&channels, &[0.5, 0.25], 1.0, 0.0);
    assert!(peak(&cue) < 1e-4, "gain 0 gives silence in the headphones");
    assert!(
        (peak(&master) - 0.5).abs() < 0.01,
        "the room is untouched by the headphone knob: {}",
        peak(&master)
    );
}

#[test]
fn a_cued_channel_is_heard_before_its_fader() {
    // Deck A is cued and its fader is at zero: the room hears nothing of it, the DJ hears all of it.
    let channels = [channel(0.0, 0.0, true), channel(1.0, 0.0, false)];
    let (master, cue) = render(&channels, &[0.5, 0.25], 1.0, 1.0);
    assert!(
        (peak(&cue) - 0.5).abs() < 0.01,
        "a cued deck with its fader down is still monitored: {}",
        peak(&cue)
    );
    // Deck B sits on the right side of the crossfader, which is closed: the room hears nothing at all.
    assert!(
        peak(&master) < 1e-4,
        "neither deck reaches the room: {}",
        peak(&master)
    );
}

#[test]
fn the_cue_tap_sits_after_the_deck_gain() {
    // The deck's own volume knob must reach the headphones: with it down, monitoring is silent.
    let down = [channel(1.0, -60.0, true)];
    let (_, cue) = render(&down, &[0.5], 1.0, 1.0);
    assert!(
        peak(&cue) < 0.01,
        "the deck's volume knob at -60 dB must silence the headphones, not {}",
        peak(&cue)
    );

    // And the same block with the knob at unity is heard: the silence above is the knob, not the tap.
    let unity = [channel(1.0, 0.0, true)];
    let (_, cue) = render(&unity, &[0.5], 1.0, 1.0);
    assert!(
        (peak(&cue) - 0.5).abs() < 0.01,
        "with the knob at unity the headphones hear 0.5, not {}",
        peak(&cue)
    );
}
