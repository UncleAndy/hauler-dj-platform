//! A rescan the DJ stops: the walk stops walking, keeps what it wrote, and says so.
//!
//! The button that did nothing (owner's report, 27.11) was not a window's fault: the walk never looked at a flag, so
//! there was nothing for a Cancel to set. These cases pin the walk itself — one stopped before it starts, one stopped in
//! the middle, and one left alone — because a press that arrives after the last file has nothing to stop.

#![allow(clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dj_engine::{StateCells, run_rescan};

/// A file the walk will find: the contents do not matter here — the step is the test's, not the engine's.
fn place(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), b"").expect("a file to walk");
}

fn cells_state(cells: &StateCells) -> dj_domain::ImportState {
    cells.import().expect("the walk publishes its state")
}

#[test]
fn a_rescan_that_is_stopped_before_it_starts_walks_nothing() {
    let dir = std::env::temp_dir().join(format!("dj-rescan-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    for name in ["one.wav", "two.wav", "three.wav"] {
        place(&dir, name);
    }
    let cells = Arc::new(StateCells::new());
    let cancel = AtomicBool::new(false);
    let running = AtomicBool::new(true);
    let measured = AtomicUsize::new(0);
    // The DJ pressed Cancel before the first file was reached — the window he sees is already saying "stopping".
    cancel.store(true, Ordering::Relaxed);

    let step = |_file: &Path, _index: Option<&mut dj_library::Index>| {
        measured.fetch_add(1, Ordering::Relaxed);
        true
    };
    run_rescan(
        &[dir.display().to_string()],
        &cells,
        &cancel,
        &running,
        &step,
        None,
    );

    assert_eq!(measured.load(Ordering::Relaxed), 0, "no file was measured");
    let state = cells_state(&cells);
    assert_eq!(state.total, 3, "the walk knew how many files there were");
    assert_eq!(state.done, 0, "and it did not count any of them as done");
    assert!(
        !state.running,
        "the walk says it is over, whatever the reason"
    );
    assert!(
        !running.load(Ordering::Relaxed),
        "the loop that answers a Cancel is told there is nothing left to stop"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_rescan_that_is_stopped_in_the_middle_keeps_what_it_wrote() {
    let dir = std::env::temp_dir().join(format!("dj-rescan-half-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    for name in ["one.wav", "two.wav", "three.wav"] {
        place(&dir, name);
    }
    let cells = Arc::new(StateCells::new());
    let cancel = AtomicBool::new(false);
    let running = AtomicBool::new(true);
    let measured = AtomicUsize::new(0);

    // The press lands while the first file is being written: that file's record stays, and nothing after it starts.
    let step = |_file: &Path, _index: Option<&mut dj_library::Index>| {
        measured.fetch_add(1, Ordering::Relaxed);
        cancel.store(true, Ordering::Relaxed);
        true
    };
    run_rescan(
        &[dir.display().to_string()],
        &cells,
        &cancel,
        &running,
        &step,
        None,
    );

    assert_eq!(
        measured.load(Ordering::Relaxed),
        1,
        "the file in hand was finished, and the two behind it were not started"
    );
    let state = cells_state(&cells);
    assert_eq!(state.done, 1, "one of three is what the window shows");
    assert_eq!(state.total, 3);
    assert!(!state.running);
    assert!(!running.load(Ordering::Relaxed));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_rescan_that_is_not_stopped_walks_every_file() {
    let dir = std::env::temp_dir().join(format!("dj-rescan-all-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    for name in ["one.wav", "two.wav"] {
        place(&dir, name);
    }
    let cells = Arc::new(StateCells::new());
    let cancel = AtomicBool::new(false);
    let running = AtomicBool::new(true);
    let measured = AtomicUsize::new(0);
    let step = |_file: &Path, _index: Option<&mut dj_library::Index>| {
        measured.fetch_add(1, Ordering::Relaxed);
        true
    };
    run_rescan(
        &[dir.display().to_string()],
        &cells,
        &cancel,
        &running,
        &step,
        None,
    );

    assert_eq!(
        measured.load(Ordering::Relaxed),
        2,
        "both files were walked"
    );
    let state = cells_state(&cells);
    assert_eq!(state.done, 2);
    assert_eq!(state.placed, 2, "and both were counted as measured");
    assert!(!state.running);
    assert!(!running.load(Ordering::Relaxed));
    std::fs::remove_dir_all(&dir).ok();
}
