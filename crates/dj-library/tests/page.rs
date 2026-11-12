//! Страница коллекции: сортировка, фильтр и поиск — в SQL, а не в памяти.
//!
//! Это то, что движок будет отдавать интерфейсу по мосту: список приходит страницей, и на сто тысяч
//! строк вход в раздел стоит столько же, сколько на тысячу.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use dj_library::index::{OrderBy, PageQuery};
use dj_library::{Index, TrackRow};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-page-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    root
}

fn row(title: &str, artist: &str, bpm: f64, genres: &[&str]) -> (TrackRow, Vec<String>) {
    let row = TrackRow {
        fingerprint: format!("{artist}:{title}"),
        path: format!("/music/{artist} - {title}.flac"),
        artist: artist.to_owned(),
        title: title.to_owned(),
        album: "Album".to_owned(),
        genre: genres.join(", "),
        year: None,
        bpm: if bpm > 0.0 { Some(bpm) } else { None },
        musical_key: None,
        rating: 0,
        duration_frames: None,
        sample_rate: None,
        gain: 1.0,
        play_count: 0,
        last_played_ms: None,
        analysis_lock: false,
    };
    let genres = genres.iter().map(|genre| (*genre).to_owned()).collect();
    (row, genres)
}

fn filled(tag: &str, rows: &[(TrackRow, Vec<String>)]) -> (Index, PathBuf) {
    let dir = scratch(tag);
    let mut index = Index::open(&dir.join("library.db")).expect("index");
    for (row, genres) in rows {
        let names = genres
            .iter()
            .map(|name| dj_library::genres::normalise(name).expect("genre"))
            .collect::<Vec<_>>();
        index.upsert_with_genres(row, &names).expect("index write");
    }
    (index, dir)
}

fn titles(page: &dj_library::index::Page) -> Vec<String> {
    page.rows.iter().map(|row| row.title.clone()).collect()
}

#[test]
fn a_page_comes_in_the_order_asked_for_and_only_as_wide_as_asked() {
    let (index, dir) = filled(
        "order",
        &[
            row("B", "Zed", 0.0, &[]),
            row("A", "Alpha", 0.0, &[]),
            row("C", "Mike", 0.0, &[]),
        ],
    );
    let first = index
        .page(&PageQuery {
            order: vec![(OrderBy::Artist, true)],
            limit: 2,
            ..PageQuery::default()
        })
        .expect("first page");
    assert_eq!(titles(&first), vec!["A", "C"]);
    assert_eq!(first.total, 3, "всего строк в отборе, а не на странице");

    let second = index
        .page(&PageQuery {
            order: vec![(OrderBy::Artist, true)],
            offset: 2,
            limit: 2,
            ..PageQuery::default()
        })
        .expect("second page");
    assert_eq!(titles(&second), vec!["B"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rows_without_a_value_go_last_whichever_way_the_sort_runs() {
    let (index, dir) = filled(
        "empty",
        &[
            row("slow", "A", 120.0, &[]),
            row("none", "B", 0.0, &[]),
            row("fast", "C", 128.0, &[]),
        ],
    );
    for ascending in [true, false] {
        let page = index
            .page(&PageQuery {
                order: vec![(OrderBy::Bpm, ascending)],
                limit: 10,
                ..PageQuery::default()
            })
            .expect("page");
        assert_eq!(
            titles(&page).last().map(String::as_str),
            Some("none"),
            "пустой BPM внизу при ascending={ascending}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn several_genres_are_a_union_and_an_empty_list_is_everything() {
    let (index, dir) = filled(
        "genres",
        &[
            row("house one", "A", 0.0, &["House"]),
            row("techno one", "B", 0.0, &["Techno"]),
            row("ambient one", "C", 0.0, &["Ambient"]),
        ],
    );
    let both = index
        .page(&PageQuery {
            order: vec![(OrderBy::Artist, true)],
            genres: vec!["House".to_owned(), "Ambient".to_owned()],
            limit: 10,
            ..PageQuery::default()
        })
        .expect("union");
    assert_eq!(titles(&both), vec!["house one", "ambient one"]);
    assert_eq!(both.total, 2);

    let all = index
        .page(&PageQuery {
            order: vec![(OrderBy::Artist, true)],
            limit: 10,
            ..PageQuery::default()
        })
        .expect("everything");
    assert_eq!(all.total, 3);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_search_looks_in_the_names_and_counts_what_it_finds() {
    let (index, dir) = filled(
        "search",
        &[
            row("Mango", "Alpha", 0.0, &[]),
            row("Apple", "Beta", 0.0, &[]),
            row("Mango", "Beta", 0.0, &[]),
        ],
    );
    let page = index
        .page(&PageQuery {
            order: vec![(OrderBy::Title, true), (OrderBy::Artist, true)],
            search: Some("mango".to_owned()),
            limit: 10,
            ..PageQuery::default()
        })
        .expect("search");
    assert_eq!(titles(&page), vec!["Mango", "Mango"]);
    assert_eq!(page.total, 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_page_belongs_to_the_roots_it_was_asked_for() {
    // The index is one for the machine; the list is one for the folders the DJ opened.
    let (mut inside, inside_genres) = row("A", "Alpha", 0.0, &[]);
    inside.path = "/music/one/A - Alpha.flac".to_owned();
    let (mut outside, outside_genres) = row("B", "Beta", 0.0, &[]);
    outside.path = "/music/two/B - Beta.flac".to_owned();
    // A folder whose name starts with the same letters is not the same folder.
    let (mut neighbour, neighbour_genres) = row("C", "Cat", 0.0, &[]);
    neighbour.path = "/music/one2/C - Cat.flac".to_owned();
    let (index, _dir) = filled(
        "roots",
        &[
            (inside, inside_genres),
            (outside, outside_genres),
            (neighbour, neighbour_genres),
        ],
    );

    let page = index
        .page(&dj_library::index::PageQuery {
            roots: vec![std::path::PathBuf::from("/music/one")],
            ..dj_library::index::PageQuery::default()
        })
        .expect("page");
    assert_eq!(page.total, 1, "в ответе только строки своего корня");
    assert_eq!(titles(&page), vec!["A".to_owned()]);
}
