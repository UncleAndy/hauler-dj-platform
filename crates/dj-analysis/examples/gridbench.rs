//! Замер сетки на корпусе: наш детектор против эталона (raveform).
//!
//! Читает TSV, который собирает `prep-corpus.py`: id, путь, BPM, первая доля, признак ровной
//! сетки, участки, жанр, длительность. Печатает по строке на трек и итог по классам.
//!
//! Запуск: `cargo run --release -p dj-analysis --example gridbench -- --corpus /tmp/bgt/corpus.tsv`

use std::io::Write as _;
use std::path::Path;

use dj_analysis::tempo::{LevelPrior, Tempo, detect_with_prior};
use dj_codec::StreamDecoder;

struct Reference {
    path: String,
    bpm: f64,
    first: f64,
    duration: f64,
    clean: bool,
    segments: Vec<(f64, f64)>,
}

fn load_corpus(path: &Path) -> std::io::Result<Vec<Reference>> {
    let text = std::fs::read_to_string(path)?;
    let mut rows = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 8 {
            continue;
        }
        let (Ok(bpm), Ok(first), Ok(duration)) = (
            fields[2].parse::<f64>(),
            fields[3].parse::<f64>(),
            fields[7].parse::<f64>(),
        ) else {
            continue;
        };
        let mut segments = Vec::new();
        if fields[5].starts_with('[') {
            for part in fields[5].trim_matches(['[', ']']).split('}') {
                let mut time = None;
                let mut value = None;
                for piece in part.split(',') {
                    let piece = piece.trim().trim_start_matches('{');
                    if let Some(rest) = piece.strip_prefix("\"time\":") {
                        time = rest.trim().parse::<f64>().ok();
                    }
                    if let Some(rest) = piece.strip_prefix("\"bpm\":") {
                        value = rest.trim().parse::<f64>().ok();
                    }
                }
                if let (Some(time), Some(value)) = (time, value) {
                    segments.push((time, value));
                }
            }
        }
        rows.push(Reference {
            path: fields[1].to_string(),
            bpm,
            first,
            duration,
            clean: fields[4] == "1",
            segments,
        });
    }
    Ok(rows)
}

fn decode(path: &Path) -> Result<(Vec<f32>, f64), String> {
    let mut decoder = StreamDecoder::open(path).map_err(|error| error.to_string())?;
    let rate = f64::from(decoder.sample_rate());
    let mut mono = Vec::new();
    let mut block = vec![0.0_f32; 8192 * 2];
    loop {
        let read = decoder
            .read_frames(&mut block)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        for pair in block[..read * 2].chunks_exact(2) {
            mono.push((pair[0] + pair[1]) * 0.5);
        }
    }
    if mono.is_empty() {
        return Err("пусто".to_string());
    }
    Ok((mono, rate))
}

fn level_class(ours: f64, reference: f64) -> &'static str {
    if !(ours.is_finite() && reference > 0.0) {
        return "отказ";
    }
    let ratio = ours / reference;
    let ladder = [
        (1.0, "точно", 0.002),
        (1.0, "рядом", 0.010),
        (2.0, "октава x2", 0.010),
        (0.5, "октава /2", 0.010),
        (4.0 / 3.0, "4:3", 0.010),
        (3.0 / 4.0, "3:4", 0.010),
        (3.0 / 2.0, "3:2", 0.010),
        (2.0 / 3.0, "2:3", 0.010),
        (5.0 / 4.0, "5:4", 0.010),
        (4.0 / 5.0, "4:5", 0.010),
    ];
    for (value, name, tolerance) in ladder {
        if (ratio - value).abs() / value <= tolerance {
            return name;
        }
    }
    "мимо"
}

fn grid_of(tempo: &Tempo, rate: f64) -> Vec<(f64, f64)> {
    tempo
        .segments
        .iter()
        .map(|segment| (segment.first_beat_frame as f64 / rate, segment.bpm))
        .collect()
}

fn nearest_beat(segments: &[(f64, f64)], t: f64) -> Option<f64> {
    let mut chosen = None;
    for (index, (start, bpm)) in segments.iter().enumerate() {
        if *bpm <= 0.0 {
            continue;
        }
        let end = segments
            .get(index + 1)
            .map_or(f64::INFINITY, |(next, _)| *next);
        if t >= *start && t < end {
            chosen = Some((*start, 60.0 / *bpm));
            break;
        }
    }
    let (start, period) = chosen.or_else(|| segments.first().map(|(s, b)| (*s, 60.0 / *b)))?;
    if !(period.is_finite() && period > 0.0) {
        return None;
    }
    Some(start + ((t - start) / period).round() * period)
}

fn our_beats(tempo: &Tempo, rate: f64, duration: f64) -> Vec<f64> {
    let segments = grid_of(tempo, rate);
    let mut beats = Vec::new();
    for (index, (start, bpm)) in segments.iter().enumerate() {
        let stop = segments
            .get(index + 1)
            .map_or(duration + 5.0, |(next, _)| *next);
        if *bpm <= 0.0 {
            continue;
        }
        let period = 60.0 / *bpm;
        let mut beat = *start;
        while beat < stop {
            if beat >= 0.0 {
                beats.push(beat);
            }
            beat += period;
        }
    }
    beats
}

fn reference_beats(reference: &Reference) -> Vec<f64> {
    let segments = if reference.clean || reference.segments.is_empty() {
        vec![(reference.first, reference.bpm)]
    } else {
        reference.segments.clone()
    };
    let mut beats = Vec::new();
    for (index, (start, bpm)) in segments.iter().enumerate() {
        let stop = segments
            .get(index + 1)
            .map_or(reference.duration + 5.0, |(next, _)| *next);
        if *bpm <= 0.0 {
            continue;
        }
        let period = 60.0 / *bpm;
        let mut beat = *start;
        while beat < stop {
            if beat >= 0.0 {
                beats.push(beat);
            }
            beat += period;
        }
    }
    beats
}

fn main() {
    let mut corpus = String::from("/tmp/bgt/corpus.tsv");
    let mut every = 1usize;
    let mut limit = usize::MAX;
    let mut only_clean = false;
    let mut ids: Option<Vec<String>> = None;
    let mut dump: Option<String> = None;
    let mut prior_path: Option<String> = None;
    let mut beatmodel: Option<String> = None;
    let mut ort_library: Option<String> = None;
    let mut prior_dump: Option<String> = None;
    let mut grid_from_model = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--corpus" => {
                if let Some(value) = arguments.next() {
                    corpus = value;
                }
            }
            "--every" => {
                if let Some(value) = arguments.next().and_then(|v| v.parse().ok()) {
                    every = value;
                }
            }
            "--limit" => {
                if let Some(value) = arguments.next().and_then(|v| v.parse().ok()) {
                    limit = value;
                }
            }
            "--only-clean" => only_clean = true,
            "--beatmodel" => {
                if let Some(value) = arguments.next() {
                    beatmodel = Some(value);
                }
            }
            "--grid-from-model" => grid_from_model = true,
            "--dump-prior" => {
                if let Some(value) = arguments.next() {
                    prior_dump = Some(value);
                }
            }
            "--ort" => {
                if let Some(value) = arguments.next() {
                    ort_library = Some(value);
                }
            }
            "--level-prior" => {
                if let Some(value) = arguments.next() {
                    prior_path = Some(value);
                }
            }
            "--dump-beats" => {
                if let Some(value) = arguments.next() {
                    dump = Some(value);
                }
            }
            "--ids" => {
                if let Some(value) = arguments.next() {
                    ids = Some(
                        std::fs::read_to_string(&value)
                            .unwrap_or_default()
                            .lines()
                            .map(|line| line.trim().to_string())
                            .filter(|line| !line.is_empty())
                            .collect(),
                    );
                }
            }
            other => eprintln!("неизвестный ключ: {other}"),
        }
    }
    let mut priors = std::collections::HashMap::new();
    if let Some(path) = &prior_path {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                for line in text.lines() {
                    let mut fields = line.split('\t');
                    if let (Some(id), Some(bpm)) = (fields.next(), fields.next())
                        && let Ok(bpm) = bpm.trim().parse::<f64>()
                    {
                        let anchor = fields.next().and_then(|v| v.trim().parse::<f64>().ok());
                        priors.insert(id.trim().to_string(), (bpm, anchor));
                    }
                }
            }
            Err(error) => eprintln!("приоры {path}: {error}"),
        }
    }
    let mut model = match (&beatmodel, &ort_library) {
        (Some(model_path), Some(library)) => {
            match dj_beatmodel::BeatModel::load(Path::new(library), Path::new(model_path)) {
                Ok(model) => Some(model),
                Err(error) => {
                    eprintln!("модель {model_path}: {error}");
                    return;
                }
            }
        }
        _ => None,
    };
    let rows = match load_corpus(Path::new(&corpus)) {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("корпус {corpus}: {error}");
            return;
        }
    };
    let mut tally = std::collections::BTreeMap::<&'static str, usize>::new();
    let mut phases: Vec<f64> = Vec::new();
    let mut drifts: Vec<f64> = Vec::new();
    let mut seconds = 0.0_f64;
    let mut done = 0usize;
    let mut out = std::io::BufWriter::new(std::io::stdout());
    let _ = writeln!(
        out,
        "id\tour_bpm\tref_bpm\tclass\tphase_ms\tphase90_ms\tdrift_ms_min\tours\trefs\tconf\tmetre\tms"
    );
    for (index, row) in rows.iter().enumerate() {
        if index % every != 0 || (only_clean && !row.clean) || done >= limit {
            continue;
        }
        let stem = Path::new(&row.path)
            .file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Some(wanted) = &ids
            && !wanted.contains(&stem)
        {
            continue;
        }
        let started = std::time::Instant::now();
        let beats = reference_beats(row);
        let reference_bpm = if row.clean || row.segments.is_empty() {
            row.bpm
        } else {
            row.segments.first().map_or(row.bpm, |(_, bpm)| *bpm)
        };
        let mut chosen: Option<(f64, Option<f64>)> = None;
        let decoded = decode(Path::new(&row.path));
        let our = decoded.ok().and_then(|(mono, rate)| {
            // Один проход по модели на трек: уровень и доли берутся из одних активаций.
            let mut model_beats: Option<Vec<f64>> = None;
            let prior = match model.as_mut() {
                Some(model) => match dj_beatmodel::prior_and_beats(
                    model,
                    &mono,
                    rate,
                    &dj_beatmodel::DbnConfig::default(),
                ) {
                    Ok((guess, found)) => {
                        model_beats = Some(found);
                        guess.map(|guess| LevelPrior {
                            bpm: guess.bpm,
                            tolerance: 0.02,
                            anchor: Some(guess.anchor),
                        })
                    }
                    Err(error) => {
                        eprintln!("{stem}: {error}");
                        None
                    }
                },
                None => priors.get(&stem).map(|(bpm, anchor)| LevelPrior {
                    bpm: *bpm,
                    tolerance: 0.02,
                    anchor: *anchor,
                }),
            };
            if let Some(prior) = &prior {
                chosen = Some((prior.bpm, prior.anchor));
            }
            if grid_from_model {
                return dj_analysis::tempo::detect_with_prior_and_beats(
                    &mono,
                    rate,
                    prior.as_ref(),
                    model_beats.as_deref(),
                )
                .map(|tempo| (tempo, rate));
            }
            detect_with_prior(&mono, rate, prior.as_ref()).map(|tempo| (tempo, rate))
        });
        if let (Some(path), Some((bpm, anchor))) = (&prior_dump, chosen) {
            let line = format!(
                "{stem}\t{bpm:.4}\t{}\n",
                anchor.map_or("-".to_string(), |value| format!("{value:.4}"))
            );
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| file.write_all(line.as_bytes()));
        }
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        seconds += elapsed / 1000.0;
        done += 1;
        let Some((tempo, rate)) = our else {
            *tally.entry("отказ").or_default() += 1;
            let _ = writeln!(
                out,
                "{}\t0\t{reference_bpm:.3}\tотказ\t0\t0\t0\t0\t{}\t0\t0\t{elapsed:.0}",
                row.path,
                if row.clean { 1 } else { row.segments.len() },
            );
            continue;
        };
        let class = level_class(tempo.bpm(), reference_bpm);
        *tally.entry(class).or_default() += 1;
        let grid = grid_of(&tempo, rate);
        if let Some(dir) = &dump {
            let mut body = String::new();
            for beat in our_beats(&tempo, rate, row.duration) {
                body.push_str(&format!("{beat:.4}\n"));
            }
            let _ = std::fs::write(Path::new(dir).join(format!("{stem}.txt")), body);
        }
        let mut distances = Vec::with_capacity(beats.len());
        let mut times = Vec::with_capacity(beats.len());
        for beat in &beats {
            if let Some(nearest) = nearest_beat(&grid, *beat) {
                distances.push((beat - nearest).abs() * 1000.0);
                times.push(*beat);
            }
        }
        let mut order: Vec<usize> = (0..distances.len()).collect();
        order.sort_by(|a, b| {
            distances[*a]
                .partial_cmp(&distances[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let median = order
            .get(order.len() / 2)
            .map_or(0.0, |position| distances[*position]);
        let tail = order
            .get(order.len() * 9 / 10)
            .map_or(0.0, |position| distances[*position]);
        let mut drift = 0.0;
        if distances.len() > 32 && tempo.is_single() && row.clean {
            let n = distances.len() as f64;
            let mean_t = times.iter().sum::<f64>() / n;
            let mean_d = distances.iter().sum::<f64>() / n;
            let mut num = 0.0;
            let mut den = 0.0;
            for (t, d) in times.iter().zip(distances.iter()) {
                num += (t - mean_t) * (d - mean_d);
                den += (t - mean_t) * (t - mean_t);
            }
            if den > 0.0 {
                drift = num / den * 60.0;
            }
        }
        if matches!(class, "точно" | "рядом") {
            phases.push(median);
            drifts.push(drift.abs());
        }
        let _ = writeln!(
            out,
            "{}\t{:.3}\t{reference_bpm:.3}\t{class}\t{median:.1}\t{tail:.1}\t{drift:.2}\t{}\t{}\t{:.2}\t{}\t{elapsed:.0}\t{:.4}",
            row.path,
            tempo.bpm(),
            tempo.segments.len(),
            if row.clean { 1 } else { row.segments.len() },
            tempo.confidence,
            tempo.metre,
            grid.first().map_or(0.0, |(start, _)| *start),
        );
    }
    let _ = out.flush();
    let total: usize = tally.values().sum();
    eprintln!(
        "\n# треков: {total}, {:.0} мс на трек",
        seconds * 1000.0 / total.max(1) as f64
    );
    for (class, count) in &tally {
        eprintln!(
            "# {:<12} {:4} {:5.1}%",
            class,
            count,
            *count as f64 * 100.0 / total.max(1) as f64
        );
    }
    phases.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    drifts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if !phases.is_empty() {
        eprintln!(
            "# фаза: медиана {:.1} мс, 90-й {:.1} мс, максимум {:.1} мс",
            phases[phases.len() / 2],
            phases[phases.len() * 9 / 10],
            phases[phases.len() - 1]
        );
        eprintln!(
            "# дрейф: медиана {:.2} мс/мин, 90-й {:.2}, максимум {:.2}",
            drifts[drifts.len() / 2],
            drifts[drifts.len() * 9 / 10],
            drifts[drifts.len() - 1]
        );
    }
}
