//! Импорт на реальных файлах: куда легло, как назвалось, что починилось, сколько стоило.
//!
//! Проба ничего не меняет в источнике: она копирует файлы в отдельный каталог коллекции.
//!
//! ```text
//! DJ_IMPORT_FROM=/path/to/music DJ_IMPORT_TO=/path/to/collection DJ_IMPORT_LIMIT=40 \
//!   cargo test -p dj-library --test probe_import -- --nocapture
//! ```
//!
//! `DJ_IMPORT_LIMIT` — сколько файлов взять (0 или не задано — все).

#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use dj_library::import;

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
    let from = PathBuf::from(from);
    let to = PathBuf::from(to);

    let mut files = Vec::new();
    collect(&from, &mut files);
    files.sort();
    if limit > 0 {
        files.truncate(limit);
    }
    println!("PROBE: {} файлов из {}", files.len(), from.display());

    let started = Instant::now();
    let mut repaired = 0;
    let mut named_from_file = 0;
    let mut failed = 0;
    let mut per_file = Vec::new();
    for (number, source) in files.iter().enumerate() {
        let file_started = Instant::now();
        match import(source, &to, 1_000 + number as u64 * 1_000) {
            Ok(imported) => {
                if imported.repaired {
                    repaired += 1;
                }
                if imported.named_from_file {
                    named_from_file += 1;
                }
                let ms = file_started.elapsed().as_millis();
                per_file.push(ms);
                println!(
                    "IMPORT {} -> {} | теги «{}» — «{}» | починено {} | имя из файла {} | {ms} мс",
                    source.display(),
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
            Err(error) => {
                failed += 1;
                println!("IMPORT FAILED {} | {error}", source.display());
            }
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    per_file.sort_unstable();
    let median = per_file.get(per_file.len() / 2).copied().unwrap_or(0);
    let worst = per_file.last().copied().unwrap_or(0);
    println!(
        "PROBE summary: файлов {} | починено кодировок {repaired} | имя из файла {named_from_file} | отказов {failed} | {elapsed:.1} с на всё ({:.1} мс на файл, медиана {median} мс, худший {worst} мс)",
        files.len(),
        elapsed * 1_000.0 / files.len().max(1) as f64
    );
}
