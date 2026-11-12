//! What the genre tags in a real collection actually say.
//!
//! The split rules are a guess until they meet real files: this walks a folder, reads the genre tag of
//! every track and prints what `genres::split` makes of it — the raw strings, the number of genres per
//! track, the distinct keys, and the cases the rules refused. Read only; nothing is written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: genre_probe <folder>");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);

    let mut files = 0usize;
    let mut with_genre = 0usize;
    let mut raw_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut keys: BTreeMap<String, usize> = BTreeMap::new();
    let mut multi = 0usize;
    let mut dropped = 0usize;
    let mut over_limit = 0usize;

    walk(&root, &mut |path| {
        files += 1;
        let Ok(tags) = dj_library::tags::read(path) else {
            return;
        };
        let raw = tags.genre.trim().to_owned();
        if raw.is_empty() {
            return;
        }
        with_genre += 1;
        *raw_counts.entry(raw.clone()).or_default() += 1;
        let genres = dj_library::genres::split(&raw);
        if genres.is_empty() {
            dropped += 1;
        }
        if genres.len() > 1 {
            multi += 1;
        }
        if genres.len() >= dj_library::genres::MAX_GENRES {
            over_limit += 1;
        }
        for genre in genres {
            *keys.entry(genre.key).or_default() += 1;
        }
    });

    println!("файлов: {files}, с тегом жанра: {with_genre}");
    println!(
        "разобралось больше чем в один жанр: {multi}; отброшено целиком: {dropped}; упёрлось в предел \
         {MAX}: {over_limit}",
        MAX = dj_library::genres::MAX_GENRES
    );
    println!("разных ключей после разбора: {}", keys.len());

    let mut top: Vec<(&String, &usize)> = raw_counts.iter().collect();
    top.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
    println!("\nсырые строки тега (топ 25):");
    for (raw, count) in top.iter().take(25) {
        let parsed = dj_library::genres::split(raw)
            .into_iter()
            .map(|genre| genre.name)
            .collect::<Vec<_>>()
            .join(" | ");
        println!("  {count:4}  «{raw}»  ->  {parsed}");
    }

    let mut keys_top: Vec<(&String, &usize)> = keys.iter().collect();
    keys_top.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
    println!("\nжанры после разбора (топ 25):");
    for (key, count) in keys_top.iter().take(25) {
        println!("  {count:4}  {key}");
    }
}

fn walk(root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(".djmeta") {
                continue;
            }
            walk(&path, visit);
        } else {
            visit(&path);
        }
    }
}
