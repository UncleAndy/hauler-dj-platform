//! Static guard for the A14 rule: the coefficients are built off the callback (DJP-NFR-001).
//!
//! The allocation counter proves that a knob does not allocate; it says nothing about arithmetic,
//! and `powf`/`sin_cos` cost far more inside a 5,3 ms block than a single store does. The table is
//! what makes a knob cheap, so this test reads the two functions the callback is allowed to call
//! and refuses a transcendental inside them — the compiler would not notice, and no listening test
//! would either.

use std::path::Path;

/// Arithmetic that must never run per knob move or per block.
const FORBIDDEN: &[(&str, &str)] = &[
    ("powf", "a power on the audio path is a table row instead"),
    ("powi", "a power on the audio path is a table row instead"),
    (
        "sin_cos",
        "trigonometry on the audio path is a table row instead",
    ),
    (
        "log10",
        "a logarithm on the audio path is a table row instead",
    ),
    (
        "exp(",
        "an exponential on the audio path is a table row instead",
    ),
    (
        "sqrt(",
        "a square root on the audio path is a table row instead",
    ),
];

/// The body of `fn <signature>`: from its opening brace to the matching closing one.
fn body_of(source: &str, signature: &str) -> String {
    let start = source.find(signature).unwrap_or_else(|| {
        panic!("`{signature}` is gone: the guard would be checking nothing");
    });
    let open = source[start..].find('{').map(|offset| start + offset);
    let Some(open) = open else {
        panic!("`{signature}` has no body");
    };
    let mut depth = 0usize;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[open..=open + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("`{signature}` is not closed");
}

#[test]
fn a_knob_move_and_a_block_are_free_of_transcendentals() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/eq.rs");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{err}"));

    let set_gains = body_of(&source, "pub fn set_gains");
    // The body has to be the one that hands the gains to the split: an empty or renamed function would
    // make every assertion below true for the wrong reason.
    assert!(
        set_gains.contains(".set_gains("),
        "set_gains no longer hands the gains on: {set_gains}"
    );
    let process = body_of(&source, "pub fn process(&mut self, frames: &mut [f32])");
    assert!(
        process.contains(".process("),
        "process no longer runs a section: {process}"
    );

    let mut violations = Vec::new();
    for (signature, body) in [("set_gains", &set_gains), ("process", &process)] {
        for (needle, why) in FORBIDDEN {
            if body.contains(needle) {
                violations.push(format!("{signature}: {needle} — {why}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "the callback path computes instead of looking up:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_coefficients_are_designed_where_transcendentals_are_allowed() {
    // The counterpart: the design has to happen when the equaliser is built, otherwise the guard above
    // is satisfied by a split that designs nothing and the first knob move would pay for it.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/eq.rs");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{err}"));
    let build = body_of(&source, "pub fn new(sample_rate: f64) -> Self");
    assert!(
        build.contains("Isolator::new("),
        "the equaliser no longer designs its split when it is built: {build}"
    );
}
