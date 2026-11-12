//! Жанры: строка тега разбирается на список, и одинаковые жанры не двоятся.
//!
//! Тег жанра — одна строка («House, Techno»), а списку коллекции нужен список. Правила разбора
//! консервативные: неразобранное остаётся одним жанром как есть, потерять жанр хуже, чем показать
//! его странно.

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
