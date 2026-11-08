//! What happens to a queued edit when the engine stops (DJP-DATA-006, A13).
//!
//! The edit is written on a thread of its own, because the audio callback may not touch a file. So
//! "the cue is stored" only becomes true when that thread has written it — and the process exits as
//! soon as the last command is answered. A cue the user set and the engine forgot is exactly the
//! failure A13 exists to prevent.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use std::path::Path;

use dj_engine::MetaEdit;
use dj_engine::meta::MetaWriter;

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-engine-writer-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn edit(track: &Path) -> MetaEdit {
    MetaEdit {
        path: track.to_path_buf(),
        cues: vec![(2, 48_000)],
        cue_meta: Vec::new(),
        pad_loops: vec![(6, 96_000, 144_000)],
        loop_region: None,
    }
}

#[test]
fn the_last_edit_is_on_disk_when_the_writer_stops() {
    // A race is invisible in one iteration: the thread either won or it did not. Repeat it, and the
    // guarantee is either met every time or the test says so.
    for round in 0..200 {
        let dir = scratch(&format!("stop-{round}"));
        let track = dir.join("song.wav");
        std::fs::write(&track, vec![3u8; 4096]).expect("track");
        let store = dj_library::Store::new(dir.join("mirror"));

        let (writer, sender) = MetaWriter::spawn(store.clone(), "vol".to_string(), dir.clone());
        sender.send(edit(&track)).expect("queue an edit");
        drop(sender);
        // Shutting down: the queue closes and the writer is awaited, so the file is final here.
        drop(writer);

        let loaded = store.load("vol", &dir, &track).unwrap_or_else(|error| {
            panic!("round {round}: nothing reached the disk before the shutdown: {error}")
        });
        assert_eq!(
            loaded.meta.cues.first().map(|cue| cue.frame),
            Some(48_000),
            "round {round}: the cue queued before the shutdown is not in the file"
        );
        assert_eq!(
            loaded.meta.loops.first().map(|pad| (pad.start, pad.end)),
            Some((96_000, 144_000)),
            "round {round}: the pad loop queued before the shutdown is not in the file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_writer_with_nothing_to_do_stops_at_once() {
    // The other side of the same promise: an idle queue must not make the engine wait.
    let dir = scratch("idle");
    let store = dj_library::Store::new(dir.join("mirror"));
    let started = std::time::Instant::now();
    let (writer, sender) = MetaWriter::spawn(store, "vol".to_string(), dir.clone());
    drop(sender);
    drop(writer);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "an idle writer held the shutdown for {:?}",
        started.elapsed()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
