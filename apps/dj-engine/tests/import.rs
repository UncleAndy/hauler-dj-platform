//! The import a DJ starts from the browser: files land, a duplicate is asked about, the answer is obeyed.

#![allow(clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dj_domain::{DuplicateAnswer, ImportState};
use dj_engine::{ImportRequest, StateCells, run_import};

/// A WAV of a melody without a period, so its fingerprint is not degenerate.
fn melody(path: &Path, seconds: u32) {
    melody_at(path, seconds, 220.0);
}

/// The same tune on another root. Two files that differ only in length are the same recording, and a
/// batch of them would be a batch of duplicate questions rather than four jobs.
fn melody_at(path: &Path, seconds: u32, base: f64) {
    const PITCHES: [f64; 12] = [
        1.0, 1.122, 1.26, 1.335, 1.498, 1.682, 1.782, 2.0, 2.245, 2.52, 2.67, 2.997,
    ];
    const RATE: u32 = 44_100;
    let samples = RATE * seconds;
    let mut data = Vec::with_capacity(samples as usize * 2);
    let mut phase = 0.0_f64;
    let mut noise = 0x2545_F491_4F6C_DD1D_u64;
    let mut step = base;
    for index in 0..samples {
        if index.is_multiple_of(RATE / 4) {
            noise = noise
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            step = base * PITCHES[(noise >> 33) as usize % PITCHES.len()];
        }
        phase += std::f64::consts::TAU * step / f64::from(RATE);
        data.extend_from_slice(&((phase.sin() * 12_000.0) as i16).to_le_bytes());
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((data.len() + 36) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).expect("wav");
}

fn scratch(name: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("dj-import-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("каталог");
    base
}

/// Wait for a state that satisfies `ready`, or give up: a batch that never publishes is a hang.
fn wait_for(cells: &StateCells, ready: impl Fn(&ImportState) -> bool) -> ImportState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(state) = cells.import()
            && ready(&state)
        {
            return state;
        }
        assert!(Instant::now() < deadline, "состояние импорта не пришло");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_duplicate_is_asked_about_and_the_answer_is_obeyed() {
    let base = scratch("duplicate");
    let sources = base.join("incoming");
    let collection = base.join("collection");
    std::fs::create_dir_all(&sources).expect("источник");
    std::fs::create_dir_all(&collection).expect("коллекция");
    let first = sources.join("A - Track.wav");
    let second = sources.join("A - Track (copy).wav");
    melody(&first, 20);
    melody(&second, 20);

    let cells = Arc::new(StateCells::new());
    let worker_cells = Arc::clone(&cells);
    let worker_root = collection.display().to_string();
    let (answers, answers_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    let running_flag = Arc::clone(&running);
    let handle = std::thread::spawn(move || {
        run_import(
            ImportRequest {
                paths: vec![first.display().to_string(), second.display().to_string()],
                root: worker_root,
                parallel: true,
            },
            worker_cells,
            answers_rx,
            cancel,
            running_flag,
            Arc::new(|_| {}),
            None,
        );
    });

    // The second file is the same recording: the batch stops and asks.
    let asked = wait_for(&cells, |state| state.question.is_some());
    let question = asked.question.clone().expect("вопрос");
    assert_eq!(asked.total, 2);
    assert!(
        question.agreement > 0.9,
        "совпадение {} ниже порога",
        question.agreement
    );
    // Which of the two is the newcomer depends on the order the batch walks: what matters is that
    // the question names two different files of the same recording.
    assert_ne!(question.incoming.path, question.existing.path);
    assert!(question.incoming.path.contains("Track"));
    assert!(question.existing.path.contains("Track"));
    assert!(!question.incoming.format.is_empty());

    answers
        .send(DuplicateAnswer::AddMarked)
        .expect("ответ доставлен");
    handle.join().expect("работник закончил");

    let done = cells.import().expect("состояние");
    assert!(!done.running);
    assert_eq!(done.total, 2);
    assert_eq!(done.done, 2);
    assert_eq!(done.placed, 2, "обе копии должны лечь: {:?}", done);
    assert_eq!(done.failed, 0);
    assert!(done.question.is_none());
    assert!(!running.load(Ordering::Relaxed));

    let mut landed: Vec<std::path::PathBuf> = walk(&collection);
    landed.sort();
    assert_eq!(landed.len(), 2, "в коллекции {:?}", landed);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_stopped_batch_takes_no_more_files() {
    let base = scratch("cancel");
    let sources = base.join("incoming");
    let collection = base.join("collection");
    std::fs::create_dir_all(&sources).expect("источник");
    std::fs::create_dir_all(&collection).expect("коллекция");
    let mut paths = Vec::new();
    for index in 0..3 {
        let path = sources.join(format!("Track {index}.wav"));
        melody(&path, 20);
        paths.push(path.display().to_string());
    }

    let cells = Arc::new(StateCells::new());
    let worker_cells = Arc::clone(&cells);
    let worker_root = collection.display().to_string();
    let (_answers, answers_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(true));
    let running = Arc::new(AtomicBool::new(true));
    let running_flag = Arc::clone(&running);
    let handle = std::thread::spawn(move || {
        run_import(
            ImportRequest {
                paths,
                root: worker_root,
                parallel: false,
            },
            worker_cells,
            answers_rx,
            cancel,
            running_flag,
            Arc::new(|_| {}),
            None,
        );
    });
    handle.join().expect("работник закончил");

    let done = cells.import().expect("состояние");
    assert!(!done.running);
    assert_eq!(done.done, 0, "остановленная пачка ничего не добавила");
    assert_eq!(done.placed, 0);
    assert!(walk(&collection).is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// Run a batch of four different recordings with the analysis hook counting how many files it is
/// inside at once. `parallel` is the flag the confirmation window sends.
fn peak_analysis_concurrency(parallel: bool) -> usize {
    let base = scratch(if parallel { "parallel" } else { "serial" });
    let sources = base.join("incoming");
    let collection = base.join("collection");
    std::fs::create_dir_all(&sources).expect("источник");
    std::fs::create_dir_all(&collection).expect("коллекция");
    let mut paths = Vec::new();
    for index in 0..4u32 {
        let path = sources.join(format!("Track {index}.wav"));
        // Four different recordings: a batch of copies would ask four questions, and this test is
        // about how many analyses run at once.
        melody_at(&path, 20, 110.0 * f64::from(index + 1));
        paths.push(path.display().to_string());
    }

    let cells = Arc::new(StateCells::new());
    let worker_cells = Arc::clone(&cells);
    let worker_root = collection.display().to_string();
    let (answers, answers_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(true));
    let running_flag = Arc::clone(&running);
    // A question here means the four files were judged the same recording, which is a broken test:
    // answer it so the batch finishes and the assertion below says what happened, instead of hanging.
    let watchdog = {
        let cells = Arc::clone(&cells);
        let answers = answers.clone();
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline && running.load(Ordering::Relaxed) {
                let asking = cells
                    .import()
                    .map(|state| state.question.is_some())
                    .unwrap_or(false);
                if asking {
                    let _ = answers.send(dj_domain::DuplicateAnswer::Skip);
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let hook = {
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        Arc::new(move |_path: &Path| {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(120));
            active.fetch_sub(1, Ordering::SeqCst);
        })
    };
    let handle = std::thread::spawn(move || {
        run_import(
            ImportRequest {
                paths,
                root: worker_root,
                parallel,
            },
            worker_cells,
            answers_rx,
            cancel,
            running_flag,
            hook,
            None,
        );
    });
    handle.join().expect("работник закончил");
    let _ = watchdog.join();
    let done = cells.import().expect("состояние");
    assert_eq!(done.placed, 4, "не все файлы легли: {done:?}");
    assert!(!done.running);
    let peak = peak.load(Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&base);
    peak
}

#[test]
fn parallel_analysis_takes_several_files_at_once() {
    let peak = peak_analysis_concurrency(true);
    assert!(
        peak > 1,
        "с включённой параллельностью файлы анализировались по одному (пик {peak})"
    );
}

#[test]
fn without_parallel_the_analysis_is_one_file_at_a_time() {
    let peak = peak_analysis_concurrency(false);
    assert_eq!(
        peak, 1,
        "без флага анализ шёл в несколько потоков (пик {peak})"
    );
}

/// Every file under a folder, for counting what the collection holds.
fn walk(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(".djmeta") {
                continue;
            }
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}
