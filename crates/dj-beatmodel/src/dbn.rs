//! DBN по активациям модели: состояния «темп × смещение внутри доли», Viterbi.
//!
//! Перенос прототипа, на котором считались числа (`docs/qa/evidence/A15/raveform.md`, раздел про
//! DBN): 60 темпов 55…215 BPM, период в кадрах 50 Гц, переход на границе доли со штрафом
//! `exp(−λ·|ln(τ/τ′)|)`, наблюдение — активация на доле и `(1 − a)/(λ_obs − 1)` между долями.
//! Позиция в такте пробовалась и ухудшила непрерывность (там же), поэтому в контракт не вошла.

/// Параметры декодера.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DbnConfig {
    /// Кадров в секунду у активаций.
    pub fps: f64,
    /// Нижняя граница темпов, BPM.
    pub min_bpm: f64,
    /// Верхняя граница темпов, BPM.
    pub max_bpm: f64,
    /// Сколько темпов перебирается между границами.
    pub num_tempi: usize,
    /// Штраф за смену темпа на границе доли: больше — жёстче.
    pub lambda: f64,
    /// Во сколько раз тишина между долями менее вероятна, чем удар.
    pub obs_lambda: f64,
}

impl Default for DbnConfig {
    fn default() -> Self {
        DbnConfig {
            fps: 50.0,
            min_bpm: 55.0,
            max_bpm: 215.0,
            num_tempi: 60,
            // λ=30 дал лучшую непрерывность в переборе (CMLt 0,772 против 0,759 при 100).
            lambda: 30.0,
            obs_lambda: 16.0,
        }
    }
}

/// Состояния: для каждого темпа — своя доля в кадрах, смещение внутри неё.
struct Space {
    offset: Vec<u16>,
    start: Vec<usize>,
    end: Vec<usize>,
    tempi: Vec<f64>,
    size: usize,
}

fn build_space(config: &DbnConfig) -> Space {
    let mut tempi = Vec::with_capacity(config.num_tempi);
    let mut period = Vec::with_capacity(config.num_tempi);
    // период нужен только для раскладки состояний; в Space он не хранится
    for index in 0..config.num_tempi {
        let fraction = index as f64 / (config.num_tempi.saturating_sub(1).max(1)) as f64;
        let bpm = config.min_bpm + (config.max_bpm - config.min_bpm) * fraction;
        tempi.push(bpm);
        let frames = (config.fps * 60.0 / bpm).round() as usize;
        period.push(frames.max(2));
    }
    let mut start = Vec::with_capacity(config.num_tempi);
    let mut end = Vec::with_capacity(config.num_tempi);
    let mut offset = Vec::new();
    let mut cursor = 0usize;
    for frames in &period {
        start.push(cursor);
        for step in 0..*frames {
            offset.push(step as u16);
            cursor += 1;
        }
        end.push(cursor - 1);
    }
    Space {
        offset,
        start,
        end,
        tempi,
        size: cursor,
    }
}

/// Доли по активации: время в секундах от начала файла.
///
/// Активация — вероятность удара на кадре (после сигмоиды), длина произвольная. Пустая или
/// слишком короткая активация даёт пустой ответ: декодировать нечего.
pub fn decode_beats(activation: &[f32], config: &DbnConfig) -> Vec<f64> {
    if activation.len() < 8 {
        return Vec::new();
    }
    let space = build_space(config);
    let tempo_count = space.tempi.len();
    // штраф за смену темпа: exp(−λ·|ln(τ/τ′)|) в логарифмах
    let mut penalty = vec![0.0f32; tempo_count * tempo_count];
    for from in 0..tempo_count {
        for to in 0..tempo_count {
            let ratio = (space.tempi[to] / space.tempi[from]).ln().abs();
            penalty[from * tempo_count + to] = -(config.lambda as f32) * ratio as f32;
        }
    }
    let log_beat: Vec<f32> = activation
        .iter()
        .map(|value| value.max(1e-6).ln())
        .collect();
    let log_off: Vec<f32> = activation
        .iter()
        .map(|value| ((1.0 - value).max(1e-6) / (config.obs_lambda as f32 - 1.0)).ln())
        .collect();

    let frames = activation.len();
    // Первый кадр не обязан быть долей: путь может начаться посреди доли. Иначе декодер всегда
    // отдаёт удар в нуле, и якорь, взятый по первой доле, ставит сетку на полдоли в сторону —
    // на корпусе это стоило фазы 113 мс и F1 0,37.
    let mut score = vec![0.0f32; space.size];
    for (state, value) in score.iter_mut().enumerate() {
        *value = if space.offset[state] == 0 {
            log_beat[0]
        } else {
            log_off[0]
        };
    }
    // обратные указатели: для каждой пары (кадр, состояние) — откуда пришли
    let mut back = vec![0u16; frames * space.size];
    let mut next = vec![f32::NEG_INFINITY; space.size];
    for frame in 1..frames {
        next[1..].copy_from_slice(&score[..space.size - 1]);
        let row = &mut back[frame * space.size..(frame + 1) * space.size];
        for (slot, slot_back) in row.iter_mut().enumerate() {
            *slot_back = slot.saturating_sub(1) as u16;
        }
        // начало новой доли: лучший конец любой доли плюс штраф за смену темпа
        for to in 0..tempo_count {
            let mut best_from = 0usize;
            let mut best = f32::NEG_INFINITY;
            for from in 0..tempo_count {
                let candidate = score[space.end[from]] + penalty[from * tempo_count + to];
                if candidate > best {
                    best = candidate;
                    best_from = from;
                }
            }
            let slot = space.start[to];
            next[slot] = best;
            back[frame * space.size + slot] = space.end[best_from] as u16;
        }
        for (slot, value) in next.iter_mut().enumerate() {
            *value += if space.offset[slot] == 0 {
                log_beat[frame]
            } else {
                log_off[frame]
            };
        }
        std::mem::swap(&mut score, &mut next);
    }

    let mut best_state = 0usize;
    let mut best = f32::NEG_INFINITY;
    for (state, value) in score.iter().enumerate() {
        if *value > best {
            best = *value;
            best_state = state;
        }
    }
    let mut path = vec![0u16; frames];
    let mut state = best_state;
    for frame in (0..frames).rev() {
        path[frame] = state as u16;
        state = back[frame * space.size + state] as usize;
    }
    let mut beats = Vec::new();
    for (frame, state) in path.iter().enumerate() {
        if space.offset[*state as usize] == 0 {
            beats.push(frame as f64 / config.fps);
        }
    }
    beats
}
