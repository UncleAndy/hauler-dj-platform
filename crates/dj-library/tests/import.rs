//! Импорт файла в коллекцию: имя, место, теги, метаданные.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use dj_library::{import, meta, sidecar, tags};

/// Эталонный файл; тесты его только читают и копируют.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dj-codec/tests/fixtures/audio")
        .join(name)
}

/// Свой каталог на каждый тест: чужие прогоны не мешают, а уборка — за нами.
fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-import-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch");
    root
}

fn tagged_copy(root: &Path, artist: &str, title: &str) -> PathBuf {
    let copy = root.join("incoming").join("track.flac");
    fs::create_dir_all(copy.parent().expect("parent")).expect("incoming");
    fs::copy(fixture("tone-1s-44100.flac"), &copy).expect("copy fixture");
    let mut written = tags::Tags {
        artist: artist.to_owned(),
        title: title.to_owned(),
        ..tags::Tags::default()
    };
    written.count = 2;
    assert!(
        tags::write(&copy, &written).expect("write tags"),
        "тег не записался"
    );
    copy
}

#[test]
fn a_track_lands_where_its_tags_say() {
    let root = scratch("layout");
    let source = tagged_copy(&root, "Abba", "Dancing Queen");
    let collection = root.join("collection");

    let imported = import(&source, &collection, 1_000).expect("import");

    assert_eq!(
        imported.path,
        collection.join("A/B/Abba - Dancing Queen.flac"),
        "разложили не туда"
    );
    assert!(imported.path.is_file(), "файла нет");
    assert!(!imported.repaired && !imported.named_from_file);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_source_is_copied_and_left_alone() {
    let root = scratch("source");
    let source = tagged_copy(&root, "Abba", "Dancing Queen");
    let before = fs::read(&source).expect("read source");
    let collection = root.join("collection");

    let imported = import(&source, &collection, 1_000).expect("import");

    assert!(source.is_file(), "исходник пропал");
    assert_eq!(
        fs::read(&source).expect("read again"),
        before,
        "исходник изменился"
    );
    assert_eq!(
        fs::read(&imported.path).expect("read copy"),
        before,
        "копия не совпала с исходником"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_file_without_tags_is_named_after_itself() {
    let root = scratch("untagged");
    let source = root.join("incoming").join("tone-1s-44100.flac");
    fs::create_dir_all(source.parent().expect("parent")).expect("incoming");
    fs::copy(fixture("tone-1s-44100.flac"), &source).expect("copy fixture");
    let collection = root.join("collection");

    let imported = import(&source, &collection, 1_000).expect("import");

    assert_eq!(
        imported.path,
        collection.join("T/O/tone-1s-44100.flac"),
        "имя без тегов взято не из файла"
    );
    assert!(imported.named_from_file);
    // Теги копии приведены в согласие с именем: коллекция сама себя объясняет.
    let read = tags::read(&imported.path).expect("read tags");
    assert_eq!(read.title, "tone-1s-44100");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_metadata_file_sits_beside_the_copy() {
    let root = scratch("meta");
    let source = tagged_copy(&root, "Abba", "Dancing Queen");
    let collection = root.join("collection");

    let imported = import(&source, &collection, 1_000).expect("import");

    assert_eq!(
        imported.meta,
        collection.join("A/B/.djmeta/Abba - Dancing Queen.flac.json"),
        "метаданные не рядом с копией"
    );
    let stored = sidecar::read(&imported.meta).expect("read meta");
    assert_eq!(stored.schema, meta::SCHEMA);
    assert!(stored.fingerprint.size > 0, "отпечаток пустой");
    assert_eq!(stored.updated_at_ms, 1_000);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn importing_the_same_track_twice_keeps_both() {
    let root = scratch("twice");
    let source = tagged_copy(&root, "Abba", "Dancing Queen");
    let collection = root.join("collection");

    let first = import(&source, &collection, 1_000).expect("first");
    let second = import(&source, &collection, 2_000).expect("second");

    assert_ne!(first.path, second.path, "второй импорт затёр первый");
    assert_eq!(
        second.path,
        collection.join("A/B/Abba - Dancing Queen (2).flac")
    );
    assert!(first.path.is_file() && second.path.is_file());
    // Вопрос «дубль или версия» задаётся позже и по отпечаткам, а не по имени файла.
    assert_eq!(
        sidecar::read(&first.meta)
            .expect("meta 1")
            .fingerprint
            .key(),
        sidecar::read(&second.meta)
            .expect("meta 2")
            .fingerprint
            .key(),
        "одинаковые файлы должны иметь один отпечаток"
    );
    let _ = fs::remove_dir_all(&root);
}
