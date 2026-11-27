//! Перевод между словами провода и словами индекса: одна функция, один тест.
#![allow(clippy::expect_used)]

use dj_domain::{CollectionOrder, CollectionQuery, CollectionSort};
use dj_library::index::OrderBy;

#[test]
fn the_question_reaches_the_index_unchanged() {
    let query = CollectionQuery {
        id: 7,
        order: vec![
            CollectionOrder {
                column: CollectionSort::Bpm,
                ascending: true,
            },
            CollectionOrder {
                column: CollectionSort::Key,
                ascending: false,
            },
        ],
        genres: vec!["House".to_owned(), "Techno".to_owned()],
        search: Some("mango".to_owned()),
        offset: 40,
        limit: 20,
        roots: Vec::new(),
        genres_only: false,
    };
    let asked = dj_engine::page_query(&query);
    assert_eq!(
        asked.order,
        vec![(OrderBy::Bpm, true), (OrderBy::Key, false)],
        "колонки и направления те же"
    );
    assert_eq!(asked.genres, vec!["House", "Techno"]);
    assert_eq!(asked.search.as_deref(), Some("mango"));
    assert_eq!((asked.offset, asked.limit), (40, 20));
}

#[test]
fn every_sort_column_has_a_name_in_the_index() {
    // A column added to the protocol and forgotten here is a column that sorts nothing: the list would
    // come back in whatever order the database felt like.
    for column in [
        CollectionSort::Artist,
        CollectionSort::Album,
        CollectionSort::Title,
        CollectionSort::Bpm,
        CollectionSort::Key,
        CollectionSort::Time,
    ] {
        let query = CollectionQuery {
            id: 1,
            order: vec![CollectionOrder {
                column,
                ascending: true,
            }],
            ..CollectionQuery::default()
        };
        let asked = dj_engine::page_query(&query);
        assert_eq!(asked.order.len(), 1, "{column:?} не перевёлся");
    }
}

#[test]
fn a_row_keeps_everything_the_list_shows() {
    let row = dj_library::TrackRow {
        fingerprint: "10:a:b".to_owned(),
        path: "/music/A - B.flac".to_owned(),
        artist: "A".to_owned(),
        title: "B".to_owned(),
        album: "C".to_owned(),
        genre: "House, Techno".to_owned(),
        year: Some(2021),
        bpm: Some(128.5),
        musical_key: Some("8d".to_owned()),
        rating: 4,
        duration_frames: Some(48_000),
        sample_rate: None,
        gain: 1.0,
        play_count: 0,
        last_played_ms: None,
        analysis_lock: false,
        bpm_trend: String::new(),
    };
    let wire = dj_engine::collection_row(&row);
    assert_eq!(wire.fingerprint, "10:a:b");
    assert_eq!(wire.path, "/music/A - B.flac");
    assert_eq!(wire.genre, "House, Techno");
    assert_eq!(wire.bpm, Some(128.5));
    assert_eq!(wire.musical_key.as_deref(), Some("8d"));
    assert_eq!(wire.duration_frames, Some(48_000));
    assert_eq!(wire.rating, 4);
    assert!(
        wire.bpm_trend.is_empty(),
        "a track of one tempo says nothing about its grid"
    );

    // The symbol has to survive the wire as well: the window puts it after the tempo, and a field the page does not
    // carry is a field no list can see (owner's report, 26.11).
    let mut two_tempos = row;
    two_tempos.bpm_trend = "\u{2197}".to_owned();
    let wire = dj_engine::collection_row(&two_tempos);
    assert_eq!(wire.bpm_trend, "\u{2197}", "the page carries the arrow");
    let json = serde_json::to_value(&wire).expect("the page is JSON on the wire");
    assert_eq!(
        json.get("bpm_trend").and_then(|value| value.as_str()),
        Some("\u{2197}"),
        "and the JSON a page is written as says which way the tempo moves"
    );
    let mut one_tempo = two_tempos.clone();
    one_tempo.bpm_trend = String::new();
    let json = serde_json::to_value(dj_engine::collection_row(&one_tempo)).expect("json");
    assert!(
        json.get("bpm_trend").is_none(),
        "a steady track does not write the field at all: the wire stays what it was"
    );
}
