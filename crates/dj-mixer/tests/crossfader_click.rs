//! The fader does not click: a move between two blocks is a ramp, not a step.

#![allow(clippy::expect_used, clippy::cast_precision_loss)]

use dj_domain::CrossfaderCurve;
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};

const RATE: u32 = 48_000;
const BLOCK: usize = 256;

/// One channel of a sine, through the mixer, block by block: what the master bus actually gets.
fn mixed(mixer: &mut Mixer, blocks: usize) -> Vec<f32> {
    let mut heard = Vec::new();
    // The phase lives across calls: a sine restarted at zero between two windows reads as a full-scale step of
    // its own, which is not the mixer's.
    let mut phase = PHASE.with(|phase| *phase.borrow());
    for _ in 0..blocks {
        let mut input = vec![0.0f32; BLOCK * 2];
        for frame in 0..BLOCK {
            let value =
                (0.5 * (std::f64::consts::TAU * 220.0 * phase / f64::from(RATE)).sin()) as f32;
            input[frame * 2] = value;
            input[frame * 2 + 1] = value;
            phase += 1.0;
        }
        mixer.begin_block(BLOCK);
        mixer.mix_channel(0, &input);
        mixer.finish_block();
        // Both channels: the master is interleaved, so half a block of samples is half a block of time.
        heard.extend(mixer.master().iter().take(BLOCK * 2).step_by(2).copied());
    }
    PHASE.with(|cell| *cell.borrow_mut() = phase);
    heard
}

thread_local! {
    /// The sine's phase, kept between calls to `mixed` from one test.
    static PHASE: std::cell::RefCell<f64> = const { std::cell::RefCell::new(0.0) };
}

fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max)
}

fn mixer_on_b() -> Mixer {
    let mut mixer = Mixer::new(1);
    // The buses are allocated here: without it `begin_block` clamps the block to zero and nothing is heard.
    mixer.prepare(RATE, BLOCK);
    mixer.set_channel(
        0,
        ChannelSettings {
            trim_db: 0.0,
            fader: 1.0,
            assign: CrossAssign::B,
            cue: false,
        },
    );
    mixer
}

/// A crossfader slammed from one end to the other does not step the gain inside a block.
///
/// The bar is the promise the plan made before the code: a one-sample step no larger than 0.03 of the signal
/// peak. Without the ramp this reads a full-scale step — the whole signal appears in one sample.
#[test]
fn the_gate_does_not_click() {
    let mut mixer = mixer_on_b();
    mixer.set_curve(CrossfaderCurve::Sharp);
    mixer.set_crossfader(0.0);
    // Steady state: the channel is shut, so nothing is heard and nothing steps.
    let _ = mixed(&mut mixer, 4);

    // Slam it open, then shut it again: two hard moves. The master limiter looks ahead, so the sound of a
    // move arrives a block later — the window covers both moves and the blocks that carry them.
    mixer.set_crossfader(1.0);
    let mut heard = mixed(&mut mixer, 6);
    mixer.set_crossfader(0.0);
    heard.extend(mixed(&mut mixer, 6));

    let peak = heard
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    assert!(peak > 0.4, "the channel never opened: peak {peak:.3}");
    let step = largest_step(&heard);
    assert!(
        step <= peak * 0.03,
        "the fader stepped {step:.4} inside a block against a peak of {peak:.4}: that is a click"
    );
}

/// And the ramp does not cost the law its accuracy: a settled fader is exactly where the law says.
#[test]
fn a_settled_fader_is_exactly_where_the_law_says() {
    let mut mixer = mixer_on_b();
    mixer.set_curve(CrossfaderCurve::Linear);
    mixer.set_crossfader(0.5);
    let heard = mixed(&mut mixer, 12);
    // The last block is settled: its peak is the sine's own peak times the law's gain at one half.
    let tail = &heard[heard.len() - BLOCK..];
    let peak = tail
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    assert!(
        (peak - 0.25).abs() < 0.01,
        "a settled fader at half travel gave a peak of {peak:.4} instead of 0,25"
    );
}

/// Control: the same signal through the same mixer with the fader held still must be continuous.
///
/// Without this the witness above could be measuring the block handling rather than the fader: a step at every
/// block boundary would look exactly like a fader that clicks.
#[test]
fn a_still_fader_makes_no_step_at_the_block_boundary() {
    let mut mixer = mixer_on_b();
    mixer.set_curve(CrossfaderCurve::Linear);
    mixer.set_crossfader(0.5);
    let heard = mixed(&mut mixer, 12);
    let tail = &heard[BLOCK..];
    let peak = tail
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    let step = largest_step(tail);
    assert!(
        step <= peak * 0.05,
        "with the fader held still the signal stepped {step:.4} against a peak of {peak:.4}"
    );
}
