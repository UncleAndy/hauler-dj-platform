//! The isolator: a three-way crossover that removes a band instead of shelving it.

use dj_dsp::{Crossover, Isolator, IsolatorBand};

/// Root-mean-square of the left channel over the second half of the block.
fn measured_db(hertz: f64, kill: [f32; 3]) -> f64 {
    let mut isolator = Isolator::new(48_000.0, Crossover::default());
    for (band, weight) in IsolatorBand::ALL.into_iter().zip(kill) {
        isolator.set_killed(band, weight > 0.5);
    }
    // A second of the tone, so the crossover is at its steady state before the measurement starts.
    let frames = 48_000;
    let mut block = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let value = (std::f64::consts::TAU * hertz * frame as f64 / 48_000.0).sin() as f32 * 0.5;
        block.push(value);
        block.push(value);
    }
    isolator.process(&mut block);
    let half = frames / 2;
    let mut sum = 0.0f64;
    for frame in half..frames {
        let value = f64::from(block[frame * 2]);
        sum += value * value;
    }
    let rms = (sum / (frames - half) as f64).sqrt();
    // The tone is a sine of amplitude 0.5: its RMS is 0.5/sqrt(2).
    20.0 * (rms / (0.5 / std::f64::consts::SQRT_2)).log10()
}

#[test]
fn killing_the_low_band_removes_it_and_leaves_the_neighbours() {
    let low = measured_db(40.0, [1.0, 0.0, 0.0]);
    let above = measured_db(1_000.0, [1.0, 0.0, 0.0]);
    assert!(
        low <= -50.0,
        "40 Hz should be gone, measured {low:.1} dB — a shelf only reaches about -30 dB there"
    );
    assert!(
        above.abs() < 0.5,
        "1 kHz sits above the low crossover and must not move, measured {above:.2} dB"
    );
}

#[test]
fn killing_the_high_band_removes_it_and_leaves_the_neighbours() {
    let high = measured_db(15_000.0, [0.0, 0.0, 1.0]);
    let below = measured_db(300.0, [0.0, 0.0, 1.0]);
    assert!(
        high <= -40.0,
        "15 kHz should be gone, measured {high:.1} dB"
    );
    assert!(
        below.abs() < 0.5,
        "300 Hz sits below the high crossover and must not move, measured {below:.2} dB"
    );
}

#[test]
fn killing_the_mid_band_removes_it_and_leaves_both_neighbours() {
    let mid = measured_db(1_000.0, [0.0, 1.0, 0.0]);
    let low = measured_db(100.0, [0.0, 1.0, 0.0]);
    let high = measured_db(12_000.0, [0.0, 1.0, 0.0]);
    assert!(mid <= -40.0, "1 kHz should be gone, measured {mid:.1} dB");
    assert!(
        low.abs() < 1.0,
        "100 Hz is below the mid band and must stay, measured {low:.2} dB"
    );
    assert!(
        high.abs() < 1.0,
        "12 kHz is above the mid band and must stay, measured {high:.2} dB"
    );
}

#[test]
fn an_untouched_isolator_is_a_bypass() {
    // With nothing killed the isolator must not colour the signal at all: it sits in the path of every
    // deck, and a filter that is "almost flat" is still a filter.
    let mut isolator = Isolator::new(48_000.0, Crossover::default());
    let mut block = Vec::new();
    for frame in 0..1024 {
        let value = (frame as f32 * 0.1).sin() * 0.4;
        block.push(value);
        block.push(value);
    }
    let before = block.clone();
    isolator.process(&mut block);
    assert_eq!(block, before, "an untouched isolator changed the signal");
}
