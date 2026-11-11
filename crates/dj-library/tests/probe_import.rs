//! Импорт на реальных файлах: куда легло, как назвалось, что починилось, сколько стоило.
//!
//! Проба ничего не меняет в источнике: она копирует файлы в отдельный каталог коллекции.
//!
//! ```text
//! DJ_IMPORT_FROM=/path/to/music DJ_IMPORT_TO=/path/to/collection \
//!   DJ_IMPORT_LIMIT=40 DJ_IMPORT_THREADS=4 \
//!   cargo test -p dj-library --test probe_import -- --nocapture
//! ```
//!
//! `DJ_IMPORT_LIMIT` — сколько файлов взять (0 или не задано — все), `DJ_IMPORT_THREADS` — сколько
//! работников у очереди (0 или не задано — по числу ядер).

#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dj_library::identity::Catalog;
use dj_library::queue::{Run, threads_for};

const AUDIO: [&str; 4] = ["flac", "mp3", "wav", "ogg"];

fn collect(root: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| AUDIO.contains(&extension.to_lowercase().as_str()))
            .unwrap_or(false)
        {
            found.push(path);
        }
    }
}

#[test]
fn probe_import() {
    let Ok(from) = std::env::var("DJ_IMPORT_FROM") else {
        println!("PROBE skipped: DJ_IMPORT_FROM не задан");
        return;
    };
    let to = std::env::var("DJ_IMPORT_TO").expect("DJ_IMPORT_TO");
    let limit = std::env::var("DJ_IMPORT_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let requested = std::env::var("DJ_IMPORT_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let from = PathBuf::from(from);
    let to = PathBuf::from(to);

    let mut files = Vec::new();
    collect(&from, &mut files);
    files.sort();
    if limit > 0 {
        files.truncate(limit);
    }
    println!(
        "PROBE: {} файлов из {} | потоков {}",
        files.len(),
        from.display(),
        threads_for(requested)
    );

    let check = std::env::var("DJ_IMPORT_CHECK").is_ok_and(|value| value == "1");
    let started = Instant::now();
    let run = if check {
        let catalog = Catalog::scan(&to);
        println!(
            "PROBE: в коллекции уже {} треков с отпечатком",
            catalog.len()
        );
        Run::start_checked(&to, files.clone(), requested, 1_000, catalog)
    } else {
        Run::start(&to, files.clone(), requested, 1_000)
    };
    let mut ticks = 0_u32;
    let mut last_share = -1.0_f64;
    while run.progress().running {
        std::thread::sleep(Duration::from_millis(250));
        let progress = run.progress();
        let share = (progress.share() * 100.0).round();
        if share > last_share {
            last_share = share;
            ticks += 1;
            println!(
                "PROGRESS {}/{} ({share}%) | {}",
                progress.finished(),
                progress.total,
                progress
                    .current
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            );
        }
    }
    let report = run.join();
    let elapsed = started.elapsed().as_secs_f64();

    for imported in &report.imported {
        println!(
            "IMPORT {} -> {} | теги «{}» — «{}» | починено {} | имя из файла {}",
            imported
                .path
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            imported
                .path
                .strip_prefix(&to)
                .unwrap_or(&imported.path)
                .display(),
            imported.tags.artist,
            imported.tags.title,
            imported.repaired,
            imported.named_from_file,
        );
    }
    for (path, reason) in &report.failures {
        println!("IMPORT FAILED {} | {reason}", path.display());
    }
    let repaired = report.imported.iter().filter(|item| item.repaired).count();
    let named = report
        .imported
        .iter()
        .filter(|item| item.named_from_file)
        .count();
    for duplicate in &report.duplicates {
        println!(
            "DUPLICATE {} | уже есть {} | совпадение {:.4}",
            duplicate
                .source
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            duplicate
                .existing
                .strip_prefix(&to)
                .unwrap_or(&duplicate.existing)
                .display(),
            duplicate.agreement
        );
    }
    for version in &report.versions {
        println!(
            "VERSION {} | другая версия {} | совпадение {:.4}",
            version
                .source
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            version
                .existing
                .strip_prefix(&to)
                .unwrap_or(&version.existing)
                .display(),
            version.agreement
        );
    }
    println!(
        "PROBE summary: файлов {} | легло {} | дублей {} | версий {} | починено кодировок {repaired} | имя из файла {named} | отказов {} | отменено {} | {elapsed:.1} с на всё ({:.1} мс на файл) | обновлений прогресса {ticks}",
        files.len(),
        report.imported.len(),
        report.duplicates.len(),
        report.versions.len(),
        report.failures.len(),
        report.cancelled,
        elapsed * 1_000.0 / files.len().max(1) as f64
    );
}
