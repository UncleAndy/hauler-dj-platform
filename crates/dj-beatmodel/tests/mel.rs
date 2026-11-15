//! Фронтенд: форма, полосы и то, что синус попадает в свою полосу.

#![allow(clippy::expect_used)]

use dj_beatmodel::{MelConfig, log_mel, mel_filterbank};

#[test]
fn the_filterbank_has_the_shape_the_model_declares() {
    let config = MelConfig::default();
    let filters = mel_filterbank(&config);
    assert_eq!(filters.len(), config.n_mels * config.bins());
    assert_eq!(config.bins(), 513, "1024 / 2 + 1");
    for value in &filters {
        assert!(*value >= 0.0, "вес фильтра не может быть отрицательным");
    }
    // у каждой полосы есть вес, и полосы идут по возрастанию частоты
    let peaks: Vec<usize> = (0..config.n_mels)
        .map(|band| {
            let row = &filters[band * config.bins()..(band + 1) * config.bins()];
            let (index, _) = row
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).expect("вес"))
                .expect("полоса не пуста");
            index
        })
        .collect();
    for pair in peaks.windows(2) {
        assert!(pair[1] > pair[0], "полосы идут по возрастанию: {peaks:?}");
    }
    assert!(peaks[0] < 50, "нижняя полоса у 30 Гц: {peaks:?}");
    assert!(
        peaks[config.n_mels - 1] > 400,
        "верхняя у 11 кГц: {peaks:?}"
    );
}

#[test]
fn frames_follow_the_hop() {
    let config = MelConfig::default();
    assert_eq!(config.frames(30 * 22_050), 1501);
    assert_eq!(config.frames(0), 1);
}

#[test]
fn a_sine_lands_in_the_band_of_its_frequency() {
    let config = MelConfig::default();
    let rate = config.rate;
    let seconds = 1.0;
    let samples: Vec<f32> = (0..(rate * seconds) as usize)
        .map(|index| {
            let phase = std::f64::consts::TAU * 1000.0 * index as f64 / rate;
            (0.5 * phase.sin()) as f32
        })
        .collect();
    let spect = log_mel(&samples, &config);
    let frames = spect.len() / config.n_mels;
    assert!(frames > 10);
    // средняя по кадрам энергия полос: у синуса 1 кГц она выше всего в своей полосе
    let mut best = (0usize, 0.0f32);
    for band in 0..config.n_mels {
        let sum: f32 = (0..frames)
            .map(|frame| spect[frame * config.n_mels + band])
            .sum();
        let mean = sum / frames as f32;
        if mean > best.1 {
            best = (band, mean);
        }
    }
    let band_hz = best.0 as f64 * config.rate / config.n_fft as f64;
    assert!(
        (600.0..1600.0).contains(&band_hz),
        "полоса {} — это {band_hz:.0} Гц, а синус 1000 Гц",
        best.0
    );
}

#[test]
fn silence_gives_zero_after_the_logarithm() {
    let config = MelConfig::default();
    let samples = vec![0.0f32; 22_050];
    for value in log_mel(&samples, &config) {
        assert_eq!(value, 0.0, "ln(1 + 1000·0) = 0");
    }
}
