//! Жанры: строка тега разбирается на список, и одинаковые жанры не двоятся.
//!
//! Тег жанра — одна строка («House, Techno»), а списку коллекции нужен список. Правила разбора
//! консервативные: неразобранное остаётся одним жанром как есть, потерять жанр хуже, чем показать
//! его странно.

#![allow(clippy::expect_used)]

use dj_library::genres::{GenreName, MAX_GENRES, MAX_NAME_CHARS, split};

fn names(raw: &str) -> Vec<String> {
    split(raw).into_iter().map(|genre| genre.name).collect()
}

#[test]
fn a_comma_separated_tag_becomes_two_genres() {
    assert_eq!(names("House, Techno"), vec!["House", "Techno"]);
}

#[test]
fn an_ampersand_does_not_split_a_genre() {
    // «Drum & Bass» — один жанр: `&` соединяет слова, а не перечисляет жанры.
    assert_eq!(names("Drum & Bass"), vec!["Drum & Bass"]);
}

#[test]
fn the_same_genre_in_another_case_is_one_genre() {
    let genres = split("house; House ;HOUSE");
    assert_eq!(genres.len(), 1, "три написания — один жанр");
    assert_eq!(genres[0].name, "house", "побеждает первое написание");
    assert_eq!(genres[0].key, "house");
}

#[test]
fn a_slash_separates_and_the_second_name_keeps_its_space() {
    assert_eq!(names("House/Deep House"), vec!["House", "Deep House"]);
}

#[test]
fn an_empty_tag_holds_no_genres() {
    assert!(split("").is_empty());
    assert!(split("   ").is_empty());
    assert!(split(" , ; / ").is_empty());
}

#[test]
fn spaces_inside_a_name_are_collapsed() {
    assert_eq!(names("  Deep   House  "), vec!["Deep House"]);
}

#[test]
fn cyrillic_is_folded_too() {
    let genres = split("Техно, техно");
    assert_eq!(genres.len(), 1);
    assert_eq!(genres[0].name, "Техно");
    assert_eq!(genres[0].key, "техно");
}

#[test]
fn a_tag_with_forty_genres_keeps_the_first_sixteen() {
    let raw = (0..40)
        .map(|index| format!("Genre {index}"))
        .collect::<Vec<_>>()
        .join(",");
    let genres = split(&raw);
    assert_eq!(genres.len(), MAX_GENRES);
    assert_eq!(genres[0].name, "Genre 0");
    assert_eq!(genres[MAX_GENRES - 1].name, "Genre 15");
}

#[test]
fn a_name_longer_than_the_limit_is_not_a_genre() {
    let long = "x".repeat(MAX_NAME_CHARS + 1);
    assert!(split(&long).is_empty(), "абзац в теге жанра — не жанр");
}

#[test]
fn a_name_at_the_limit_is_kept() {
    let name = "x".repeat(MAX_NAME_CHARS);
    assert_eq!(split(&name).len(), 1);
}

#[test]
fn the_structured_form_is_public() {
    // Индексу нужны и написание, и ключ: оба приходят из одного вызова.
    let genres: Vec<GenreName> = split("Techno");
    assert_eq!(genres[0].name, "Techno");
    assert_eq!(genres[0].key, "techno");
}

// ---- индекс: жанры трека, список жанров, фильтр по ним -------------------------------------------

use std::path::PathBuf;

use dj_library::{Index, TrackRow};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-genres-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn row(fingerprint: &str, title: &str) -> TrackRow {
    TrackRow {
        fingerprint: fingerprint.to_owned(),
        path: format!("/music/{fingerprint}.flac"),
        artist: "Artist".to_owned(),
        title: title.to_owned(),
        album: String::new(),
        genre: String::new(),
        year: None,
        bpm: None,
        musical_key: None,
        rating: 0,
        duration_frames: None,
        gain: 1.0,
        play_count: 0,
        last_played_ms: None,
        analysis_lock: false,
    }
}

fn with_genres(index: &mut Index, fingerprint: &str, raw: &str) {
    index.upsert(&row(fingerprint, fingerprint)).expect("row");
    index.set_genres(fingerprint, &split(raw)).expect("genres");
}

#[test]
fn setting_the_same_genres_twice_leaves_one_link_each() {
    let dir = scratch("twice");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    with_genres(&mut index, "a", "House, Techno");
    index
        .set_genres("a", &split("House, Techno"))
        .expect("second set");
    assert_eq!(
        index.genres_of("a").expect("genres"),
        vec!["House", "Techno"]
    );
    assert_eq!(
        index.genre_list().expect("list"),
        vec![("House".to_owned(), 1), ("Techno".to_owned(), 1)]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn setting_genres_replaces_them_instead_of_appending() {
    let dir = scratch("replace");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    with_genres(&mut index, "a", "House, Techno");
    index.set_genres("a", &split("House")).expect("narrow");
    assert_eq!(index.genres_of("a").expect("genres"), vec!["House"]);
    assert_eq!(
        index.genre_list().expect("list"),
        vec![("House".to_owned(), 1)],
        "a genre nobody carries any more is not in the list"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_genre_list_counts_the_tracks_that_carry_each_genre() {
    let dir = scratch("counts");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    with_genres(&mut index, "a", "House, Techno");
    with_genres(&mut index, "b", "House");
    with_genres(&mut index, "c", "Ambient");
    assert_eq!(
        index.genre_list().expect("list"),
        vec![
            ("Ambient".to_owned(), 1),
            ("House".to_owned(), 2),
            ("Techno".to_owned(), 1),
        ]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn several_genres_are_a_union_of_tracks() {
    let dir = scratch("union");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    with_genres(&mut index, "a", "House");
    with_genres(&mut index, "b", "Techno");
    with_genres(&mut index, "c", "Ambient");
    let found = index
        .tracks_with_genres(&["Techno".to_owned(), "Ambient".to_owned()])
        .expect("tracks");
    assert_eq!(found, vec!["b", "c"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_genre_filter_selects_no_tracks() {
    let dir = scratch("empty");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    with_genres(&mut index, "a", "House");
    assert!(
        index.tracks_with_genres(&[]).expect("tracks").is_empty(),
        "no genres chosen is not «all genres»"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_genre_filter_ignores_case() {
    let dir = scratch("case");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    with_genres(&mut index, "a", "House");
    assert_eq!(
        index
            .tracks_with_genres(&["house".to_owned()])
            .expect("tracks"),
        vec!["a"]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_rebuild_that_drops_the_tracks_drops_their_genres() {
    let dir = scratch("cascade");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    with_genres(&mut index, "a", "House");
    index.rebuild(&[]).expect("rebuild empty");
    assert_eq!(index.count().expect("count"), 0);
    assert!(
        index.genre_list().expect("list").is_empty(),
        "a genre left behind would point at a track that is gone"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_row_and_its_genres_are_written_together() {
    let dir = scratch("together");
    let mut index = Index::open(&dir.join("library.db")).expect("open");
    index
        .upsert_with_genres(&row("a", "Track"), &split("Deep House; Soulful"))
        .expect("upsert");
    assert_eq!(
        index.genres_of("a").expect("genres"),
        vec!["Deep House", "Soulful"]
    );
    assert_eq!(index.count().expect("count"), 1);
    std::fs::remove_dir_all(&dir).ok();
}

// ---- импорт: жанры в сайдкар и в индекс ----------------------------------------------------------

use dj_library::import::index_row;
use dj_library::sidecar;

/// Короткий тон с шагом по высоте: ровный тон даёт вырожденный отпечаток, а повторяющаяся гамма —
/// сдвиг не там, где он есть.
fn write_tone(path: &std::path::Path) {
    const PITCHES: [f64; 12] = [
        1.0, 1.122, 1.26, 1.335, 1.498, 1.682, 1.782, 2.0, 2.245, 2.52, 2.67, 2.997,
    ];
    let rate = 22_050_u32;
    let seconds = 2_u32;
    let samples = rate * seconds;
    let mut data = Vec::with_capacity(samples as usize * 2);
    let mut phase = 0.0_f64;
    let mut noise = 0x2545_F491_4F6C_DD1D_u64;
    let mut step = 440.0_f64;
    for index in 0..samples {
        if index.is_multiple_of(rate / 4) {
            noise = noise
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            step = 440.0 * PITCHES[(noise >> 33) as usize % PITCHES.len()];
        }
        phase += std::f64::consts::TAU * step / f64::from(rate);
        data.extend_from_slice(&((phase.sin() * 12_000.0) as i16).to_le_bytes());
    }
    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16_u32.to_le_bytes());
    file.extend_from_slice(&1_u16.to_le_bytes());
    file.extend_from_slice(&1_u16.to_le_bytes());
    file.extend_from_slice(&rate.to_le_bytes());
    file.extend_from_slice(&(rate * 2).to_le_bytes());
    file.extend_from_slice(&2_u16.to_le_bytes());
    file.extend_from_slice(&16_u16.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(&data);
    std::fs::write(path, file).expect("wav");
}

#[test]
fn an_imported_track_is_listed_by_the_genres_of_its_tag() {
    let dir = scratch("import");
    let source = dir.join("Artist - Track.wav");
    write_tone(&source);
    let collection = dir.join("collection");
    std::fs::create_dir_all(&collection).expect("collection");
    let mut imported = dj_library::import(&source, &collection, 1_000).expect("import");
    // The tag of the copy: the import reads it from the container, and a WAV written by this test
    // cannot carry one that the reader gives back — the container path itself is `tags::read`, which
    // is covered by its own tests and by the probe over the real collection.
    imported.tags.genre = "House, Techno".to_owned();

    let (row, genres) = index_row(&imported);
    assert_eq!(row.genre, "House, Techno", "the raw tag stays in the row");
    assert_eq!(row.path, imported.path.to_string_lossy());
    assert_eq!(
        genres
            .iter()
            .map(|genre| genre.name.clone())
            .collect::<Vec<_>>(),
        vec!["House", "Techno"]
    );

    let mut index = Index::open(&dir.join("library.db")).expect("index");
    index
        .upsert_with_genres(&row, &genres)
        .expect("index write");
    assert_eq!(
        index.genres_of(&row.fingerprint).expect("genres"),
        vec!["House", "Techno"],
        "the track is found by both genres it carries"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_imported_track_writes_an_empty_genre_list_when_the_tag_has_none() {
    let dir = scratch("import-empty");
    let source = dir.join("Artist - Track.wav");
    write_tone(&source);
    let collection = dir.join("collection");
    std::fs::create_dir_all(&collection).expect("collection");
    let imported = dj_library::import(&source, &collection, 1_000).expect("import");
    let meta = sidecar::read(&imported.meta).expect("sidecar");
    let written = meta.tags.expect("tags in the sidecar");
    assert!(
        written.genres.is_empty(),
        "a track without a genre tag carries no genres, not a missing field"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_sidecar_without_genres_still_reads() {
    // A record written before the field existed: the browser must read it, not fail on it.
    let record = r#"{"schema":"djmeta/1","revision":1,"updated_at_ms":1000,
                     "fingerprint":{"size":1,"head":"a","tail":"b"},
                     "tags":{"artist":"A","album":"B","title":"C"}}"#;
    let meta: dj_library::TrackMeta = serde_json::from_str(record).expect("old sidecar");
    let written = meta.tags.expect("tags");
    assert!(
        written.genres.is_empty(),
        "старая запись — без жанров, не с ошибкой"
    );
}
