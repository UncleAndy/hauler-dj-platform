//! Лог-мел-спектрограмма в том виде, в каком её ждёт модель.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// Параметры фронтенда: контракт модели, а не настройка.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MelConfig {
    /// Частота дискретизации, Гц.
    pub rate: f64,
    /// Длина окна БПФ.
    pub n_fft: usize,
    /// Шаг между кадрами.
    pub hop: usize,
    /// Число мел-полос.
    pub n_mels: usize,
    /// Нижняя граница полос, Гц.
    pub f_min: f64,
    /// Верхняя граница полос, Гц.
    pub f_max: f64,
}

impl Default for MelConfig {
    fn default() -> Self {
        MelConfig {
            rate: 22_050.0,
            n_fft: 1024,
            hop: 441,
            n_mels: 128,
            f_min: 30.0,
            f_max: 11_000.0,
        }
    }
}

impl MelConfig {
    /// Число частотных бинов одностороннего спектра.
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Число кадров для сигнала такой длины.
    pub fn frames(&self, samples: usize) -> usize {
        samples / self.hop + 1
    }
}

/// Шкала Слэйни: до 1000 Гц линейная, дальше логарифмическая.
fn hz_to_mel(hz: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    let logstep = 6.4_f64.ln() / 27.0;
    if hz < MIN_LOG_HZ {
        hz / F_SP
    } else {
        MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / logstep
    }
}

/// Обратная шкала Слэйни.
fn mel_to_hz(mel: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    let logstep = 6.4_f64.ln() / 27.0;
    if mel < MIN_LOG_MEL {
        mel * F_SP
    } else {
        MIN_LOG_HZ * (logstep * (mel - MIN_LOG_MEL)).exp()
    }
}

/// Треугольные фильтры: `n_mels × bins`, шкала Слэйни **без** нормировки площади.
///
/// Нормировка площади (`norm="slaney"`) — отдельный параметр torchaudio, и эталонная
/// `LogMelSpect` его не включает: площадь делится только если попросили. Ошибка здесь стоит
/// множителя в сотни раз и видна сразу — сверка с torchaudio дала расхождение 5,4 в логарифме.
pub fn mel_filterbank(config: &MelConfig) -> Vec<f32> {
    let bins = config.bins();
    let all_freqs: Vec<f64> = (0..bins)
        .map(|bin| bin as f64 * config.rate / config.n_fft as f64)
        .collect();
    let mel_min = hz_to_mel(config.f_min);
    let mel_max = hz_to_mel(config.f_max);
    let points: Vec<f64> = (0..config.n_mels + 2)
        .map(|index| {
            let mel = mel_min + (mel_max - mel_min) * index as f64 / (config.n_mels + 1) as f64;
            mel_to_hz(mel)
        })
        .collect();
    let mut filters = vec![0.0f32; config.n_mels * bins];
    for band in 0..config.n_mels {
        let left = points[band];
        let center = points[band + 1];
        let right = points[band + 2];
        for (bin, freq) in all_freqs.iter().enumerate() {
            let weight = if *freq > left && *freq <= center {
                (freq - left) / (center - left)
            } else if *freq > center && *freq < right {
                (right - freq) / (right - center)
            } else {
                0.0
            };
            filters[band * bins + bin] = weight as f32;
        }
    }
    filters
}

/// Периодическое окно Ханна — как `torch.hann_window(n_fft, periodic=True)`.
fn hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|index| {
            let phase = std::f64::consts::TAU * index as f64 / size as f64;
            (0.5 - 0.5 * phase.cos()) as f32
        })
        .collect()
}

/// Отражение краёв — как `torch.nn.functional.pad(..., "reflect")`.
fn reflect_pad(samples: &[f32], left: usize, right: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(samples.len() + left + right);
    for step in 0..left {
        let index = left - step;
        out.push(samples[index.min(samples.len() - 1)]);
    }
    out.extend_from_slice(samples);
    for step in 0..right {
        let index = samples.len().saturating_sub(2 + step);
        out.push(samples[index]);
    }
    out
}

/// Лог-мел-спектрограмма: кадры по строкам, полосы по столбцам, `ln(1 + 1000·x)`.
///
/// Кадр центрирован на `t · hop` (дополнение отражением), окно Ханна, нормировка на
/// `sqrt(n_fft)` — ровно то, что делает `torch.stft(normalized=True)` внутри torchaudio.
pub fn log_mel(samples: &[f32], config: &MelConfig) -> Vec<f32> {
    let bins = config.bins();
    let frames = config.frames(samples.len());
    let window = hann_window(config.n_fft);
    let filters = mel_filterbank(config);
    let norm = 1.0 / (config.n_fft as f32).sqrt();
    let padded = reflect_pad(samples, config.n_fft / 2, config.n_fft / 2);

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(config.n_fft);
    let mut buffer = vec![Complex::new(0.0f32, 0.0); config.n_fft];
    let mut out = vec![0.0f32; frames * config.n_mels];
    for frame in 0..frames {
        let start = frame * config.hop;
        for index in 0..config.n_fft {
            buffer[index] = Complex::new(padded[start + index] * window[index], 0.0);
        }
        fft.process(&mut buffer);
        for band in 0..config.n_mels {
            let mut sum = 0.0f32;
            for bin in 0..bins {
                sum += buffer[bin].norm() * norm * filters[band * bins + bin];
            }
            out[frame * config.n_mels + band] = (1.0 + 1000.0 * sum).ln();
        }
    }
    out
}
