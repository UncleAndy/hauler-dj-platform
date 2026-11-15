//! Приведение частоты дискретизации к той, что ждёт модель (22 050 Гц).
//!
//! Не ресемплер для воспроизведения: здесь нужен один проход по файлу и предсказуемое подавление
//! того, что выше половины новой частоты. Порядок такой: фильтр низких частот на входной частоте
//! (Кайзер, срез чуть ниже 11 кГц), затем линейная интерполяция в новую сетку.

/// Полуширина фильтра в отсчётах входной частоты.
const TAPS: usize = 32;

/// Коэффициенты фильтра низких частот с окном Кайзера.
fn lowpass(cutoff: f64, rate: f64) -> Vec<f32> {
    let normalized = 2.0 * cutoff / rate;
    let mut taps = Vec::with_capacity(2 * TAPS + 1);
    let beta = 8.0_f64;
    let mut sum = 0.0f64;
    for index in 0..=(2 * TAPS) {
        let offset = index as f64 - TAPS as f64;
        let sinc = if offset.abs() < 1e-9 {
            normalized
        } else {
            (std::f64::consts::PI * normalized * offset).sin() / (std::f64::consts::PI * offset)
        };
        let window_argument = offset / TAPS as f64;
        let window = kaiser(window_argument, beta);
        let value = sinc * window;
        taps.push(value as f32);
        sum += value;
    }
    // единичный коэффициент усиления на постоянном сигнале
    for tap in &mut taps {
        *tap = (*tap as f64 / sum) as f32;
    }
    taps
}

/// Окно Кайзера через функцию Бесселя первого рода нулевого порядка.
fn kaiser(x: f64, beta: f64) -> f64 {
    let argument = 1.0 - x * x;
    if argument <= 0.0 {
        return 0.0;
    }
    bessel_i0(beta * argument.sqrt()) / bessel_i0(beta)
}

/// Функция Бесселя первого рода нулевого порядка (ряд, сходится быстро при наших β).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for order in 1..40 {
        term *= (half / order as f64) * (half / order as f64);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// Пересчитать сигнал на новую частоту.
pub fn resample(samples: &[f32], from: f64, to: f64) -> Vec<f32> {
    if (from - to).abs() < 1e-9 || samples.is_empty() {
        return samples.to_vec();
    }
    let cutoff = (to / 2.0) * 0.96;
    let taps = lowpass(cutoff, from);
    // фильтр с компенсацией задержки: результат той же длины, что вход
    let mut filtered = vec![0.0f32; samples.len()];
    for (index, value) in filtered.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for (tap_index, tap) in taps.iter().enumerate() {
            let position = index as isize + TAPS as isize - tap_index as isize;
            if position >= 0 && (position as usize) < samples.len() {
                sum += samples[position as usize] * tap;
            }
        }
        *value = sum;
    }
    let ratio = from / to;
    let length = ((samples.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(length);
    for index in 0..length {
        let position = index as f64 * ratio;
        let left = position.floor() as usize;
        let fraction = (position - left as f64) as f32;
        let a = filtered.get(left).copied().unwrap_or(0.0);
        let b = filtered.get(left + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * fraction);
    }
    out
}
