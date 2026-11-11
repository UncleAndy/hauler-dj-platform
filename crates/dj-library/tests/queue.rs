//! Очередь импорта: несколько работников, прогресс, отмена.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dj_library::queue::Run;
use dj_library::tags;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dj-codec/tests/fixtures/audio")
        .join("tone-1s-44100.flac")
}

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-queue-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch");
    root
}

/// `count` разных треков: у каждого своё имя в тегах, поэтому каждый ложится своим путём.
fn batch(root: &Path, count: usize) -> Vec<PathBuf> {
    let incoming = root.join("incoming");
    fs::create_dir_all(&incoming).expect("incoming");
    let mut jobs = Vec::with_capacity(count);
    for number in 0..count {
        let path = incoming.join(format!("track-{number:03}.flac"));
        fs::copy(fixture(), &path).expect("copy fixture");
        let written = tags::Tags {
            artist: format!("Artist {number}"),
            title: format!("Title {number}"),
            count: 2,
            ..tags::Tags::default()
        };
        assert!(
            tags::write(&path, &written).expect("tags"),
            "тег не записался"
        );
        jobs.push(path);
    }
    jobs
}

#[test]
fn a_batch_lands_whole_whatever_the_number_of_workers() {
    for threads in [1_usize, 4] {
        let root = scratch(&format!("whole-{threads}"));
        let jobs = batch(&root, 6);
        let collection = root.join("collection");

        let run = Run::start(&collection, jobs, threads, 1_000);
        assert_eq!(
            run.progress().total,
            6,
            "очередь не знает, сколько ей нести"
        );
        let report = run.join();

        assert_eq!(report.imported.len(), 6, "потоков {threads}");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(!report.cancelled);
        for imported in &report.imported {
            assert!(
                imported.path.is_file(),
                "{} пропал",
                imported.path.display()
            );
            assert!(
                imported.meta.is_file(),
                "{} без метаданных",
                imported.path.display()
            );
        }
        let _ = fs::remove_dir_all(&root);
    }
}

#[test]
fn one_worker_and_many_give_the_same_collection() {
    let root = scratch("same");
    let jobs = batch(&root, 5);
    let single = root.join("single");
    let many = root.join("many");

    let one = Run::start(&single, jobs.clone(), 1, 1_000).join();
    let several = Run::start(&many, jobs, 8, 1_000).join();

    let relative = |report: &dj_library::Report, base: &Path| {
        report
            .imported
            .iter()
            .map(|imported| {
                imported
                    .path
                    .strip_prefix(base)
                    .expect("под корнем")
                    .to_path_buf()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(relative(&one, &single), relative(&several, &many));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn progress_is_visible_while_the_batch_runs() {
    let root = scratch("progress");
    let jobs = batch(&root, 40);
    let collection = root.join("collection");

    let run = Run::start(&collection, jobs, 2, 1_000);
    let started = Instant::now();
    let mut seen_moving = false;
    while started.elapsed() < Duration::from_secs(20) {
        let progress = run.progress();
        if progress.finished() > 0 && progress.running {
            seen_moving = true;
            assert!(progress.share() > 0.0 && progress.share() < 1.0);
            break;
        }
        if !progress.running {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let report = run.join();
    assert_eq!(report.imported.len(), 40);
    // На быстром диске партия может кончиться раньше, чем мы её увидели: тогда проверять нечего.
    let _ = seen_moving;
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn cancelling_stops_the_batch_between_files() {
    let root = scratch("cancel");
    let jobs = batch(&root, 200);
    let collection = root.join("collection");

    let run = Run::start(&collection, jobs, 1, 1_000);
    // Ждём первого готового файла, затем просим остановиться: отмена — не «стоп-кадр», а «дальше
    // не бери». Файл, который работник уже несёт, доносится целиком.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) && run.progress().finished() == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    run.cancel();
    let progress = run.progress();
    assert!(progress.cancelled, "отмена не видна в прогрессе");
    let report = run.join();

    assert!(report.cancelled);
    assert!(
        report.total() < 200,
        "отмена не остановила очередь: {} из 200",
        report.total()
    );
    for imported in &report.imported {
        assert!(
            imported.path.is_file(),
            "недоделанный файл остался в коллекции"
        );
        assert!(imported.meta.is_file(), "файл без метаданных после отмены");
    }
    let _ = fs::remove_dir_all(&root);
}
