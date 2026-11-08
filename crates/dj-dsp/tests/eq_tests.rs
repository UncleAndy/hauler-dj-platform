use dj_dsp::{BOOST_DB, CUT_DB, ChannelEq, EqBand, EqGains, EqTable, FLOOR_DB, STEP_DB, STEPS};

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
fn the_boost_is_capped_at_the_dj_limit_and_the_cut_reaches_the_floor() {
    // Asymmetric on purpose: a lift raises the loudness of the whole channel (+12 dB of low shelf
    // measured +11 dB RMS on bass-heavy material), a cut only takes one band away.
    let mut eq = ChannelEq::new(RATE);
    eq.set_gains(EqGains {
        low_db: 12.0,
        mid_db: 0.0,
        high_db: 0.0,
    });
    assert_eq!(eq.gains().low_db, BOOST_DB, "the boost cap did not hold");
    let low_resp = eq.magnitude_db(60.0);
    assert!(low_resp > 4.0, "low shelf +6 at 60 Hz gave {low_resp} dB");

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: -12.0,
    });
    let high_resp = eq.magnitude_db(10_000.0);
    assert!(
        high_resp < -8.0,
        "high shelf -12 at 10 kHz gave {high_resp} dB"
    );
}

#[test]
fn eq_gains_clamping() {
    let gains = EqGains {
        low_db: 99.0,
        mid_db: -999.0,
        high_db: 0.0,
    }
    .clamped();
    assert_eq!(gains.low_db, BOOST_DB);
    assert_eq!(gains.mid_db, FLOOR_DB);
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
        low_db: BOOST_DB,
        mid_db: 0.0,
        high_db: 0.0,
    });
    assert!(eq.magnitude_db(60.0) > 4.0, "low shelf did not lift 60 Hz");
    assert!(
        eq.magnitude_db(12_000.0).abs() < 0.3,
        "low shelf moved 12 kHz"
    );

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: BOOST_DB,
        high_db: 0.0,
    });
    assert!(
        eq.magnitude_db(1_000.0) > 4.0,
        "mid peak did not lift 1 kHz"
    );
    assert!(eq.magnitude_db(60.0).abs() < 0.3, "mid peak moved 60 Hz");

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: BOOST_DB,
    });
    assert!(
        eq.magnitude_db(12_000.0) > 4.0,
        "high shelf did not lift 12 kHz"
    );
    assert!(eq.magnitude_db(60.0).abs() < 0.3, "high shelf moved 60 Hz");
}

#[test]
fn the_table_is_as_long_as_the_range_says() {
    let expected = ((BOOST_DB - FLOOR_DB) / STEP_DB) as usize + 1;
    assert_eq!(STEPS, expected, "the table no longer covers the whole knob");
    assert_eq!(
        EqTable::gain_of(EqTable::UNITY_STEP),
        0.0,
        "unity moved off its row"
    );
    assert_eq!(EqTable::gain_of(0), FLOOR_DB);
    assert_eq!(EqTable::gain_of(STEPS - 1), BOOST_DB);
    assert_eq!(CUT_DB, 26.0, "the ordinary cut moved");
}

#[test]
fn a_knob_lands_on_the_grid_and_says_where() {
    let mut eq = ChannelEq::new(RATE);
    eq.set_gains(EqGains {
        low_db: 6.07,
        mid_db: -0.04,
        high_db: -25.9,
    });
    let applied = eq.gains();
    // The published value is the applied one: a knob cannot show a number the filter is not using.
    assert!(
        (applied.low_db - 6.07).abs() <= STEP_DB / 2.0,
        "{}",
        applied.low_db
    );
    assert_eq!(applied.mid_db, 0.0, "a band inside half a step is unity");
    assert!(
        (applied.high_db + 25.9).abs() <= STEP_DB / 2.0,
        "the nearest step to -25.9 is not -26.0: {}",
        applied.high_db
    );
    let mut overshoot = EqGains::FLAT;
    overshoot.set_band(EqBand::High, -999.0);
    eq.set_gains(overshoot);
    assert_eq!(
        eq.gains().high_db,
        FLOOR_DB,
        "past the end the knob stops at the floor"
    );
    assert_eq!(applied.band(EqBand::Low), applied.low_db);
}

#[test]
fn a_knob_move_is_the_same_filter_the_curve_describes() {
    // The table is what the callback copies from: if a row were built for the wrong gain, the sound
    // would disagree with the published number, and only a measurement would notice.
    let mut eq = ChannelEq::new(RATE);
    for gain in [-24.0f32, -12.0, -3.0, 3.0, BOOST_DB] {
        let mut gains = EqGains::FLAT;
        gains.set_band(EqBand::High, gain);
        eq.set_gains(gains);
        let applied = eq.gains().high_db;
        let measured = measured_db(&mut eq, 16_000.0);
        let reported = eq.magnitude_db(16_000.0);
        assert!(
            (measured - reported).abs() < 0.2,
            "asked {gain} dB, applied {applied}: curve {reported:.2}, filter {measured:.2}"
        );
        assert!(
            (measured - f64::from(applied)).abs() < 1.0,
            "applied {applied} dB but the filter does {measured:.2} dB at 16 kHz"
        );
    }
}

#[test]
fn the_floor_removes_the_band_and_leaves_its_neighbours() {
    // The floor used to be a −120 dB shelf: it silenced its own band but took −32 dB out of 1 kHz —
    // a gutted channel, not a killed band. The floor is now a kill through a three-way crossover, so
    // the band is gone and the neighbours do not move.
    let mut eq = ChannelEq::new(RATE);
    let mut gains = EqGains::FLAT;
    gains.set_band(EqBand::Low, FLOOR_DB);
    eq.set_gains(gains);
    assert_eq!(eq.gains().low_db, FLOOR_DB);
    assert!(
        eq.isolator().killed()[0],
        "the floor must reach the isolator, not the shelf"
    );

    let low = measured_db(&mut eq, 40.0);
    assert!(low < -50.0, "the floor still passes {low:.1} dB at 40 Hz");
    let mid = measured_db(&mut eq, 1_000.0);
    assert!(
        mid.abs() < 0.5,
        "killing the low band moved 1 kHz by {mid:.2} dB"
    );
}

#[test]
fn the_high_floor_removes_the_top_and_leaves_the_bass() {
    let mut eq = ChannelEq::new(RATE);
    let mut gains = EqGains::FLAT;
    gains.set_band(EqBand::High, FLOOR_DB);
    eq.set_gains(gains);
    assert!(
        eq.isolator().killed()[2],
        "the high floor must kill the top"
    );

    let high = measured_db(&mut eq, 15_000.0);
    assert!(
        high < -40.0,
        "the floor still passes {high:.1} dB at 15 kHz"
    );
    let bass = measured_db(&mut eq, 100.0);
    assert!(
        bass.abs() < 0.5,
        "killing the top moved 100 Hz by {bass:.2} dB"
    );
}

#[test]
fn an_ordinary_cut_is_not_a_kill() {
    // −26 dB is a deep cut and stays a shelf: the isolator is what the floor means, not what a cut means.
    let mut eq = ChannelEq::new(RATE);
    let mut gains = EqGains::FLAT;
    gains.set_band(EqBand::Low, -CUT_DB);
    eq.set_gains(gains);
    assert!(
        !eq.isolator().killed()[0],
        "an ordinary cut must not reach the crossover"
    );
    let low = measured_db(&mut eq, 40.0);
    assert!(
        low > -40.0,
        "an ordinary cut should be a shelf, measured {low:.1} dB at 40 Hz"
    );
}
