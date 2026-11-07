use dj_dsp::{ChannelEq, EqGains, RANGE_DB};

const RATE: f64 = 48_000.0;

#[test]
fn flat_eq_is_bypassed_and_unity() {
    let eq = ChannelEq::new(RATE);
    assert!(eq.gains().is_flat());
    for freq in [100.0, 1_000.0, 10_000.0] {
        let db = eq.magnitude_db(freq);
        assert!(db.abs() < 1e-4, "flat EQ at {freq} Hz gave {db} dB");
    }
}

#[test]
fn channel_eq_boost_and_cut() {
    let mut eq = ChannelEq::new(RATE);
    eq.set_gains(EqGains {
        low_db: 12.0,
        mid_db: 0.0,
        high_db: 0.0,
    });
    let low_resp = eq.magnitude_db(60.0);
    assert!(
        low_resp > 8.0,
        "low shelf 12dB boost at 60Hz gave {low_resp}dB"
    );

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: -12.0,
    });
    let high_resp = eq.magnitude_db(10_000.0);
    assert!(
        high_resp < -8.0,
        "high shelf -12dB cut at 10kHz gave {high_resp}dB"
    );
}

#[test]
fn eq_gains_clamping() {
    let gains = EqGains {
        low_db: 99.0,
        mid_db: -99.0,
        high_db: 0.0,
    }
    .clamped();
    assert_eq!(gains.low_db, RANGE_DB);
    assert_eq!(gains.mid_db, -RANGE_DB);
    assert_eq!(gains.high_db, 0.0);
}

/// Steady-state gain of a sine pushed through the real filter, in decibels.
///
/// The analytic response and a measured one are two different claims, and the analytic one is the
/// cheap one to get wrong: one sign in the evaluation made the reported curve disagree with the
/// sound by six decibels at the top of the band while every number still looked plausible. The
/// settle window is half a second, so the transient of the section is long gone.
fn measured_db(eq: &mut ChannelEq, frequency: f64) -> f64 {
    const FRAMES: usize = 48_000;
    eq.reset();
    let mut buffer = [0.0f32; 2];
    let mut energy = 0.0f64;
    let mut counted = 0u64;
    for frame in 0..FRAMES {
        let value = (std::f64::consts::TAU * frequency * frame as f64 / RATE).sin() as f32;
        buffer[0] = value;
        buffer[1] = value;
        eq.process(&mut buffer);
        if frame >= FRAMES / 2 {
            energy += f64::from(buffer[0]) * f64::from(buffer[0]);
            counted += 1;
        }
    }
    let rms = (energy / counted as f64).sqrt();
    // The sine's own RMS is 1/sqrt(2): the gain is the RMS ratio, so scale it back to amplitude.
    20.0 * (rms * std::f64::consts::SQRT_2).log10()
}

#[test]
fn the_reported_response_matches_a_measured_sine() {
    let mut eq = ChannelEq::new(RATE);
    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: -12.0,
    });
    for frequency in [1_000.0, 4_000.0, 8_000.0, 10_000.0, 16_000.0] {
        let reported = eq.magnitude_db(frequency);
        let measured = measured_db(&mut eq, frequency);
        assert!(
            (reported - measured).abs() < 0.2,
            "at {frequency} Hz the curve says {reported:.2} dB but the filter does {measured:.2} dB"
        );
    }
}

#[test]
fn every_band_works_inside_its_own_range() {
    let mut eq = ChannelEq::new(RATE);

    eq.set_gains(EqGains {
        low_db: 12.0,
        mid_db: 0.0,
        high_db: 0.0,
    });
    assert!(eq.magnitude_db(60.0) > 8.0, "low shelf did not lift 60 Hz");
    assert!(
        eq.magnitude_db(12_000.0).abs() < 0.3,
        "low shelf moved 12 kHz"
    );

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: 12.0,
        high_db: 0.0,
    });
    assert!(
        eq.magnitude_db(1_000.0) > 8.0,
        "mid peak did not lift 1 kHz"
    );
    assert!(eq.magnitude_db(60.0).abs() < 0.3, "mid peak moved 60 Hz");

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: 12.0,
    });
    assert!(
        eq.magnitude_db(12_000.0) > 8.0,
        "high shelf did not lift 12 kHz"
    );
    assert!(eq.magnitude_db(60.0).abs() < 0.3, "high shelf moved 60 Hz");
}
