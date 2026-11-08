use dj_dsp::{BOOST_DB, CUT_DB, ChannelEq, EqBand, EqGains, FLOOR_DB, STEP_DB};

const RATE: f64 = 48_000.0;

#[test]
fn flat_eq_is_bypassed_and_unity() {
    let eq = ChannelEq::new(RATE);
    assert!(eq.gains().is_flat());
    for freq in [100.0, 1_000.0, 10_000.0] {
        let db = eq.magnitude_db(freq);
        // The split is flat to a hundredth of a decibel: the crossing's own allpass leaves that much
        // behind, and unity is a bypass in the audio path anyway, so nothing is multiplied by it.
        assert!(db.abs() < 0.01, "flat EQ at {freq} Hz gave {db} dB");
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
    assert!(
        eq.magnitude_db(60.0) > 4.0,
        "the low band did not lift 60 Hz"
    );
    assert!(
        eq.magnitude_db(12_000.0).abs() < 0.3,
        "the low band moved 12 kHz"
    );

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: BOOST_DB,
        high_db: 0.0,
    });
    assert!(
        eq.magnitude_db(1_000.0) > 4.0,
        "the middle band did not lift 1 kHz"
    );
    assert!(
        eq.magnitude_db(60.0).abs() < 0.3,
        "the middle band moved 60 Hz"
    );

    eq.set_gains(EqGains {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: BOOST_DB,
    });
    assert!(
        eq.magnitude_db(12_000.0) > 4.0,
        "the high band did not lift 12 kHz"
    );
    assert!(
        eq.magnitude_db(60.0).abs() < 0.3,
        "the high band moved 60 Hz"
    );
}

#[test]
fn the_knob_covers_the_whole_range_on_its_grid() {
    // The equaliser has no table any more — three gains replace it — so the grid is checked where it is
    // made: every end of the range has to land on the grid and stay inside it.
    for requested in [FLOOR_DB, -CUT_DB, 0.0, BOOST_DB] {
        let applied = ChannelEq::on_the_grid(requested);
        assert!(
            (applied - requested).abs() <= STEP_DB / 2.0,
            "{requested} landed on {applied}, which is off the grid"
        );
    }
    assert_eq!(ChannelEq::on_the_grid(FLOOR_DB), FLOOR_DB);
    assert_eq!(ChannelEq::on_the_grid(BOOST_DB), BOOST_DB);
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
    // a gutted channel, not a killed band. The floor now takes its band out of the crossover, so the
    // band is gone and the neighbours do not move.
    let mut eq = ChannelEq::new(RATE);
    let mut gains = EqGains::FLAT;
    gains.set_band(EqBand::Low, FLOOR_DB);
    eq.set_gains(gains);
    assert_eq!(eq.gains().low_db, FLOOR_DB);
    assert_eq!(
        eq.bands().gains()[0],
        0.0,
        "the floor must take the band out, not leave it quiet"
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
    assert_eq!(
        eq.bands().gains()[2],
        0.0,
        "the high floor must take the top out"
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
    // −26 dB is a cut, and a cut leaves a band that is still there: the floor is silence, this is not.
    let mut eq = ChannelEq::new(RATE);
    let mut gains = EqGains::FLAT;
    gains.set_band(EqBand::Low, -CUT_DB);
    eq.set_gains(gains);
    let gain = eq.bands().gains()[0];
    assert!(
        gain > 0.01 && gain < 0.1,
        "an ordinary cut should leave about a twentieth of the band, not {gain}"
    );
    let low = measured_db(&mut eq, 40.0);
    assert!(
        low > -40.0,
        "an ordinary cut should leave the band audible, measured {low:.1} dB at 40 Hz"
    );
}

#[test]
fn a_cut_takes_the_whole_band_down_evenly() {
    // The knob says -26 dB, so the whole band has to be down about that much, not only its centre. With
    // shelves and a narrow peak the middle band lost 26 dB at 1 kHz and 13 dB at 500 Hz and at 2 kHz —
    // which is what "it does not cut" sounds like: the number on the knob and the ear disagree, and the
    // detent then has the missing decibels to make up all at once.
    let probes: [(EqBand, f64, &[f64]); 3] = [
        (EqBand::Low, 40.0, &[40.0, 80.0]),
        (EqBand::Mid, 1_000.0, &[500.0, 1_000.0, 2_000.0]),
        (EqBand::High, 12_000.0, &[8_000.0, 12_000.0]),
    ];
    for (band, centre, inside) in probes {
        let mut eq = ChannelEq::new(RATE);
        let mut gains = EqGains::FLAT;
        gains.set_band(band, -CUT_DB);
        eq.set_gains(gains);
        // The middle of a band follows the knob exactly: that is the difference from a shelf, whose
        // number was true only at one frequency.
        let middle = measured_db(&mut eq, centre);
        assert!(
            (middle + f64::from(CUT_DB)).abs() <= 4.0,
            "{band:?} at its centre {centre:.0} Hz measures {middle:.1} dB, not the {:.0} dB the knob \
             says",
            -CUT_DB
        );
        // Towards the crossings the neighbouring band still carries a few percent, so the depth there is
        // bounded by the crossover and not by the knob — the same on any mixer with a three-way split.
        for hertz in inside.iter().copied() {
            let measured = measured_db(&mut eq, hertz);
            assert!(
                measured <= -18.0,
                "{band:?} at {hertz:.0} Hz is only {measured:.1} dB down, so the band is not really cut"
            );
        }
    }
}

#[test]
fn the_detent_takes_only_what_the_cut_left() {
    // Crossing into the detent must not do the cut's work twice: what it adds is the band's remaining
    // -26 dB. On the narrow middle peak it added 46 dB at 1 kHz, which is the step the owner heard.
    for (band, frequency) in [
        (EqBand::Low, 60.0),
        (EqBand::Mid, 1_000.0),
        (EqBand::High, 12_000.0),
    ] {
        let mut cut = ChannelEq::new(RATE);
        let mut gains = EqGains::FLAT;
        gains.set_band(band, -CUT_DB);
        cut.set_gains(gains);
        let cut_db = measured_db(&mut cut, frequency);

        let mut kill = ChannelEq::new(RATE);
        let mut gains = EqGains::FLAT;
        gains.set_band(band, FLOOR_DB);
        kill.set_gains(gains);
        let kill_db = measured_db(&mut kill, frequency);

        assert!(
            kill_db <= cut_db - 8.0 && kill_db >= cut_db - 34.0,
            "{band:?} at {frequency:.0} Hz goes from {cut_db:.1} to {kill_db:.1} dB: the detent has \
             to take what is left, not a fresh helping"
        );
    }
}

#[test]
fn killing_every_band_leaves_silence() {
    let mut eq = ChannelEq::new(RATE);
    let mut gains = EqGains::FLAT;
    for band in EqBand::ALL {
        gains.set_band(band, FLOOR_DB);
    }
    eq.set_gains(gains);
    let measured = measured_db(&mut eq, 1_000.0);
    assert!(
        measured < -80.0,
        "three kills still pass {measured:.1} dB at 1 kHz"
    );
}

#[test]
fn the_detent_is_deeper_than_the_cut_before_it() {
    // The hand crosses a line into the kill, and crossing it must never make the band louder. On a drum
    // loop the kill used to come back 5.3 dB louder at 250 Hz than the deepest cut: a shelf bleeds
    // upward, a crossover does not, and nothing joined the two.
    // Inside the band only: above the low crossover the middle band carries the signal, and a low knob
    // is not supposed to move it — that is the point of a three-way split.
    for hertz in [40.0, 55.0, 80.0, 110.0] {
        let mut cut = ChannelEq::new(RATE);
        let mut gains = EqGains::FLAT;
        gains.set_band(EqBand::Low, -24.8);
        cut.set_gains(gains);
        let cut_db = measured_db(&mut cut, hertz);

        let mut kill = ChannelEq::new(RATE);
        let mut gains = EqGains::FLAT;
        gains.set_band(EqBand::Low, FLOOR_DB);
        kill.set_gains(gains);
        let kill_db = measured_db(&mut kill, hertz);

        assert!(
            kill_db <= cut_db,
            "at {hertz:.0} Hz the detent gives {kill_db:.1} dB against the cut's {cut_db:.1} dB, so \
             killing the band makes it louder"
        );
        // Inside the band the kill has to be plainly deeper, not merely not louder: the point of the
        // detent is a band that goes away.
        if hertz <= 250.0 {
            assert!(
                kill_db <= cut_db - 1.0,
                "at {hertz:.0} Hz the detent is only {:.1} dB deeper than the cut",
                cut_db - kill_db
            );
        }
    }
}
