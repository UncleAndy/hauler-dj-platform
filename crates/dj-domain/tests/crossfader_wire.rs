//! How a crossfader law travels: the cut-in law carries its own number, and the old laws keep their names.
//!
//! A client that cannot say where the fader opens cannot show it, and a law that arrives without its number
//! would silently open somewhere else.

#![allow(clippy::expect_used)]

use dj_domain::CrossfaderCurve;

#[test]
fn a_cut_law_travels_with_its_number() {
    let law = CrossfaderCurve::Cut { cut_in: 7 };
    let wire = serde_json::to_string(&law).expect("a law must serialize");
    assert_eq!(
        wire, "{\"cut\":{\"cut_in\":7}}",
        "the law's wire form changed"
    );
    let back: CrossfaderCurve = serde_json::from_str(&wire).expect("the law must read back");
    assert_eq!(back, law, "the law did not survive the wire");
}

#[test]
fn the_old_laws_keep_their_names() {
    for name in ["linear", "constant_power", "smooth", "sharp"] {
        let wire = format!("\"{name}\"");
        let back: CrossfaderCurve = serde_json::from_str(&wire).expect("an old law must read");
        assert_eq!(back.as_str(), name);
        let out = serde_json::to_string(&back).expect("an old law must write");
        assert_eq!(out, wire, "an old law changed its wire form");
    }
}
