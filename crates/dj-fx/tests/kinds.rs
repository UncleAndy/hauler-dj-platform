//! The names of the seven effects are wire format: a client and an engine that disagree about one
//! string disagree about what the deck is playing (ADR-003).

use dj_fx::{Dry, FxKind, FxPatch, FxUnit};

/// The gate denies `unwrap`/`expect` even in tests (`-D warnings`), so a broken serialisation is a panic with
/// the reason spelled out instead of a lint failure.
fn json_of<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(text) => text,
        Err(error) => panic!("plain data does not serialise: {error}"),
    }
}

fn kind_from(text: &str) -> FxKind {
    match serde_json::from_str(text) {
        Ok(kind) => kind,
        Err(error) => panic!("{text} does not parse back as a kind: {error}"),
    }
}

fn patch_from(text: &str) -> FxPatch {
    match serde_json::from_str(text) {
        Ok(patch) => patch,
        Err(error) => panic!("{text} does not parse back as a patch: {error}"),
    }
}

#[test]
fn every_kind_round_trips_on_the_wire() {
    for kind in FxKind::ALL {
        let json = json_of(&kind);
        let back: FxKind = kind_from(&json);
        assert_eq!(back, kind);
    }
}

#[test]
fn the_names_are_the_ones_the_protocol_promises() {
    let pairs = [
        (FxKind::Filter, "\"filter\""),
        (FxKind::Echo, "\"echo\""),
        (FxKind::Roll, "\"roll\""),
        (FxKind::Flanger, "\"flanger\""),
        (FxKind::Phaser, "\"phaser\""),
        (FxKind::Bitcrusher, "\"bitcrusher\""),
        (FxKind::Noise, "\"noise\""),
    ];
    for (kind, name) in pairs {
        assert_eq!(json_of(&kind), name);
    }
}

#[test]
fn the_patch_keeps_its_three_parts() {
    let patch = FxPatch {
        kind: FxKind::Echo,
        amount: 0.75,
        enabled: true,
    };
    let json = json_of(&patch);
    let back: FxPatch = patch_from(&json);
    assert_eq!(back.kind, FxKind::Echo);
    assert!((back.amount - 0.75).abs() < 1e-6);
    assert!(back.enabled);
}

#[test]
fn an_empty_slot_is_the_signal_itself() {
    // "Off" has one meaning on the render path: the same samples, bit for bit.
    let input: Vec<f32> = (0..64).map(|i| (i as f32 * 0.031).sin()).collect();
    let mut out = vec![0.0; input.len()];
    let mut dry = Dry;
    dry.prepare(48_000.0, 32);
    dry.process(&input, &mut out, 32);
    assert_eq!(out, input);
}
