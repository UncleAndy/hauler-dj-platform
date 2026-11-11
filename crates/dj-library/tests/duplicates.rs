//! Дубли при добавлении: та же запись, другое издание, другой трек.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use dj_library::identity::{self, Catalog, Identity, Verdict};
use dj_library::{Outcome, import_checked, meta, sidecar, tags};

/// Поиск по каталогу в том виде, в каком его ждёт импорт: замок и владение каталогом — за вызывающим.
fn find_in(catalog: &Catalog) -> impl Fn(&Identity) -> Option<(PathBuf, Verdict)> + '_ {
    move |identity| {
        catalog
            .find(identity)
            .map(|(path, verdict)| (path.to_path_buf(), verdict))
    }
}

fn catalogue(path: &Path, identity: &Identity) -> Catalog {
    let mut catalog = Catalog::new();
    catalog.add(path.to_path_buf(), identity.clone());
    catalog
}

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-dups-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch");
    root
}

/// Тон с началом через `skip` секунд: обрезка в начале сдвигает отпечаток, и именно так выглядит
/// копия, у которой срезали вступление.
fn write_tone_from(path: &Path, seconds: u32, frequency: f64, skip: u32) {
    const RATE: u32 = 44_100;
    /// Мелодия без периода: у ровного тона отпечаток вырожден, а у повторяющейся гаммы сдвиг
    /// находится не там, где он есть.
    const PITCHES: [f64; 12] = [
        1.0, 1.122, 1.26, 1.335, 1.498, 1.682, 1.782, 2.0, 2.245, 2.52, 2.67, 2.997,
    ];
    let samples = RATE * seconds;
    let mut data = Vec::with_capacity(samples as usize * 2);
    let mut phase = 0.0_f64;
    let mut noise = 0x2545_F491_4F6C_DD1D_u64;
    let mut step_frequency = frequency;
    for index in 0..samples {
        let absolute = index + RATE * skip;
        if absolute.is_multiple_of(RATE / 4) {
            noise = noise
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            step_frequency = frequency * PITCHES[(noise >> 33) as usize % PITCHES.len()];
        }
        phase += std::f64::consts::TAU * step_frequency / f64::from(RATE);
        let value = (phase.sin() * 12_000.0) as i16;
        data.extend_from_slice(&value.to_le_bytes());
    }
    write_wav(path, RATE, &data);
}

/// Тридцать секунд тона: настоящий отпечаток, который можно сравнивать.
fn write_tone(path: &Path, seconds: u32, frequency: f64) {
    write_tone_from(path, seconds, frequency, 0);
}

fn write_wav(path: &Path, rate: u32, data: &[u8]) {
    let mut wav = Vec::with_capacity(data.len() + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((data.len() + 36) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // моно
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(data);
    fs::write(path, wav).expect("wav");
}

fn tagged(
    root: &Path,
    name: &str,
    artist: &str,
    title: &str,
    seconds: u32,
    frequency: f64,
) -> PathBuf {
    let path = root.join("incoming").join(name);
    fs::create_dir_all(path.parent().expect("parent")).expect("incoming");
    write_tone(&path, seconds, frequency);
    let written = tags::Tags {
        artist: artist.to_owned(),
        title: title.to_owned(),
        count: 2,
        ..tags::Tags::default()
    };
    // WAV не держит теги так же охотно, как FLAC: имя файла здесь и есть источник имени.
    let _ = tags::write(&path, &written);
    path
}

#[test]
fn the_same_file_twice_is_a_duplicate_and_is_not_copied() {
    let root = scratch("same");
    let collection = root.join("collection");
    let source = tagged(&root, "A - Track.wav", "A", "Track", 30, 220.0);

    let first = import_checked(&source, &collection, 1_000, find_in(&Catalog::new()))
        .expect("первый импорт");
    let Outcome::Placed(placed) = first else {
        panic!("первый файл не лёг: {first:?}");
    };
    let catalog = catalogue(&placed.path, &placed.identity);

    let second =
        import_checked(&source, &collection, 2_000, find_in(&catalog)).expect("второй импорт");
    match second {
        Outcome::Duplicate {
            existing, verdict, ..
        } => {
            assert_eq!(existing, placed.path);
            assert!(matches!(verdict, Verdict::Duplicate { .. }), "{verdict:?}");
        }
        other => panic!("дубль не распознан: {other:?}"),
    }
    // Второй копии в коллекции нет: спросить — значит не копировать.
    let files = fs::read_dir(collection.join("A/T"))
        .expect("каталог")
        .flatten()
        .filter(|entry| entry.path().is_file())
        .count();
    assert_eq!(files, 1, "дубль всё-таки скопирован");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_copy_a_second_shorter_is_still_the_same_recording() {
    let root = scratch("trim");
    let collection = root.join("collection");
    let full = tagged(&root, "A - Track.wav", "A", "Track", 60, 220.0);
    // Та же запись, конец короче на секунду: у настоящих файлов разница бывает до 1,5 с — это копия,
    // а не другая версия. Случай «сдвиг отпечатка объясняет разницу» проверяется правилом в
    // `identity::tests::a_trimmed_copy_is_still_the_same_recording`, где сдвиг задан точно.
    let short = root.join("incoming").join("A - Track (short tail).wav");
    fs::create_dir_all(short.parent().expect("parent")).expect("incoming");
    write_tone_from(&short, 59, 220.0, 0);

    let Outcome::Placed(placed) =
        import_checked(&full, &collection, 1_000, find_in(&Catalog::new())).expect("полный")
    else {
        panic!("полный не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second = import_checked(&short, &collection, 2_000, find_in(&catalog)).expect("обрезанный");
    match second {
        Outcome::Duplicate { verdict, .. } => {
            assert!(matches!(verdict, Verdict::Duplicate { .. }), "{verdict:?}");
        }
        other => panic!("обрезанная копия не распознана: {other:?}"),
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn another_edition_is_a_version_and_both_stay() {
    let root = scratch("edition");
    let collection = root.join("collection");
    let dub = tagged(
        &root,
        "A - Bones (Dub Mix).wav",
        "A",
        "Bones (Dub Mix)",
        30,
        220.0,
    );
    let extended = tagged(
        &root,
        "A - Bones (Extended Mix).wav",
        "A",
        "Bones (Extended Mix)",
        30,
        220.0,
    );

    let Outcome::Placed(placed) =
        import_checked(&dub, &collection, 1_000, find_in(&Catalog::new())).expect("даб")
    else {
        panic!("даб не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second =
        import_checked(&extended, &collection, 2_000, find_in(&catalog)).expect("extended");
    match second {
        Outcome::Version {
            imported, verdict, ..
        } => {
            assert!(matches!(verdict, Verdict::Version { .. }), "{verdict:?}");
            assert!(imported.path.is_file(), "версия не легла в коллекцию");
            assert_ne!(imported.path, placed.path);
        }
        other => panic!("издание не распознано: {other:?}"),
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_stranger_is_placed_without_questions() {
    let root = scratch("stranger");
    let collection = root.join("collection");
    let first = tagged(&root, "A - One.wav", "A", "One", 30, 220.0);
    let other = tagged(&root, "B - Two.wav", "B", "Two", 30, 660.0);

    let Outcome::Placed(placed) =
        import_checked(&first, &collection, 1_000, find_in(&Catalog::new())).expect("первый")
    else {
        panic!("первый не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second = import_checked(&other, &collection, 2_000, find_in(&catalog)).expect("второй");
    assert!(matches!(second, Outcome::Placed(_)), "{second:?}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_chromaprint_travels_in_the_metadata_file() {
    let root = scratch("print");
    let collection = root.join("collection");
    let source = tagged(&root, "A - Track.wav", "A", "Track", 30, 220.0);

    let Outcome::Placed(placed) =
        import_checked(&source, &collection, 1_000, find_in(&Catalog::new())).expect("импорт")
    else {
        panic!("не лёг");
    };
    let record = sidecar::read(&placed.meta).expect("метаданные");
    let stored = record.chromaprint.expect("печати нет в .djmeta");
    let decoded = meta::decode_print(&stored).expect("печать не читается");
    assert_eq!(decoded, placed.identity.chromaprint);
    assert!(
        !decoded.is_empty(),
        "пустая печать не годится для поиска дублей"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_print_round_trips_through_text() {
    let print: Vec<u32> = (0..2_500_u32)
        .map(|value| value.wrapping_mul(2_654_435_761))
        .collect();
    let text = meta::encode_print(&print);
    assert_eq!(meta::decode_print(&text).expect("декод"), print);
    // Мусор не должен выглядеть печатью.
    assert!(meta::decode_print("!!!").is_none());
    assert_eq!(meta::encode_print(&[]), "");
    let _ = identity::MIN_AGREEMENT;
}
