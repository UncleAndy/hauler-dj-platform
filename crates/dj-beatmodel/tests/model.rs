//! Ресемплер и вывод уровня из долей — без модели и без ONNX Runtime.

#![allow(clippy::expect_used)]

use dj_beatmodel::{prior_from_beats, resample};

#[test]
fn a_sine_keeps_its_frequency_after_resampling() {
    let from = 48_000.0;
    let to = 22_050.0;
    let seconds = 1.0;
    let samples: Vec<f32> = (0..(from * seconds) as usize)
        .map(|index| (0.5 * (std::f64::consts::TAU * 1000.0 * index as f64 / from).sin()) as f32)
        .collect();
    let out = resample(&samples, from, to);
    assert!(
        (out.len() as f64 - to * seconds).abs() < 2.0,
        "длина {} при {} Гц",
        out.len(),
        to
    );
    // считаем переходы через ноль: их должно быть примерно 2·f за секунду
    let mut crossings = 0usize;
    for pair in out.windows(2) {
        if (pair[0] < 0.0) != (pair[1] < 0.0) {
            crossings += 1;
        }
    }
    let measured = crossings as f64 / 2.0;
    assert!(
        (measured - 1000.0).abs() < 30.0,
        "частота после пересчёта {measured:.0} Гц"
    );
}

#[test]
fn a_dc_level_survives_the_filter() {
    let samples = vec![0.25f32; 48_000];
    let out = resample(&samples, 48_000.0, 22_050.0);
    // края фильтра не в счёт: смотрим середину
    let middle = &out[2000..out.len() - 2000];
    let mean: f32 = middle.iter().sum::<f32>() / middle.len() as f32;
    assert!(
        (mean - 0.25).abs() < 0.01,
        "постоянная составляющая {mean:.4}"
    );
}

#[test]
fn the_prior_reads_a_regular_beat_train() {
    // 128 BPM: доля каждые 0,46875 с, начиная с 0,1 с
    let beats: Vec<f64> = (0..64)
        .map(|index| 0.1 + index as f64 * 60.0 / 128.0)
        .collect();
    let prior = prior_from_beats(&beats).expect("годный ряд");
    assert!((prior.bpm - 128.0).abs() < 1.0, "темп {}", prior.bpm);
    assert!((prior.anchor - 0.1).abs() < 1e-6);
}

#[test]
fn too_few_beats_give_no_prior() {
    assert!(prior_from_beats(&[]).is_none());
    assert!(prior_from_beats(&[0.5, 1.0, 1.5]).is_none());
}
