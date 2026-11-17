//! Crossfader laws: the shapes a selector offers, and the one a scratch mixer has.
//!
//! Every law is measured the same way — the gain of a side against the handle's travel — so the numbers here
//! can be compared with each other and with what a DJ hears.

#![allow(clippy::expect_used, clippy::cast_precision_loss)]

use dj_domain::CrossfaderCurve;
use dj_mixer::{CrossAssign, Mixer};

fn gain_at(curve: CrossfaderCurve, position: f32) -> (f32, f32) {
    let mut mixer = Mixer::new(2);
    mixer.set_curve(curve);
    mixer.set_crossfader(position);
    (
        mixer.crossfader_gain(CrossAssign::A),
        mixer.crossfader_gain(CrossAssign::B),
    )
}

fn db(gain: f32) -> f32 {
    if gain <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * gain.log10()
    }
}

/// The cut-in law opens where the DJ says it does: shut below the cut, open two percent later.
///
/// The numbers are the ones the plan promised before the code existed (cut at five percent: −60 dB at four,
/// 0 dB at six), so the measurement cannot have been fitted to whatever the code happened to do.
#[test]
fn a_cut_curve_opens_at_the_cut_in_point() {
    let law = CrossfaderCurve::Cut { cut_in: 5 };
    // Side B is the one that opens as the handle moves right from the left edge.
    let (_, below) = gain_at(law, 0.04);
    let (_, at) = gain_at(law, 0.06);
    assert!(
        db(below) <= -59.9,
        "four percent of the travel should be shut, it measured {:.1} dB",
        db(below)
    );
    assert!(
        db(at) >= -0.1,
        "six percent of the travel should be open, it measured {:.1} dB",
        db(at)
    );
    // And it stays open: the law is a gate at the end of the travel, not a balance.
    let (_, wide) = gain_at(law, 0.8);
    assert!(
        db(wide) >= -0.1,
        "past the cut the fader must stay open, it measured {:.1} dB",
        db(wide)
    );
    // The other side is the mirror of it: at the same place the first side is shut.
    let (other, _) = gain_at(law, 0.04);
    assert!(
        db(other) >= -0.1,
        "the side the fader points at must be open, it measured {:.1} dB",
        db(other)
    );
}

/// Where the cut sits is the caller's number, not a constant: seven percent opens later than five.
#[test]
fn the_cut_in_is_the_number_the_dj_asks_for() {
    let early = CrossfaderCurve::Cut { cut_in: 5 };
    let late = CrossfaderCurve::Cut { cut_in: 7 };
    let (_, early_at_six) = gain_at(early, 0.06);
    let (_, late_at_six) = gain_at(late, 0.06);
    assert!(
        db(early_at_six) > db(late_at_six) + 20.0,
        "at six percent the five-percent cut should be open and the seven-percent cut still shut ({:.1} dB against {:.1} dB)",
        db(early_at_six),
        db(late_at_six)
    );
}

/// Hamster turns the fader round: the handle on the left opens the other side.
#[test]
fn the_hamster_switches_the_sides() {
    let mut mixer = Mixer::new(2);
    mixer.set_curve(CrossfaderCurve::ConstantPower);
    mixer.set_crossfader(0.1);
    let (a_before, b_before) = (
        mixer.crossfader_gain(CrossAssign::A),
        mixer.crossfader_gain(CrossAssign::B),
    );
    assert!(
        a_before > b_before,
        "the handle at the left opens the first side"
    );
    mixer.set_hamster(true);
    assert!(mixer.hamster());
    let (a_after, b_after) = (
        mixer.crossfader_gain(CrossAssign::A),
        mixer.crossfader_gain(CrossAssign::B),
    );
    assert!(
        b_after > a_after,
        "with the hamster on the same handle must open the other side"
    );
    mixer.set_hamster(false);
    let (a_back, _) = (mixer.crossfader_gain(CrossAssign::A), ());
    assert!(
        (a_back - a_before).abs() < 1e-6,
        "turning it off must come back"
    );
}

/// The law a client names on the command line is the law the engine runs.
#[test]
fn every_law_can_be_named_and_parsed_back() {
    for law in CrossfaderCurve::ALL {
        let name = law.as_str();
        let parsed = CrossfaderCurve::parse(name).expect("a law's own name must parse");
        assert_eq!(parsed, law, "{name} did not come back");
    }
    assert_eq!(
        CrossfaderCurve::parse("cut:7"),
        Some(CrossfaderCurve::Cut { cut_in: 7 })
    );
    assert_eq!(CrossfaderCurve::parse("nonsense"), None);
}
