//! The roll holds what came *after* the press, and it keeps holding it however long the switch is held.
//!
//! Owner's report, 25.11: «Такое ощущение что он неправильно работает». Two claims are measured here:
//!
//! 1. the first pass is the deck itself — a roll captures the beat that *follows* the press, so pressing must be
//!    inaudible until the loop comes round (the slice used to be taken from *before* the press, so the output jumped
//!    backwards in the material);
//! 2. the loop keeps what it captured — a held roll must repeat the same slice however long it is held (the ring behind
//!    it used to keep taking material, so after two seconds the captured slice had been overwritten by what followed).

mod support;

use dj_fx::{FxUnit, Roll};
use support::{BLOCK, RATE};

/// A stereo buffer of one tone per second, so a slice's material says which second it came from.
fn tones(seconds: usize, per_second: &[f32]) -> Vec<f32> {
    let frames = RATE as usize * seconds;
    let mut out = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let second = (frame / RATE as usize).min(per_second.len() - 1);
        let sample =
            (std::f32::consts::TAU * per_second[second] * (frame as f32 / RATE)).sin() * 0.4;
        out.push(sample);
        out.push(sample);
    }
    out
}

/// The amplitude of one frequency in a window: the tone's own amplitude when the window is a whole number of periods,
/// and near zero when the frequency is not there.
fn amplitude_at(window: &[f32], hz: f32) -> f32 {
    let (mut sine, mut cosine) = (0.0f32, 0.0f32);
    for (index, sample) in window.iter().enumerate() {
        let phase = std::f32::consts::TAU * hz * (index as f32 / RATE);
        sine += sample * phase.sin();
        cosine += sample * phase.cos();
    }
    2.0 * (sine * sine + cosine * cosine).sqrt() / window.len() as f32
}

/// The left channel of a half-open window of frames.
fn left_window(samples: &[f32], from: usize, to: usize) -> Vec<f32> {
    samples[from * 2..to * 2]
        .iter()
        .step_by(2)
        .copied()
        .collect()
}

#[test]
fn the_first_pass_is_the_deck_itself() {
    // A continuous tone: a roll that captures what comes next repeats the material the deck was about to play, so the
    // first pass through the loop is the deck sample for sample. Capturing backwards instead makes the press a jump
    // back into the tone — a click and the wrong slice.
    let mut roll = Roll::new();
    roll.prepare(RATE, BLOCK);
    roll.set_beat(0.5);
    roll.set_amount(0.0);
    let length = roll.length_frames();
    assert_eq!(length, 3_000, "an eighth of a beat at 48 kHz");

    let input = tones(2, &[220.0]);
    // The press has to fall on a block boundary: the harness walks the input block by block, and 48 000 is not a
    // multiple of the block — the switch would never go down and the test would pass by asking nothing (it did).
    let press = (48_000 / BLOCK) * BLOCK;
    let mut out = vec![0.0; input.len()];
    let frames = input.len() / 2;
    let mut at = 0;
    while at + BLOCK <= frames {
        if at == press {
            roll.engage(true);
        }
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        roll.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }

    // The first pass: exactly the deck. Its last eight milliseconds are the seam's crossfade — the tail blended into
    // the head, which is what keeps the join silent — so the claim covers the pass up to the fade.
    let fade = (0.008 * RATE) as usize;
    let first = left_window(&out, press, press + length - fade);
    let deck = left_window(&input, press, press + length - fade);
    let worst = first
        .iter()
        .zip(deck.iter())
        .map(|(one, other)| (one - other).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst <= 1.0e-6,
        "the first pass is not the deck itself: it differs by {worst:.4} at the widest"
    );

    // And the crossfade at the end of the pass does not click: a 220 Hz tone at this amplitude steps by 0.012 a sample.
    let seam = left_window(&out, press + length - fade, press + length * 2);
    let step = seam
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        step <= 0.02,
        "the seam stepped by {step:.4} between two samples"
    );

    // And after it, the same slice again: the loop repeats what the press started, not what preceded it.
    let second = left_window(&out, press + length, press + length * 2);
    assert!(
        amplitude_at(&second, 220.0) > 0.2,
        "the loop does not repeat the tone it captured: {} of it",
        amplitude_at(&second, 220.0)
    );
}

#[test]
fn a_held_roll_keeps_repeating_what_it_captured() {
    // The deck changes tone every second: 200, then 800, then 1600 Hz. The hand presses in the middle of the 800 Hz
    // second and holds for three more: the 1600 Hz that follows the loop must never reach the output, however long the
    // hold — a roll repeats the slice the press started, not a moving window of whatever arrives.
    let mut roll = Roll::new();
    roll.prepare(RATE, BLOCK);
    roll.set_beat(0.5);
    roll.set_amount(0.0);

    let input = tones(5, &[200.0, 800.0, 1_600.0]);
    let press = (48_000 / BLOCK) * BLOCK + (24_000 / BLOCK) * BLOCK;
    let mut out = vec![0.0; input.len()];
    let frames = input.len() / 2;
    let mut at = 0;
    while at + BLOCK <= frames {
        if at == press {
            roll.engage(true);
        }
        let (from, to) = (at * 2, (at + BLOCK) * 2);
        roll.process(&input[from..to], &mut out[from..to], BLOCK);
        at += BLOCK;
    }

    // Windows of 0.1 s, from half a second into the hold (the capture is 800 Hz and the deck is 1600 Hz after the second
    // boundary at 2 s) to the end.
    // Windows of a tenth of a second, from half a second into the hold (the deck is 1600 Hz from the second boundary
    // two seconds in) to the end. The peak carries "the captured tone is still playing" — a coherent projection of it
    // is not a fair measure, because the seam moves its phase by design — and the projection carries "the material that
    // followed the capture is not playing".
    let window = 4_800;
    let mut at = press + 24_000;
    while at + window <= frames {
        let slice = left_window(&out, at, at + window);
        let peak = slice
            .iter()
            .fold(0.0f32, |worst, sample| worst.max(sample.abs()));
        let later = amplitude_at(&slice, 1_600.0);
        assert!(
            later < 0.05,
            "at {at} the held loop is playing the 1600 Hz that followed it ({later:.3})"
        );
        assert!(
            peak > 0.3,
            "at {at} the held loop lost the material it captured: its peak is {peak:.3}"
        );
        at += window;
    }
}

#[test]
fn a_patch_message_does_not_restart_the_loop() {
    // The rack sends the whole patch on every change and the switch itself sends one: the loop's phase must survive a
    // message that says nothing new, or a held roll stutters on every press and every knob report.
    let input = tones(2, &[330.0]);
    let press = (24_000 / BLOCK) * BLOCK;

    let quiet = {
        let mut roll = Roll::new();
        roll.prepare(RATE, BLOCK);
        roll.set_beat(0.5);
        roll.set_amount(0.0);
        let mut out = vec![0.0; input.len()];
        let frames = input.len() / 2;
        let mut at = 0;
        while at + BLOCK <= frames {
            if at == press {
                roll.engage(true);
            }
            let (from, to) = (at * 2, (at + BLOCK) * 2);
            roll.process(&input[from..to], &mut out[from..to], BLOCK);
            at += BLOCK;
        }
        out
    };

    let said = {
        let mut roll = Roll::new();
        roll.prepare(RATE, BLOCK);
        roll.set_beat(0.5);
        roll.set_amount(0.0);
        let mut out = vec![0.0; input.len()];
        let frames = input.len() / 2;
        let mut at = 0;
        while at + BLOCK <= frames {
            if at == press {
                roll.engage(true);
            }
            if at == press + (8_192 / BLOCK) * BLOCK {
                roll.set_amount(0.0);
            }
            let (from, to) = (at * 2, (at + BLOCK) * 2);
            roll.process(&input[from..to], &mut out[from..to], BLOCK);
            at += BLOCK;
        }
        out
    };

    let mut loudest = 0.0f32;
    for (one, other) in quiet.iter().zip(said.iter()) {
        loudest = loudest.max((one - other).abs());
    }
    assert!(
        loudest <= 1.0e-6,
        "a patch that said nothing new moved the loop: the two runs differ by {loudest:.4}"
    );
}
