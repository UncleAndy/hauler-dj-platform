//! The deck's effect in its channel strip: before the fader, under the monitoring, and silent when it is off.
//!
//! These are the three things the slot promises (DJP-FX-003 insert, owner's decision of 21.11): what the DJ hears in
//! the headphones is what the effect does, the fader still decides how much reaches the room, and "off" means the
//! block comes out exactly as the deck rendered it.

use dj_domain::{FxKind, FxPatch};
use dj_mixer::{ChannelSettings, CrossAssign, CrossfaderCurve, Mixer};

const FRAMES: usize = 256;
const RATE: u32 = 48_000;

fn tone(frequency: f32, amplitude: f32, frames: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let phase = 2.0 * std::f32::consts::PI * frequency * frame as f32 / RATE as f32;
        let value = amplitude * phase.sin();
        samples.push(value);
        samples.push(value);
    }
    samples
}

fn mixer(fader: f32, cue: bool) -> Mixer {
    let mut mixer = Mixer::new(2);
    mixer.prepare(RATE, FRAMES);
    mixer.set_curve(CrossfaderCurve::ConstantPower);
    mixer.set_crossfader(0.0);
    mixer.set_cue_mix(1.0);
    mixer.set_channel(
        0,
        ChannelSettings {
            trim_db: 0.0,
            fader,
            assign: CrossAssign::A,
            cue,
        },
    );
    mixer
}

/// Render one block of the first channel through the effect and the strip, returning what the two buses got.
fn render(mixer: &mut Mixer, block: &mut [f32]) -> (Vec<f32>, Vec<f32>) {
    mixer.begin_block(FRAMES);
    mixer.process_fx(0, block, FRAMES);
    mixer.mix_channel(0, block);
    mixer.finish_block();
    (mixer.master().to_vec(), mixer.cue().to_vec())
}

fn rms(samples: &[f32]) -> f64 {
    let count = (samples.len() / 2).max(1);
    let sum: f64 = samples
        .iter()
        .step_by(2)
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum();
    (sum / count as f64).sqrt()
}

#[test]
fn the_effect_sits_before_the_fader_and_under_the_monitoring() {
    let mut mixer = mixer(0.5, true);
    mixer.set_deck_fx(
        0,
        FxPatch {
            kind: FxKind::Bitcrusher,
            amount: 1.0,
            enabled: true,
        },
    );

    // Four blocks: the limiter has a lookahead, and the switch fades in over 20 ms.
    let mut master = Vec::new();
    let mut cue = Vec::new();
    for _ in 0..6 {
        let mut block = tone(300.0, 0.5, FRAMES);
        let (master_block, cue_block) = render(&mut mixer, &mut block);
        master = master_block;
        cue = cue_block;
    }
    assert!(rms(&master) > 0.0, "the effect silenced the room");
    assert!(rms(&cue) > 0.0, "the monitoring does not hear the effect");

    // The room is at half the fader and the crusher has put the signal on a coarse grid: the master's samples land
    // on that grid times the fader, which is what "before the fader" means.
    let quantum = 0.125 * 0.5;
    let on_grid = master
        .iter()
        .step_by(2)
        .filter(|v| {
            let steps = *v / quantum;
            (steps - steps.round()).abs() < 0.02
        })
        .count();
    let total = master.len() / 2;
    assert!(
        on_grid * 2 > total,
        "the room does not carry the crushed signal: {on_grid} of {total} samples on the grid"
    );
}

#[test]
fn a_lowered_fader_silences_the_room_and_not_the_headphones() {
    let mut mixer = mixer(0.0, true);
    mixer.set_deck_fx(
        0,
        FxPatch {
            kind: FxKind::Bitcrusher,
            amount: 1.0,
            enabled: true,
        },
    );
    let mut master = Vec::new();
    let mut cue = Vec::new();
    for _ in 0..6 {
        let mut block = tone(300.0, 0.5, FRAMES);
        let (master_block, cue_block) = render(&mut mixer, &mut block);
        master = master_block;
        cue = cue_block;
    }
    assert!(rms(&master) < 1e-6, "a fader at zero still feeds the room");
    assert!(rms(&cue) > 0.01, "the monitoring went quiet with the fader");
}

#[test]
fn the_effect_is_not_audible_when_it_is_off() {
    let mut mixer = mixer(1.0, false);
    // Default patch: the filter at its centre, switched out.
    mixer.set_deck_fx(0, FxPatch::default());
    let mut block = tone(440.0, 0.3, FRAMES);
    let before = block.clone();
    let _ = render(&mut mixer, &mut block);
    assert_eq!(block, before, "a switched-off effect changed the block");
}

#[test]
fn the_switch_fades_rather_than_steps() {
    // 20 Hz, so a jump in gain is visible against the signal's own 0.0013 step per sample.
    let mut mixer = mixer(1.0, false);
    // One continuous tone sliced per block: a tone restarted every block puts its own steps in the measurement, and
    // the test would blame the switch.
    let signal = tone(20.0, 0.5, 40 * FRAMES);
    let mut out = Vec::new();
    for index in 0..40 {
        if index == 10 {
            mixer.set_deck_fx(
                0,
                FxPatch {
                    kind: FxKind::Flanger,
                    amount: 1.0,
                    enabled: true,
                },
            );
        }
        let mut block = signal[index * FRAMES * 2..(index + 1) * FRAMES * 2].to_vec();
        let (master, _) = render(&mut mixer, &mut block);
        out.extend_from_slice(&master);
    }
    let mut worst = 0.0f32;
    for pair in out.windows(2) {
        worst = worst.max((pair[1] - pair[0]).abs());
    }
    assert!(worst <= 0.05, "the switch stepped by {worst:.4}");
}
