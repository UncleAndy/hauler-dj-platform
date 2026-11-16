//! What the deck relies on from the interpolation shim, pinned so it cannot drift quietly.
//!
//! The deck reads a window of material and asks the shim for the frame each output belongs to. Three
//! properties matter, and each one is a test here: a whole-frame phase is exact, a half-frame phase is
//! within the kernel's own passband ripple, and a negative step is not something the deck uses — walking
//! backwards is done by turning the window round, so the kernel keeps looking the way it was designed to.

#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use dj_signalsmith_sys::{Interpolation, Resampler};

/// A window of material where each sample is its own frame number.
fn ramp(frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames * 2];
    for frame in 0..frames {
        let value = frame as f32;
        out[frame * 2] = value;
        out[frame * 2 + 1] = value;
    }
    out
}

fn read(input: &[f32], phase: f64, step: f64, frames: usize) -> (Vec<f64>, f64) {
    let mut resampler = Resampler::new(Interpolation::KaiserSinc16);
    let mut out = vec![0.0f32; frames * 2];
    let reached = resampler.process(input, &mut out, 0, phase, step);
    let heard = (0..frames).map(|index| f64::from(out[index * 2])).collect();
    (heard, reached)
}

/// A whole-frame phase is a plain tap: the sample that comes out names its own frame, exactly.
#[test]
fn a_whole_phase_reads_the_material_exactly() {
    let material = ramp(2048);
    for length in [144usize, 256, 400] {
        for phase in [8.0f64, 100.0, 20.0] {
            let (heard, reached) = read(&material, phase, 1.0, length);
            assert!(
                (reached - (phase + length as f64)).abs() <= 0.01,
                "length {length} phase {phase}: reached {reached}"
            );
            for (index, value) in heard.iter().enumerate().take(length - 16) {
                let want = phase + index as f64;
                assert!(
                    (value - want).abs() <= 0.01,
                    "length {length} phase {phase}: output {index} read {value:.3}, expected {want:.3}"
                );
            }
        }
    }
}

/// The kernel looks back from the frame it is asked about, so the caller has to leave it room: a head
/// with fewer frames behind it than the kernel's latency reads silence, not a guess. The deck's window
/// pads by several times that, which is why this contract is easy to forget.
#[test]
fn a_head_without_room_behind_it_reads_silence() {
    let material = ramp(2048);
    let (heard, reached) = read(&material, 0.0, 1.0, 64);
    assert!(
        (reached - 0.0).abs() <= 0.01,
        "a head with no room behind it reached {reached}"
    );
    assert!(
        heard.iter().all(|value| *value == 0.0),
        "a head with no room behind it wrote material"
    );
}

/// A half-frame phase is interpolated, and the kernel's gain ripples: the level is within a fifth of a
/// percent of the material, which at the deck's own levels is inaudible — but it is why a witness cannot
/// read a frame number out of a level at a fractional phase.
#[test]
fn a_half_phase_reads_within_the_kernels_own_ripple() {
    let material = ramp(2048);
    let (heard, _) = read(&material, 100.0, 0.5, 400);
    let mut worst = 0.0f64;
    for (index, value) in heard.iter().enumerate().take(400) {
        let want = 100.0 + 0.5 * index as f64;
        let error = (value - want).abs();
        worst = worst.max(error / want.max(1.0));
    }
    assert!(
        worst <= 0.002,
        "the interpolated level is off by {:.4} of the material, past the kernel's ripple",
        worst
    );
}

/// A negative step is not exact, and the deck does not use one: walking backwards is done by turning the
/// window round and reading it forwards. If this ever starts passing, the deck's own way still holds —
/// but whoever changes the deck should know the shim has changed under it.
#[test]
fn a_negative_step_is_not_what_the_deck_reads_backwards_with() {
    let material = ramp(2048);
    let (heard, _) = read(&material, 900.0, -1.5, 256);
    let want = 900.0 - 1.5;
    assert!(
        (heard[1] - want).abs() > 0.5,
        "a negative step now reads {:.3} where {want:.3} was asked for: the deck's reversed window \
         is no longer the only exact way backwards",
        heard[1]
    );
}
