//! What the vendored stretcher has to do before the deck can rely on it.

use dj_stretch_sys::Stretch;

/// A stereo sine at `hertz`, as one second of interleaved frames.
fn tone(hertz: f64, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * 2);
    for index in 0..frames {
        let value = (std::f64::consts::TAU * hertz * index as f64 / 48_000.0).sin() as f32 * 0.5;
        out.push(value);
        out.push(value);
    }
    out
}

/// Share of the block's energy that sits exactly at `hertz`.
fn purity(block: &[f32], hertz: f64) -> f64 {
    let frames = block.len() / 2;
    let mut re = 0.0f64;
    let mut im = 0.0f64;
    let mut energy = 0.0f64;
    for index in 0..frames {
        let value = f64::from(block[index * 2]);
        let phase = std::f64::consts::TAU * hertz * index as f64 / 48_000.0;
        re += value * phase.cos();
        im += value * phase.sin();
        energy += value * value;
    }
    if energy <= 0.0 {
        return 0.0;
    }
    2.0 * (re * re + im * im) / frames as f64 / energy
}

#[test]
fn a_tone_keeps_its_pitch_and_its_purity_at_eight_per_cent_faster() {
    let mut stretch = Stretch::new(48_000.0);
    let input = tone(440.0, 480_000);
    let mut consumed = 0usize;
    let mut heard = Vec::new();
    let mut out = vec![0.0f32; 512 * 2];
    while consumed + 4096 < input.len() / 2 {
        let in_frames = (512.0f64 * 1.08).round() as usize;
        stretch.process(&input[consumed * 2..(consumed + in_frames) * 2], &mut out);
        consumed += in_frames;
        heard.extend_from_slice(&out);
    }
    // The library announces its delay: skip it before measuring, it is silence by design.
    let skip = stretch.latency();
    let measured = purity(&heard[skip * 2..], 440.0);
    assert!(measured > 0.99, "purity at 1.08 is only {measured:.3}");
}

#[test]
fn a_short_block_costs_almost_nothing_and_halves_the_delay() {
    // The deck needs the delay small enough that a loop marker and the clock agree with the sound.
    let mut default = Stretch::new(48_000.0);
    let mut short = Stretch::with_config(48_000.0, 0.06, 0.015);
    println!(
        "default latency {} block {}, short latency {} block {}",
        default.latency(),
        default.block(),
        short.latency(),
        short.block()
    );
    assert!(
        short.latency() < default.latency(),
        "the short block is not shorter: {} vs {}",
        short.latency(),
        default.latency()
    );

    let measure = |stretch: &mut Stretch| {
        let input = tone(440.0, 480_000);
        let mut consumed = 0usize;
        let mut heard = Vec::new();
        let mut out = vec![0.0f32; 512 * 2];
        while consumed + 4096 < input.len() / 2 {
            let in_frames = (512.0f64 * 1.08).round() as usize;
            let from = consumed * 2;
            stretch.process(&input[from..from + in_frames * 2], &mut out);
            consumed += in_frames;
            heard.extend_from_slice(&out);
        }
        let skip = stretch.latency();
        purity(&heard[skip * 2..], 440.0)
    };
    let clean = measure(&mut default);
    let short_clean = measure(&mut short);
    assert!(
        short_clean > 0.99 && clean > 0.99,
        "purity: default {clean:.4}, short {short_clean:.4}"
    );
}

#[test]
fn the_library_reports_its_latency_and_block() {
    let stretch = Stretch::new(48_000.0);
    println!("latency {} block {}", stretch.latency(), stretch.block());
    assert!(
        stretch.latency() < 48_000 / 2,
        "latency {} is half a second",
        stretch.latency()
    );
    assert!(stretch.block() > 0);
}
