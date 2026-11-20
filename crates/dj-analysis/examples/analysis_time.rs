//! Сколько стоит замер одного файла: декодирование, детектор темпа, детектор ключа.
//!
//! Запуск: `cargo run --release -p dj-analysis --example analysis_time -- /path/to/track.flac`
//!
//! Батч замера состоит из двух полных декодирований и двух проходов по отсчётам, и на длинном lossless-файле
//! это минуты. Цифры по стадиям нужны, чтобы решение «мерить окно или весь файл» принималось по ним.

use std::path::Path;
use std::time::Instant;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("нужен путь к файлу");
        std::process::exit(2);
    };
    let path = Path::new(&path);

    let started = Instant::now();
    match dj_analysis::tempo::decode_mono(path) {
        Ok((mono, rate)) => {
            let seconds = mono.len() as f64 / rate;
            println!(
                "decode: {} ms, {:.1} s at {:.0} Hz ({} samples)",
                started.elapsed().as_millis(),
                seconds,
                rate,
                mono.len()
            );
            let detected = Instant::now();
            let tempo =
                dj_analysis::tempo::detect_with_prior_and_beats(&mono, rate, None, Some(&[]));
            println!(
                "tempo: {} ms -> {}",
                detected.elapsed().as_millis(),
                tempo
                    .map(|found| format!(
                        "{:.2} BPM, {} segment(s)",
                        found.segments.first().map(|s| s.bpm).unwrap_or(0.0),
                        found.segments.len()
                    ))
                    .unwrap_or_else(|| "none".to_string())
            );
        }
        Err(error) => println!("decode failed: {error}"),
    }

    let key_started = Instant::now();
    match dj_analysis::key::analyse(path) {
        Ok(Some(key)) => println!(
            "key: {} ms -> {}",
            key_started.elapsed().as_millis(),
            key.traktor()
        ),
        Ok(None) => println!("key: {} ms -> none", key_started.elapsed().as_millis()),
        Err(error) => println!("key: {} ms -> {error}", key_started.elapsed().as_millis()),
    }
}
