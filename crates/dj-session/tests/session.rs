//! A08: a session survives a crash, a bad file and an older schema.
//!
//! Each test is one promise of DJP-SET-002/003: the document round-trips, a broken document costs one
//! write and not the set-up, a newer document is refused by name, an older one is migrated, and
//! autosave coalesces without ever blocking the caller.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use dj_session::{Autosave, DeckState, SessionDoc, SessionError, SessionStore, Source};

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-session-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn document(now_ms: u64) -> SessionDoc {
    let mut document = SessionDoc::new(now_ms);
    document.audio.device = Some("Traktor Kontrol S4 MK3".to_owned());
    document.decks.insert(
        "A".to_owned(),
        DeckState {
            track: Some("/Music/Album/Song.flac".to_owned()),
            position_frames: 441_000,
            gain: 0.9,
        },
    );
    document
}

#[test]
fn a_session_round_trips() {
    let store = SessionStore::new(workspace("roundtrip").join("session.json"));
    let written = document(1_000);
    store.save(&written).expect("save");
    let loaded = store.load();
    assert_eq!(loaded.source, Source::Current);
    assert_eq!(loaded.document, written);
    assert!(loaded.problem.is_none());
}

#[test]
fn a_broken_document_costs_one_write_not_the_setup() {
    let path = workspace("broken").join("session.json");
    let store = SessionStore::new(&path);
    store.save(&document(1_000)).expect("first");
    store.save(&document(2_000)).expect("second");
    assert!(store.backup_path().exists(), "the previous write is kept");

    std::fs::write(&path, b"{ not a session").expect("corrupt it");
    let loaded = store.load();
    assert_eq!(loaded.source, Source::Backup);
    assert_eq!(
        loaded.document,
        document(1_000),
        "the previous write answers"
    );
    assert!(loaded.problem.is_some(), "the reason must be visible");
    assert_eq!(
        std::fs::read(&path).expect("read"),
        b"{ not a session",
        "the broken file is left as it is"
    );
}

#[test]
fn nothing_readable_means_defaults_and_a_reason() {
    let path = workspace("nothing").join("session.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, b"garbage").expect("write");
    let store = SessionStore::new(&path);
    let loaded = store.load();
    assert_eq!(loaded.source, Source::Defaults);
    assert_eq!(loaded.document, SessionDoc::default());
    assert!(loaded.problem.is_some());
}

#[test]
fn a_newer_document_is_refused_by_name() {
    let path = workspace("newer").join("session.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &path,
        br#"{"schema":"djp-session-v1","version":99,"audio":{},"decks":{}}"#,
    )
    .expect("write");
    let store = SessionStore::new(&path);
    let loaded = store.load();
    assert_eq!(
        loaded.source,
        Source::Defaults,
        "a newer file is not guessed at"
    );
    match loaded.problem {
        Some(SessionError::TooNew { found, known }) => {
            assert_eq!(found, 99);
            assert_eq!(known, dj_session::VERSION);
        }
        other => panic!("expected TooNew, got {other:?}"),
    }
}

#[test]
fn an_older_document_is_migrated() {
    let path = workspace("migrate").join("session.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    // v0 kept the device at the top level and had no mixer section.
    std::fs::write(
        &path,
        br#"{
            "schema": "djp-session-v0",
            "version": 0,
            "audio_device": "Scarlett 2i2",
            "decks": {"B": {"track": "/Music/B.flac", "position_frames": 4800, "gain": 1.0}}
        }"#,
    )
    .expect("write");
    let store = SessionStore::new(&path);
    let loaded = store.load();
    assert_eq!(loaded.source, Source::Current);
    assert_eq!(loaded.document.version, dj_session::VERSION);
    assert_eq!(
        loaded.document.audio.device.as_deref(),
        Some("Scarlett 2i2"),
        "the old field is carried into the new shape"
    );
    assert_eq!(loaded.document.mixer.crossfader_curve, "constant_power");
    let deck = loaded.document.decks.get("B").expect("deck B");
    assert_eq!(deck.position_frames, 4_800);
}

#[test]
fn autosave_coalesces_and_lands_the_last_document() {
    let path = workspace("autosave").join("session.json");
    let store = SessionStore::new(&path);
    let saver = Autosave::start(store.clone());
    for step in 0..50u64 {
        let mut document = document(step);
        document.mixer.crossfader = step as f32 / 100.0;
        saver.request(&document);
    }
    saver.flush().expect("flush");
    drop(saver);

    let loaded = store.load();
    assert_eq!(loaded.source, Source::Current);
    assert_eq!(
        loaded.document.mixer.crossfader, 0.49,
        "the freshest one wins"
    );

    let leftovers: Vec<String> = std::fs::read_dir(path.parent().expect("parent"))
        .expect("list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
}

#[test]
fn the_document_holds_paths_not_audio() {
    let document = document(5_000);
    let text = serde_json::to_string(&document).expect("serialise");
    assert!(
        text.len() < 2_048,
        "a session is a page, not a track: {}",
        text.len()
    );
    assert!(text.contains("/Music/Album/Song.flac"));
    for forbidden in ["\"pcm\"", "\"samples\"", "\"audio_data\""] {
        assert!(
            !text.contains(forbidden),
            "the session must not carry {forbidden}"
        );
    }
}
