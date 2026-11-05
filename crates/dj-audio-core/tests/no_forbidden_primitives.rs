//! Static guard for the RT contract: the core crate must not contain the primitives that
//! would let the callback block, allocate implicitly or print (DJP-NFR-001).
//!
//! The allocation counter proves today's behaviour; this test keeps it true tomorrow, when
//! someone reaches for a `Mutex` to "just share" a counter with the UI.

use std::path::{Path, PathBuf};

const FORBIDDEN: &[(&str, &str)] = &[
    ("Mutex", "a lock on the audio path can block the callback"),
    ("RwLock", "a lock on the audio path can block the callback"),
    ("Condvar", "waiting on the audio path is forbidden"),
    (
        "OnceLock",
        "lazy initialisation on the audio path is forbidden",
    ),
    (
        "OnceCell",
        "lazy initialisation on the audio path is forbidden",
    ),
    ("thread::sleep", "the callback must never sleep"),
    ("thread::spawn", "the callback must never spawn threads"),
    ("println!", "I/O on the audio path is forbidden"),
    ("eprintln!", "I/O on the audio path is forbidden"),
    ("std::fs", "file I/O on the audio path is forbidden"),
];

fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    files
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn the_core_crate_holds_no_blocking_primitives() {
    let files = sources();
    assert!(
        !files.is_empty(),
        "no sources found: the guard would be vacuous"
    );

    let mut violations = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_else(|err| panic!("{err}"));
        // Tests may print and use whatever they like; the guard covers production code.
        let in_tests = text
            .find("\n#[cfg(test)]")
            .map(|index| text[..index].to_string());
        let audited = in_tests.unwrap_or(text);
        for (needle, why) in FORBIDDEN {
            if audited.contains(needle) {
                violations.push(format!(
                    "{}: {needle} — {why}",
                    file.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(file)
                        .display()
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "forbidden primitives in the audio core:\n{}",
        violations.join("\n")
    );
}
