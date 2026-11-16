//! Сессия ONNX и доли по активации: аудио → мел → модель → DBN → уровень и якорь.

use std::path::Path;

use crate::dbn::{DbnConfig, decode_beats};
use crate::mel::{MelConfig, log_mel};
use crate::resample::resample;

/// Кадров в куске, который ждёт модель.
const CHUNK: usize = 1500;
/// Сколько кадров по краям куска считается ненадёжными и перекрывается соседним.
const BORDER: usize = 6;

/// Что модель говорит о треке: темп доли и момент одной из долей.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prior {
    /// Темп доли, BPM.
    pub bpm: f64,
    /// Момент доли от начала файла, секунды.
    pub anchor: f64,
}

/// Что пошло не так при загрузке или исполнении модели.
#[derive(Debug)]
pub enum ModelError {
    /// Не загрузилась библиотека ONNX Runtime или файл модели.
    Load(String),
    /// Модель отказалась считать.
    Run(String),
    /// Ответ модели не той формы, что ждёт код.
    Shape(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::Load(reason) => write!(formatter, "модель не загрузилась: {reason}"),
            ModelError::Run(reason) => write!(formatter, "модель не посчитала: {reason}"),
            ModelError::Shape(reason) => write!(formatter, "ответ модели: {reason}"),
        }
    }
}

impl std::error::Error for ModelError {}

/// Загруженная модель удара.
pub struct BeatModel {
    /// Окружение держим живым: сессия создана из него, и оно обязано пережить её.
    _environment: ort::environment::Environment,
    session: ort::session::Session,
}

impl BeatModel {
    /// Загрузить модель: `library` — путь к `libonnxruntime.so`, `model` — путь к `.onnx`.
    pub fn load(library: &Path, model: &Path) -> Result<Self, ModelError> {
        let environment = ort::init_from(library)
            .map_err(|error| ModelError::Load(error.to_string()))?
            .build()
            .map_err(|error| ModelError::Load(error.to_string()))?;
        let session = ort::session::Session::builder(&environment)
            .map_err(|error| ModelError::Load(error.to_string()))?
            .commit_from_file(model)
            .map_err(|error| ModelError::Load(error.to_string()))?;
        Ok(BeatModel {
            _environment: environment,
            session,
        })
    }

    /// Активации доли и сильной доли по спектрограмме (кадры × полосы).
    fn run(&mut self, spect: &[f32], frames: usize) -> Result<(Vec<f32>, Vec<f32>), ModelError> {
        let input = ort::value::Tensor::from_array(([1usize, frames, 128usize], spect.to_vec()))
            .map_err(|error| ModelError::Run(error.to_string()))?;
        let outputs = self
            .session
            .run(ort::inputs!["spectrogram" => input])
            .map_err(|error| ModelError::Run(error.to_string()))?;
        let mut beat = Vec::with_capacity(frames);
        let mut downbeat = Vec::with_capacity(frames);
        for (name, value) in outputs.iter() {
            let (shape, data) = value
                .try_extract_tensor::<f32>()
                .map_err(|error| ModelError::Shape(error.to_string()))?;
            let plain: Vec<f32> = data.iter().map(|value| sigmoid(*value)).collect();
            if plain.len() != frames {
                return Err(ModelError::Shape(format!(
                    "{name}: {} значений вместо {frames} ({shape:?})",
                    plain.len()
                )));
            }
            if name == "beat" {
                beat = plain;
            } else {
                downbeat = plain;
            }
        }
        if beat.is_empty() {
            return Err(ModelError::Shape("нет выхода beat".to_string()));
        }
        Ok((beat, downbeat))
    }

    /// Активации доли на всём треке: куски по 1500 кадров, перекрытия усредняются.
    pub fn activations(&mut self, mono: &[f32], rate: f64) -> Result<Vec<f32>, ModelError> {
        let config = MelConfig::default();
        let signal = resample(mono, rate, config.rate);
        let spect = log_mel(&signal, &config);
        let total = spect.len() / config.n_mels;
        if total == 0 {
            return Err(ModelError::Run("пустой сигнал".to_string()));
        }
        let mut beat = vec![0.0f32; total];
        let mut weight = vec![0.0f32; total];
        let step = CHUNK - 2 * BORDER;
        let mut start = 0usize;
        while start < total {
            let first = start.saturating_sub(BORDER);
            let last = (first + CHUNK).min(total);
            let frames = last - first;
            let mut chunk = Vec::with_capacity(frames * config.n_mels);
            for frame in first..last {
                chunk.extend_from_slice(&spect[frame * config.n_mels..(frame + 1) * config.n_mels]);
            }
            let (chunk_beat, _chunk_downbeat) = self.run(&chunk, frames)?;
            for (offset, value) in chunk_beat.iter().enumerate() {
                let frame = first + offset;
                beat[frame] += value;
                weight[frame] += 1.0;
            }
            if last >= total {
                break;
            }
            start += step;
        }
        for (value, count) in beat.iter_mut().zip(weight.iter()) {
            if *count > 0.0 {
                *value /= count;
            }
        }
        Ok(beat)
    }
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value).exp())
}

/// Доли трека по модели: мел → модель → DBN, без обращения к нашему детектору.
///
/// Это сырой материал для сетки: декодер идёт по активациям и потому видит местные изменения темпа,
/// которые глобальная гребёнка сглаживает.
pub fn beats(
    model: &mut BeatModel,
    mono: &[f32],
    rate: f64,
    config: &DbnConfig,
) -> Result<Vec<f64>, ModelError> {
    let activation = model.activations(mono, rate)?;
    Ok(decode_beats(&activation, config))
}

/// Уровень и доли за один проход: активации модели считаются один раз.
///
/// [`prior`] и [`beats`] по отдельности — это два прохода по файлу, а сетка по долям нужна ровно
/// тогда же, когда уровень: девять секунд на трек превращаются в четыре.
pub fn prior_and_beats(
    model: &mut BeatModel,
    mono: &[f32],
    rate: f64,
    config: &DbnConfig,
) -> Result<(Option<Prior>, Vec<f64>), ModelError> {
    let activation = model.activations(mono, rate)?;
    let beats = decode_beats(&activation, config);
    let prior = prior_from_beats(&beats);
    Ok((prior, beats))
}

/// Уровень и якорь по аудио: мел → модель → DBN.
pub fn prior(
    model: &mut BeatModel,
    mono: &[f32],
    rate: f64,
    config: &DbnConfig,
) -> Result<Option<Prior>, ModelError> {
    let activation = model.activations(mono, rate)?;
    let beats = decode_beats(&activation, config);
    Ok(prior_from_beats(&beats))
}

/// Уровень и якорь по готовым долям.
pub fn prior_from_beats(beats: &[f64]) -> Option<Prior> {
    if beats.len() < 8 {
        return None;
    }
    // Период и фаза — МНК по первым долям, а не медиана интервалов: декодер шагает целыми кадрами
    // (20 мс), поэтому медиана даёт период с ошибкой до 2 %, а круговая средняя фаза при такой
    // ошибке за 400 с наматывает восемь оборотов и попадает куда угодно. МНК берёт короткий
    // участок, где дрейфа ещё нет, и даёт период с точностью долей процента.
    let count = beats.len().min(120);
    let window = &beats[..count];
    let mean_index = (count as f64 - 1.0) / 2.0;
    let mean_time = window.iter().sum::<f64>() / count as f64;
    let mut numerator = 0.0f64;
    let mut denominator = 0.0f64;
    for (index, beat) in window.iter().enumerate() {
        numerator += (index as f64 - mean_index) * (beat - mean_time);
        denominator += (index as f64 - mean_index).powi(2);
    }
    if denominator <= 0.0 {
        return None;
    }
    let period = numerator / denominator;
    if !(0.2..2.0).contains(&period) {
        return None;
    }
    let anchor = (mean_time - period * mean_index).rem_euclid(period);
    Some(Prior {
        bpm: 60.0 / period,
        anchor,
    })
}
