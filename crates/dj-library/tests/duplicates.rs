//! Дубли при добавлении: та же запись, другое издание, другой трек, судьба проигравшего.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use dj_library::identity::{self, Catalog, Identity, Verdict};
use dj_library::import::{DUPLICATES_DIR, Policy};
use dj_library::{Outcome, import_checked, meta, sidecar, tags};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-dups-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch");
    root
}

/// Поиск по каталогу в том виде, в каком его ждёт импорт: замок и владение каталогом — за вызывающим.
fn find_in(catalog: &Catalog) -> impl Fn(&Identity) -> Option<(PathBuf, Identity, Verdict)> + '_ {
    move |identity| {
        catalog
            .find(identity)
            .map(|(path, existing, verdict)| (path.to_path_buf(), existing.clone(), verdict))
    }
}

fn catalogue(path: &Path, identity: &Identity) -> Catalog {
    let mut catalog = Catalog::new();
    catalog.add(path.to_path_buf(), identity.clone());
    catalog
}

/// Мелодия без периода: у ровного тона отпечаток вырожден, а у повторяющейся гаммы сдвиг
/// находится не там, где он есть.
fn write_tone_from(path: &Path, seconds: u32, frequency: f64, skip: u32, rate: u32) {
    const PITCHES: [f64; 12] = [
        1.0, 1.122, 1.26, 1.335, 1.498, 1.682, 1.782, 2.0, 2.245, 2.52, 2.67, 2.997,
    ];
    let samples = rate * seconds;
    let mut data = Vec::with_capacity(samples as usize * 2);
    let mut phase = 0.0_f64;
    let mut noise = 0x2545_F491_4F6C_DD1D_u64;
    let mut step_frequency = frequency;
    for index in 0..samples {
        let absolute = index + rate * skip;
        if absolute.is_multiple_of(rate / 4) {
            noise = noise
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            step_frequency = frequency * PITCHES[(noise >> 33) as usize % PITCHES.len()];
        }
        phase += std::f64::consts::TAU * step_frequency / f64::from(rate);
        let value = (phase.sin() * 12_000.0) as i16;
        data.extend_from_slice(&value.to_le_bytes());
    }
    write_wav(path, rate, &data);
}

/// Тот же звук, но файл больше: `JUNK` перед данными. Звук не меняется, средний битрейт растёт —
/// так проверяется политика «оставить лучшую копию», не трогая сам звук.
fn write_wav_padded(path: &Path, rate: u32, data: &[u8], junk: usize) {
    let mut wav = Vec::with_capacity(data.len() + junk + 60);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((data.len() + junk + 36) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    if junk > 0 {
        wav.extend_from_slice(b"JUNK");
        wav.extend_from_slice(&(junk as u32).to_le_bytes());
        wav.extend(std::iter::repeat_n(0_u8, junk));
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(data);
    fs::write(path, wav).expect("wav");
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

/// Все звуковые файлы под каталогом: коллекция считается файлами, а не записями в каталоге.
fn wav_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(wav_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "wav") {
            found.push(path);
        }
    }
    found
}

/// Тон с добивкой: тот же звук, но файл тяжелее — «лучшая копия» по среднему битрейту.
fn padded(root: &Path, name: &str, seconds: u32, frequency: f64, junk: usize) -> PathBuf {
    const RATE: u32 = 44_100;
    let path = root.join("incoming").join(name);
    fs::create_dir_all(path.parent().expect("parent")).expect("incoming");
    let mut data = Vec::with_capacity(RATE as usize * seconds as usize * 2);
    let mut phase = 0.0_f64;
    let mut noise = 0x2545_F491_4F6C_DD1D_u64;
    let mut step = frequency;
    for index in 0..RATE * seconds {
        if index.is_multiple_of(RATE / 4) {
            noise = noise
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            step = frequency * (1.0 + (noise >> 33) as f64 / 1.0e10);
        }
        phase += std::f64::consts::TAU * step / f64::from(RATE);
        data.extend_from_slice(&((phase.sin() * 12_000.0) as i16).to_le_bytes());
    }
    write_wav_padded(&path, RATE, &data, junk);
    // Каждая копия несёт своё название: теги WAV теперь читаются, и одинаковое название увело бы
    // обе копии в один файл — тогда «замена» проверяла бы не то.
    let stem = name.strip_suffix(".wav").unwrap_or(name);
    let title = stem.strip_prefix("A - ").unwrap_or(stem);
    let written = tags::Tags {
        artist: "A".to_owned(),
        title: title.to_owned(),
        count: 2,
        ..tags::Tags::default()
    };
    let _ = tags::write(&path, &written);
    path
}

fn tagged(
    root: &Path,
    name: &str,
    artist: &str,
    title: &str,
    seconds: u32,
    frequency: f64,
    rate: u32,
) -> PathBuf {
    let path = root.join("incoming").join(name);
    fs::create_dir_all(path.parent().expect("parent")).expect("incoming");
    write_tone_from(&path, seconds, frequency, 0, rate);
    let written = tags::Tags {
        artist: artist.to_owned(),
        title: title.to_owned(),
        count: 2,
        ..tags::Tags::default()
    };
    let _ = tags::write(&path, &written);
    path
}

fn tone(root: &Path, name: &str, seconds: u32, frequency: f64) -> PathBuf {
    tagged(root, name, "A", "Track", seconds, frequency, 44_100)
}

#[test]
fn the_same_file_twice_is_a_duplicate_and_is_not_copied() {
    let root = scratch("same");
    let collection = root.join("collection");
    let source = tone(&root, "A - Track.wav", 30, 220.0);

    let first = import_checked(
        &source,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::Ask,
    )
    .expect("первый импорт");
    let Outcome::Placed(placed) = first else {
        panic!("первый файл не лёг: {first:?}");
    };
    let catalog = catalogue(&placed.path, &placed.identity);

    let second = import_checked(&source, &collection, 2_000, find_in(&catalog), Policy::Ask)
        .expect("второй импорт");
    match second {
        Outcome::Duplicate {
            existing, verdict, ..
        } => {
            assert_eq!(existing, placed.path);
            assert!(matches!(verdict, Verdict::Duplicate { .. }), "{verdict:?}");
        }
        other => panic!("дубль не распознан: {other:?}"),
    }
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
    let full = tone(&root, "A - Track.wav", 60, 220.0);
    let short = tone(&root, "A - Track (short tail).wav", 59, 220.0);

    let Outcome::Placed(placed) = import_checked(
        &full,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::Ask,
    )
    .expect("полный") else {
        panic!("полный не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second = import_checked(&short, &collection, 2_000, find_in(&catalog), Policy::Ask)
        .expect("обрезанный");
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
        44_100,
    );
    let extended = tagged(
        &root,
        "A - Bones (Extended Mix).wav",
        "A",
        "Bones (Extended Mix)",
        30,
        220.0,
        44_100,
    );

    let Outcome::Placed(placed) = import_checked(
        &dub,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::Ask,
    )
    .expect("даб") else {
        panic!("даб не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second = import_checked(
        &extended,
        &collection,
        2_000,
        find_in(&catalog),
        Policy::Ask,
    )
    .expect("extended");
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
    let first = tagged(&root, "A - One.wav", "A", "One", 30, 220.0, 44_100);
    let other = tagged(&root, "B - Two.wav", "B", "Two", 30, 660.0, 44_100);

    let Outcome::Placed(placed) = import_checked(
        &first,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::Ask,
    )
    .expect("первый") else {
        panic!("первый не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second =
        import_checked(&other, &collection, 2_000, find_in(&catalog), Policy::Ask).expect("второй");
    assert!(matches!(second, Outcome::Placed(_)), "{second:?}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_chromaprint_travels_in_the_metadata_file() {
    let root = scratch("print");
    let collection = root.join("collection");
    let source = tone(&root, "A - Track.wav", 30, 220.0);

    let Outcome::Placed(placed) = import_checked(
        &source,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::Ask,
    )
    .expect("импорт") else {
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
    assert!(meta::decode_print("!!!").is_none());
    assert_eq!(meta::encode_print(&[]), "");
    let _ = identity::MIN_AGREEMENT;
}

#[test]
fn the_better_copy_replaces_the_one_in_the_collection() {
    let root = scratch("better");
    let collection = root.join("collection");
    // Один и тот же звук: у второго файла добивка, средний битрейт выше — он и лучше.
    let low = padded(&root, "A - Track light.wav", 30, 220.0, 0);
    let high = padded(&root, "A - Track heavy.wav", 30, 220.0, 400_000);

    let Outcome::Placed(placed) = import_checked(
        &low,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::ReplaceIfBetter,
    )
    .expect("низкий") else {
        panic!("низкий не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second = import_checked(
        &high,
        &collection,
        2_000,
        find_in(&catalog),
        Policy::ReplaceIfBetter,
    )
    .expect("высокий");
    let Outcome::Replaced {
        imported, rejected, ..
    } = second
    else {
        panic!("замена не произошла: {second:?}");
    };
    assert!(imported.path.is_file(), "новый файл не лёг");
    assert!(
        rejected.starts_with(collection.join(DUPLICATES_DIR)),
        "проигравший не в _duplicates: {}",
        rejected.display()
    );
    assert!(rejected.is_file(), "проигравший пропал");
    assert!(!placed.path.exists(), "старый файл остался в коллекции");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_worse_copy_goes_to_duplicates_and_the_collection_stays() {
    let root = scratch("worse");
    let collection = root.join("collection");
    let low = padded(&root, "A - Track light.wav", 30, 220.0, 0);
    let high = padded(&root, "A - Track heavy.wav", 30, 220.0, 400_000);

    let Outcome::Placed(placed) = import_checked(
        &high,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::ReplaceIfBetter,
    )
    .expect("высокий") else {
        panic!("высокий не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second = import_checked(
        &low,
        &collection,
        2_000,
        find_in(&catalog),
        Policy::ReplaceIfBetter,
    )
    .expect("низкий");
    let Outcome::Rejected {
        existing, stored, ..
    } = second
    else {
        panic!("отказ не произошёл: {second:?}");
    };
    assert_eq!(existing, placed.path);
    assert!(stored.starts_with(collection.join(DUPLICATES_DIR)));
    assert!(stored.is_file(), "отклонённый файл не сохранён");
    assert!(placed.path.is_file(), "победитель пострадал");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn replacing_a_worse_copy_is_the_djs_choice() {
    // The numbers say the copy in the collection is better. The DJ was shown both and said "replace":
    // the import obeys the hand, not the measurement, and the old copy is kept in `_duplicates/`.
    let root = scratch("replace-choice");
    let collection = root.join("collection");
    let better = tagged(&root, "A - Track 44k.wav", "A", "Track", 30, 220.0, 44_100);
    // The same melody at half the rate: the same recording, and the worse copy by the numbers.
    let worse = tagged(&root, "A - Track 22k.wav", "A", "Track", 30, 220.0, 22_050);

    let Outcome::Placed(placed) = import_checked(
        &better,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::Ask,
    )
    .expect("первый") else {
        panic!("первый не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let outcome = import_checked(
        &worse,
        &collection,
        2_000,
        find_in(&catalog),
        Policy::Replace,
    )
    .expect("замена");
    match outcome {
        Outcome::Replaced {
            imported, rejected, ..
        } => {
            assert!(imported.path.exists(), "нового файла нет в коллекции");
            assert!(
                !rejected.exists() || rejected.starts_with(collection.join("_duplicates")),
                "старая копия не уехала в _duplicates: {}",
                rejected.display()
            );
            // У обеих копий одно название, поэтому новая встаёт ровно туда, где стояла старая: пропасть
            // должны байты старой, а не путь. В коллекции остаётся одна копия.
            let left: Vec<_> = wav_files(&collection)
                .into_iter()
                .filter(|path| !path.starts_with(collection.join(DUPLICATES_DIR)))
                .collect();
            assert_eq!(
                left,
                vec![imported.path.clone()],
                "в коллекции должна остаться одна копия, а не две"
            );
        }
        other => panic!("замена не произошла: {other:?}"),
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn keeping_both_marks_the_second_copy_as_a_variant() {
    let root = scratch("both");
    let collection = root.join("collection");
    let source = tone(&root, "A - Track.wav", 30, 220.0);

    let Outcome::Placed(placed) = import_checked(
        &source,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::KeepBoth,
    )
    .expect("первый") else {
        panic!("первый не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second = import_checked(
        &source,
        &collection,
        2_000,
        find_in(&catalog),
        Policy::KeepBoth,
    )
    .expect("второй");
    let Outcome::Placed(again) = second else {
        panic!("оба не сохранены: {second:?}");
    };
    assert!(
        placed.path.is_file() && again.path.is_file(),
        "один из двух пропал"
    );
    assert_ne!(placed.path, again.path);
    assert_eq!(again.variant.as_deref(), Some("2"));
    let record = sidecar::read(&again.meta).expect("метаданные варианта");
    assert_eq!(record.variant.as_deref(), Some("2"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_mono_file_is_measured_whole() {
    // Декодер всегда пишет стерео и дублирует моно в оба канала. Если верить числу каналов файла,
    // читается половина каждого куска, и трек измеряется вдвое короче — а его же копия перестаёт
    // быть копией.
    let root = scratch("mono");
    let source = tone(&root, "A - Track.wav", 30, 220.0);
    let measured = identity::of_file(&source, "Track").expect("измерение");
    assert_eq!(
        measured.duration_ms, 30_000,
        "длительность моно-файла неверна"
    );
    assert_eq!(measured.sample_rate, 44_100);
}

#[test]
fn a_copy_at_another_sample_rate_is_still_the_same_recording() {
    let root = scratch("rate");
    let collection = root.join("collection");
    let high = tagged(&root, "A - Track 44k.wav", "A", "Track", 30, 220.0, 44_100);
    let low = tagged(&root, "A - Track 22k.wav", "A", "Track", 30, 220.0, 22_050);

    let Outcome::Placed(placed) = import_checked(
        &high,
        &collection,
        1_000,
        find_in(&Catalog::new()),
        Policy::Ask,
    )
    .expect("высокий") else {
        panic!("высокий не лёг");
    };
    let catalog = catalogue(&placed.path, &placed.identity);
    let second =
        import_checked(&low, &collection, 2_000, find_in(&catalog), Policy::Ask).expect("низкий");
    match second {
        Outcome::Duplicate { verdict, .. } => {
            assert!(matches!(verdict, Verdict::Duplicate { .. }), "{verdict:?}");
        }
        other => panic!("копия на другой частоте не распознана: {other:?}"),
    }
    let _ = fs::remove_dir_all(&root);
}
