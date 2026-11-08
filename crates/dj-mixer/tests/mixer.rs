//! Mixer behaviour: crossfader law, routing, limiter, metering, NaN guards (A04).

use dj_mixer::{ChannelSettings, CrossAssign, CrossfaderCurve, Mixer};

fn settings(assign: CrossAssign, fader: f32) -> ChannelSettings {
    ChannelSettings {
        trim_db: 0.0,
        fader,
        assign,
        cue: false,
    }
}

fn dc_block(frames: usize, value: f32) -> Vec<f32> {
    vec![value; frames * 2]
}

fn mix_two(a: f32, b: f32, crossfader: f32, curve: CrossfaderCurve) -> (Vec<f32>, Vec<f32>) {
    let mut mixer = Mixer::new(2);
    mixer.prepare(48_000, 256);
    mixer.set_curve(curve);
    mixer.set_crossfader(crossfader);
    mixer.set_channel(0, settings(CrossAssign::A, 1.0));
    mixer.set_channel(1, settings(CrossAssign::B, 1.0));
    mixer.begin_block(256);
    let block_a = dc_block(256, a);
    let block_b = dc_block(256, b);
    mixer.mix_channel(0, &block_a);
    mixer.mix_channel(1, &block_b);
    mixer.finish_block();
    (mixer.master().to_vec(), mixer.cue().to_vec())
}

#[test]
fn the_constant_power_crossfader_isolates_the_extremes() {
    let mut mixer = Mixer::new(2);
    mixer.prepare(48_000, 64);
    mixer.set_curve(CrossfaderCurve::ConstantPower);

    mixer.set_crossfader(0.0);
    assert!((mixer.crossfader_gain(CrossAssign::A) - 1.0).abs() < 1e-6);
    assert!(mixer.crossfader_gain(CrossAssign::B).abs() < 1e-6);

    mixer.set_crossfader(1.0);
    assert!(mixer.crossfader_gain(CrossAssign::A).abs() < 1e-6);
    assert!((mixer.crossfader_gain(CrossAssign::B) - 1.0).abs() < 1e-6);

    mixer.set_crossfader(0.5);
    let half = mixer.crossfader_gain(CrossAssign::A);
    assert!(
        (half - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4,
        "constant power must sit at -3 dB in the middle, got {half}"
    );
    assert!((mixer.crossfader_gain(CrossAssign::Thru) - 1.0).abs() < 1e-6);
}

#[test]
fn the_linear_curve_is_linear() {
    let mut mixer = Mixer::new(2);
    mixer.prepare(48_000, 64);
    mixer.set_curve(CrossfaderCurve::Linear);
    mixer.set_crossfader(0.25);
    assert!((mixer.crossfader_gain(CrossAssign::A) - 0.75).abs() < 1e-6);
    assert!((mixer.crossfader_gain(CrossAssign::B) - 0.25).abs() < 1e-6);
}

#[test]
fn every_crossfader_law_has_the_shape_it_promises() {
    let mut mixer = Mixer::new(2);
    mixer.prepare(48_000, 64);

    // The extremes are the same for every law: one side open, the other shut. A law that leaks at
    // the ends is a law that makes silence impossible.
    for law in CrossfaderCurve::ALL {
        mixer.set_curve(law);
        mixer.set_crossfader(0.0);
        assert!(
            (mixer.crossfader_gain(CrossAssign::A) - 1.0).abs() < 1e-6,
            "{law:?}"
        );
        assert!(
            mixer.crossfader_gain(CrossAssign::B).abs() < 1e-6,
            "{law:?}"
        );
        mixer.set_crossfader(1.0);
        assert!(
            mixer.crossfader_gain(CrossAssign::A).abs() < 1e-6,
            "{law:?}"
        );
        assert!(
            (mixer.crossfader_gain(CrossAssign::B) - 1.0).abs() < 1e-6,
            "{law:?}"
        );
    }

    // In the middle: linear and smooth sit at half each (amplitude), constant power at −3 dB, and
    // the cut law keeps both channels wide open — that is what makes a scratch audible.
    let mut centre = |law| {
        mixer.set_curve(law);
        mixer.set_crossfader(0.5);
        mixer.crossfader_gain(CrossAssign::A)
    };
    assert!((centre(CrossfaderCurve::Linear) - 0.5).abs() < 1e-6);
    assert!((centre(CrossfaderCurve::Smooth) - 0.5).abs() < 1e-6);
    assert!(
        (centre(CrossfaderCurve::ConstantPower) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4
    );
    assert!((centre(CrossfaderCurve::Sharp) - 1.0).abs() < 1e-6);

    // Smooth is an S: it holds level near its own end and falls away quickly past the middle, so the
    // audible move happens over the central half of the travel.
    mixer.set_curve(CrossfaderCurve::Smooth);
    mixer.set_crossfader(0.1);
    let near_its_end = mixer.crossfader_gain(CrossAssign::A);
    assert!(
        near_its_end > 0.95,
        "smooth stays high near its own end: {near_its_end}"
    );
    mixer.set_crossfader(0.8);
    let past_the_middle = mixer.crossfader_gain(CrossAssign::A);
    assert!(
        past_the_middle < 0.2,
        "and is nearly gone past it: {past_the_middle}"
    );
    // Both sides quieter than a linear law on their way out — the dip the S-shape is for.
    mixer.set_crossfader(0.7);
    let linear_side = 0.3f32;
    assert!(mixer.crossfader_gain(CrossAssign::A) < linear_side);

    // The cut law is flat for the first three quarters of its own travel and then falls away: A at
    // 0.85 of the crossfader still holds 0.6 of its level, at 0.7 it is untouched.
    mixer.set_curve(CrossfaderCurve::Sharp);
    mixer.set_crossfader(0.85);
    assert!((mixer.crossfader_gain(CrossAssign::A) - 0.6).abs() < 1e-6);
    mixer.set_crossfader(0.7);
    assert!((mixer.crossfader_gain(CrossAssign::A) - 1.0).abs() < 1e-6);
    assert!((mixer.crossfader_gain(CrossAssign::B) - 1.0).abs() < 1e-6);
}

#[test]
fn the_law_names_round_trip() {
    for law in CrossfaderCurve::ALL {
        assert_eq!(CrossfaderCurve::parse(law.as_str()), Some(law));
    }
    assert_eq!(
        CrossfaderCurve::parse("Sharp"),
        Some(CrossfaderCurve::Sharp)
    );
    assert_eq!(
        CrossfaderCurve::parse("  smooth "),
        Some(CrossfaderCurve::Smooth)
    );
    assert_eq!(CrossfaderCurve::parse("cubic"), None);
    assert_eq!(CrossfaderCurve::default(), CrossfaderCurve::ConstantPower);
}

#[test]
fn routing_puts_each_channel_only_where_it_belongs() {
    // Crossfader fully to B: channel A must not reach the master, but its cue stays audible.
    let mut mixer = Mixer::new(2);
    mixer.prepare(48_000, 256);
    mixer.set_crossfader(1.0);
    mixer.set_channel(
        0,
        ChannelSettings {
            cue: true,
            ..settings(CrossAssign::A, 1.0)
        },
    );
    mixer.set_channel(1, settings(CrossAssign::B, 1.0));

    let block_a = dc_block(256, 0.5);
    let silent_b = dc_block(256, 0.0);
    // Both buses carry the limiter's lookahead, so it takes a few blocks before the material is out.
    for _ in 0..3 {
        mixer.begin_block(256);
        mixer.mix_channel(0, &block_a);
        mixer.mix_channel(1, &silent_b);
        mixer.finish_block();
    }

    assert!(
        mixer.master().iter().all(|sample| sample.abs() < 1e-6),
        "a channel closed by the crossfader must not leak into master"
    );
    assert!(
        mixer.cue().iter().all(|sample| (sample - 0.5).abs() < 1e-6),
        "the cued channel must appear in the cue bus pre-crossfader"
    );
}

#[test]
fn non_finite_parameters_keep_the_previous_value() {
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 64);
    mixer.set_crossfader(0.25);
    mixer.set_crossfader(f32::NAN);
    assert!((mixer.crossfader() - 0.25).abs() < 1e-6);

    mixer.set_channel(0, settings(CrossAssign::A, 0.5));
    mixer.set_channel(
        0,
        ChannelSettings {
            trim_db: f32::INFINITY,
            fader: f32::NAN,
            ..settings(CrossAssign::B, 1.0)
        },
    );
    let channel = mixer.channel(0).copied().unwrap_or_default();
    assert!(
        (channel.fader - 0.5).abs() < 1e-6,
        "NaN fader must be ignored"
    );
    assert!(
        (channel.trim_db - 0.0).abs() < 1e-6,
        "inf trim must be ignored"
    );
}

#[test]
fn non_finite_samples_become_silence_and_are_counted() {
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 32);
    mixer.set_channel(0, settings(CrossAssign::Thru, 1.0));
    mixer.begin_block(32);
    let mut block = dc_block(32, 0.1);
    block[0] = f32::NAN;
    block[3] = f32::INFINITY;
    mixer.mix_channel(0, &block);
    mixer.finish_block();

    assert_eq!(mixer.meters().non_finite_samples, 2);
    assert!(mixer.master().iter().all(|sample| sample.is_finite()));
    assert_eq!(mixer.master()[0], 0.0);
}

#[test]
fn the_limiter_keeps_the_master_below_full_scale() {
    // Two full-scale channels on opposite sides at the centre sum to about 1.41.
    let (master, _) = mix_two(1.0, 1.0, 0.5, CrossfaderCurve::ConstantPower);
    let peak = master
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    assert!(
        peak <= 1.0,
        "the master must never exceed full scale, peak was {peak}"
    );
    assert!(peak > 0.5, "the limiter must not silence the mix");
}

#[test]
fn clipping_is_reported_when_the_input_is_over_full_scale() {
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 64);
    mixer.set_channel(0, settings(CrossAssign::Thru, 1.0));
    mixer.begin_block(64);
    let block = dc_block(64, 1.5);
    mixer.mix_channel(0, &block);
    mixer.finish_block();

    assert!(mixer.meters().clipped(), "overdriven input must be flagged");
    assert!(mixer.meters().clipped_samples > 0);
    assert!(mixer.meters().limiter_gain < 1.0);
    let peak = mixer
        .master()
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    assert!(peak <= 1.0);
}

#[test]
fn the_master_limiter_looks_ahead_of_the_peak() {
    // A limiter without lookahead can only drop the gain on the sample that needs it: everything
    // before the peak leaves at full level and the waveform gets an edge the ear hears as the limiter
    // working. With a lookahead the gain is already down when the peak arrives.
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 512);
    mixer.set_channel(0, settings(CrossAssign::Thru, 1.0));

    let latency = mixer.latency_frames() as usize;
    assert!(
        latency > 0,
        "a lookahead limiter adds its attack as latency, and the mixer must say so"
    );

    // Half scale for 200 frames, then a step to 1.5 for 40 frames, then silence.
    let mut block = vec![0.0f32; 512 * 2];
    for frame in 0..200 {
        block[frame * 2] = 0.5;
        block[frame * 2 + 1] = 0.5;
    }
    for frame in 200..240 {
        block[frame * 2] = 1.5;
        block[frame * 2 + 1] = 1.5;
    }

    mixer.begin_block(512);
    mixer.mix_channel(0, &block);
    mixer.finish_block();
    let master = mixer.master();

    let peak = master
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    assert!(peak <= 1.0, "the limiter let {peak} through");

    // The output is the input delayed by the lookahead, so the frames just before the step sit at
    // `latency + 186`. With a lookahead they are already turned down; without one they stay at 0.5.
    let before = (latency + 186..latency + 200)
        .map(|frame| master[frame * 2].abs())
        .fold(0.0f32, f32::max);
    assert!(
        before < 0.45,
        "the gain must be down before the peak arrives, got {before} just ahead of it"
    );
}

#[test]
fn meters_report_peak_and_rms_of_the_channel() {
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 128);
    mixer.set_channel(0, settings(CrossAssign::Thru, 0.5));
    mixer.begin_block(128);
    let block = dc_block(128, 0.6);
    mixer.mix_channel(0, &block);
    mixer.finish_block();

    let channel = mixer.meters().channels[0];
    assert!((channel.peak - 0.3).abs() < 1e-6, "peak {}", channel.peak);
    assert!((channel.rms - 0.3).abs() < 1e-6, "rms {}", channel.rms);
    // The master comes out of the limiter's delay line, so its first block is still filling: keep
    // feeding the same material until the delay is past.
    let mut seen = 0.0f32;
    for _ in 0..4 {
        mixer.begin_block(128);
        mixer.mix_channel(0, &block);
        mixer.finish_block();
        seen = seen.max(mixer.meters().master.peak);
    }
    assert!(
        (seen - 0.3).abs() < 1e-3,
        "the master never reached 0.3: {seen}"
    );
}

#[test]
fn the_cue_bus_is_delayed_like_the_master() {
    // The limiter's lookahead delays the master. If the cue bus stayed early, a DJ matching beats by
    // ear would hear the two buses a few milliseconds apart — the one thing headphones are for.
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 512);
    mixer.set_channel(
        0,
        ChannelSettings {
            trim_db: 0.0,
            fader: 1.0,
            assign: CrossAssign::Thru,
            cue: true,
        },
    );

    // One click, then silence, over a block long enough for both buses to come out.
    let mut block = vec![0.0f32; 512 * 2];
    block[0] = 0.5;
    block[1] = 0.5;
    mixer.begin_block(512);
    mixer.mix_channel(0, &block);
    mixer.finish_block();

    let master_at = mixer.master().iter().position(|sample| sample.abs() > 1e-6);
    let cue_at = mixer.cue().iter().position(|sample| sample.abs() > 1e-6);
    assert_eq!(
        master_at, cue_at,
        "the cue bus must arrive with the master, not before it"
    );
    assert!(
        master_at.is_some_and(|index| index >= mixer.latency_frames() as usize * 2),
        "both buses carry the lookahead delay"
    );
}

#[test]
fn a_meter_holds_its_peak_and_decays_after_the_signal_stops() {
    // A meter that flickers at the block rate is a counter, not a meter. The bar jumps to the peak and
    // falls back over about half a second, so a block of silence still carries a value.
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 128);
    mixer.set_channel(0, settings(CrossAssign::Thru, 1.0));

    mixer.begin_block(128);
    let loud = dc_block(128, 0.5);
    mixer.mix_channel(0, &loud);
    mixer.finish_block();
    let at_peak = mixer.meters().channels[0];
    assert!((at_peak.peak - 0.5).abs() < 1e-6, "peak {}", at_peak.peak);
    assert!(
        (at_peak.held - 0.5).abs() < 1e-3,
        "the meter did not rise to the peak: held {}",
        at_peak.held
    );

    // A quarter of a second of silence: 94 blocks of 128 frames.
    for _ in 0..94 {
        mixer.begin_block(128);
        mixer.mix_channel(0, &dc_block(128, 0.0));
        mixer.finish_block();
    }
    let after = mixer.meters().channels[0];
    assert!(
        after.peak < 1e-6,
        "the block peak must stay per block: {}",
        after.peak
    );
    assert!(
        (0.15..0.35).contains(&after.held),
        "half a second of decay should be near half the peak, got {}",
        after.held
    );
}

#[test]
fn trim_is_a_decibel_gain() {
    let mut mixer = Mixer::new(1);
    mixer.prepare(48_000, 64);
    mixer.set_channel(
        0,
        ChannelSettings {
            trim_db: -6.0206,
            ..settings(CrossAssign::Thru, 1.0)
        },
    );
    mixer.begin_block(64);
    let block = dc_block(64, 0.5);
    mixer.mix_channel(0, &block);
    mixer.finish_block();
    let peak = mixer.meters().channels[0].peak;
    assert!(
        (peak - 0.25).abs() < 1e-3,
        "-6 dB must halve the level, got {peak}"
    );
}

#[test]
fn the_mixer_declares_the_limiters_lookahead_as_its_latency() {
    // The lookahead is what lets the gain come down before a peak, and it is a delay: the graph has to
    // know about it, so the mixer states it instead of pretending to be free.
    let mut mixer = Mixer::new(2);
    mixer.prepare(48_000, 256);
    let latency = mixer.latency_frames();
    assert_eq!(
        latency,
        (48_000.0 * f64::from(dj_signalsmith_sys::LIMITER_ATTACK_MS) / 1000.0).ceil() as u32,
        "the latency is the limiter's attack"
    );
    assert!(
        latency > 0 && latency < 480,
        "a booth limiter cannot add 10 ms: {latency}"
    );
    assert_eq!(mixer.tail_frames(), 0);
}
