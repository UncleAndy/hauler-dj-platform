//! Genres during analysis: the tag is read once, split, and written to the record and the index.
//!
//! A track that was never imported still has to appear in the collection by its genres — the engine
//! reads the tag when it first looks at the file, and the index learns about it then.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-genres-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    root
}

/// A file whose tags the reader understands. Our own WAV cannot carry one — symphonia reads no tags
/// from RIFF at all — so a FLAC from the codec fixtures is copied and retagged; the fixture itself is
/// never touched.
fn tagged_copy(dir: &Path, genre: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("crates/dj-codec/tests/fixtures/audio/tone-1s-44100.flac");
    let path = dir.join("Artist - Track.flac");
    std::fs::copy(&source, &path).expect("copy the fixture");
    let mut tags = dj_library::tags::read(&path).expect("read tags");
    tags.artist = "Artist".to_owned();
    tags.title = "Track".to_owned();
    tags.album = "Album".to_owned();
    tags.genre = genre.to_owned();
    dj_library::tags::write(&path, &tags).expect("write tags");
    assert_eq!(
        dj_library::tags::read(&path).expect("read back").genre,
        genre,
        "предпосылка: тег должен читаться из копии"
    );
    path
}

#[test]
fn analysis_writes_the_genres_of_the_tag_into_the_record_and_the_index() {
    let dir = scratch("write");
    let path = tagged_copy(&dir, "Deep House; Soulful");
    let store = dj_library::Store::new(dir.join("mirror"));
    let mut index = dj_library::Index::open(&dir.join("library.db")).expect("index");

    let wrote = dj_engine::meta::write_names(&store, "vol", &dir, &path, false, Some(&mut index));
    assert!(wrote, "the names and the genres reach the record");

    let loaded = store.load("vol", &dir, &path).expect("record");
    let tags = loaded.meta.tags.expect("tags in the record");
    assert_eq!(
        tags.genres,
        vec!["Deep House", "Soulful"],
        "жанры в .djmeta"
    );
    assert_eq!(
        index
            .genres_of(&loaded.meta.fingerprint.key())
            .expect("genres in the index"),
        vec!["Deep House", "Soulful"]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_second_analysis_replaces_the_genres_instead_of_adding_them() {
    let dir = scratch("again");
    let path = tagged_copy(&dir, "Deep House; Soulful");
    let store = dj_library::Store::new(dir.join("mirror"));
    let mut index = dj_library::Index::open(&dir.join("library.db")).expect("index");
    assert!(dj_engine::meta::write_names(
        &store,
        "vol",
        &dir,
        &path,
        false,
        Some(&mut index)
    ));

    // Another program retagged the file: the second analysis must follow the file, not the old record.
    let mut tags = dj_library::tags::read(&path).expect("read tags");
    tags.genre = "Techno".to_owned();
    dj_library::tags::write(&path, &tags).expect("retag");
    assert!(dj_engine::meta::write_names(
        &store,
        "vol",
        &dir,
        &path,
        true,
        Some(&mut index)
    ));

    let loaded = store.load("vol", &dir, &path).expect("record");
    assert_eq!(
        loaded.meta.tags.expect("tags").genres,
        vec!["Techno"],
        "новый тег заменил старые жанры"
    );
    assert_eq!(
        index
            .genres_of(&loaded.meta.fingerprint.key())
            .expect("genres"),
        vec!["Techno"],
        "связи заменены, а не добавлены"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_record_with_names_is_not_read_again_without_a_request() {
    let dir = scratch("quiet");
    let path = tagged_copy(&dir, "Techno");
    let store = dj_library::Store::new(dir.join("mirror"));
    assert!(dj_engine::meta::write_names(
        &store, "vol", &dir, &path, false, None
    ));
    assert!(
        !dj_engine::meta::write_names(&store, "vol", &dir, &path, false, None),
        "a record that already has its names is not read again"
    );
    std::fs::remove_dir_all(&dir).ok();
}
