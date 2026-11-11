//! PROBE: what a collection looks like before any of it is indexed.
//!
//! Reads tags, repairs broken encodings, measures quality, fingerprints every file and looks for
//! duplicates — and writes nothing anywhere. Run it over a folder:
//!
//! ```text
//! DJ_COLLECTION=/path/to/music cargo test -p dj-library --test probe_collection -- --nocapture
//! ```
//!
//! Without `DJ_COLLECTION` it measures `tmp/refmusic` beside the repository, so the gate stays green.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTag};

const AUDIO: [&str; 4] = ["wav", "flac", "mp3", "ogg"];
/// Shared subfingerprints below this are not worth a full comparison.
const MIN_SHARED: u32 = 8;
/// How far apart two copies may sit in the fingerprint, in its own frames (0,12 s each).
const MAX_LAG: i32 = 128;
/// Length of one fingerprint frame, measured: a lag of 16 items is 2,0 s of audio.
const FRAME_SECONDS: f64 = 0.1238;
/// How far the shift may be from the duration difference and still be the same recording.
const SHIFT_TOLERANCE_MS: u64 = 300;

#[derive(Debug, Default, Clone)]
struct Tags {
    artist: String,
    title: String,
    album: String,
    genre: String,
    year: String,
    track: String,
    count: usize,
}

#[derive(Debug, Clone)]
struct Quality {
    codec: String,
    lossless: bool,
    rate: u32,
    channels: u16,
    bits: Option<u32>,
    bitrate: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Nothing but ASCII: no encoding question at all.
    Ascii,
    /// Valid text that reads as text.
    Clean,
    /// Repaired, with the encoding it came from.
    Repaired(&'static str),
    /// Bytes were already lost before we saw them.
    Lost,
    /// The decoder refused the file: the reason travels with it.
    Undecodable,
}

fn main_root() -> PathBuf {
    if let Ok(dir) = std::env::var("DJ_COLLECTION") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/refmusic")
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| AUDIO.contains(&ext.to_ascii_lowercase().as_str()))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

/// A character a broken byte turns into: `Ð`, `Ñ`, `Ì`, `î`, `œ`, `€`, and the like.
///
/// These are letters too, so a score that only asks "is it alphabetic" calls mojibake clean — that is
/// exactly how the first version of this check passed `Ìîñêâà` through untouched.
fn suspicious(character: char) -> bool {
    ('\u{00A0}'..='\u{017F}').contains(&character) || ('\u{2000}'..='\u{2030}').contains(&character)
}

fn suspicious_ratio(text: &str) -> f64 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let bad = text
        .chars()
        .filter(|character| suspicious(*character))
        .count();
    bad as f64 / total as f64
}

/// How much a string looks like text: Latin and Cyrillic letters good, mojibake and controls bad.
fn text_score(text: &str) -> f64 {
    let mut good = 0.0_f64;
    let mut bad = 0.0_f64;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || ('\u{0400}'..='\u{04FF}').contains(&character) {
            good += 1.0;
        } else if character == ' ' || ".,'-&()/+:".contains(character) {
            good += 0.8;
        } else if character.is_control() || character == '\u{FFFD}' {
            bad += 3.0;
        } else if suspicious(character) {
            bad += 1.5;
        } else if character.is_alphabetic() {
            // Another alphabet: not evidence of damage, not evidence of health either.
            good += 0.5;
        } else {
            bad += 0.5;
        }
    }
    good / (good + bad).max(1.0)
}

fn cp1251(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            0x00..=0x7F => byte as char,
            0xA8 => 'Ё',
            0xB8 => 'ё',
            0xC0..=0xFF => char::from_u32(0x0410 + u32::from(byte - 0xC0)).unwrap_or('?'),
            _ => '?',
        })
        .collect()
}

fn latin1_bytes(text: &str) -> Option<Vec<u8>> {
    text.chars()
        .map(|character| u8::try_from(u32::from(character)).ok())
        .collect()
}

/// The bytes a cp1252 reader would have produced for these characters.
fn cp1252_bytes(text: &str) -> Option<Vec<u8>> {
    const HIGH: [(char, u8); 27] = [
        ('\u{20AC}', 0x80),
        ('\u{201A}', 0x82),
        ('\u{0192}', 0x83),
        ('\u{201E}', 0x84),
        ('\u{2026}', 0x85),
        ('\u{2020}', 0x86),
        ('\u{2021}', 0x87),
        ('\u{02C6}', 0x88),
        ('\u{2030}', 0x89),
        ('\u{0160}', 0x8A),
        ('\u{2039}', 0x8B),
        ('\u{0152}', 0x8C),
        ('\u{017D}', 0x8E),
        ('\u{2018}', 0x91),
        ('\u{2019}', 0x92),
        ('\u{201C}', 0x93),
        ('\u{201D}', 0x94),
        ('\u{2022}', 0x95),
        ('\u{2013}', 0x96),
        ('\u{2014}', 0x97),
        ('\u{02DC}', 0x98),
        ('\u{2122}', 0x99),
        ('\u{0161}', 0x9A),
        ('\u{203A}', 0x9B),
        ('\u{0153}', 0x9C),
        ('\u{017E}', 0x9E),
        ('\u{0178}', 0x9F),
    ];
    let mut bytes = Vec::with_capacity(text.len());
    for character in text.chars() {
        if let Ok(byte) = u8::try_from(u32::from(character)) {
            bytes.push(byte);
            continue;
        }
        let (_, byte) = HIGH.iter().find(|(known, _)| *known == character)?;
        bytes.push(*byte);
    }
    Some(bytes)
}

/// Repair a tag string: cp1251 read as Latin-1, or UTF-8 read as cp1252 (the `ÐœÐ¾ÑÐºÐ²Ð°` case).
/// True when no single word mixes Latin and Cyrillic letters.
///
/// The two repairs that matter — cp1251 read as Latin-1 and UTF-8 read as cp1252 — produce text in one
/// alphabet, word by word. A candidate that turns `Käärijä` into `Kддrijд` mixes them inside a word,
/// and that is the signature of a guess rather than of a repair.
fn single_script(text: &str) -> bool {
    for word in text.split_whitespace() {
        let mut latin = false;
        let mut cyrillic = false;
        for character in word.chars() {
            if character.is_ascii_alphabetic() {
                latin = true;
            } else if ('\u{0400}'..='\u{04FF}').contains(&character) {
                cyrillic = true;
            }
        }
        if latin && cyrillic {
            return false;
        }
    }
    true
}

fn repair(text: &str) -> (String, Verdict) {
    if text.is_empty() {
        return (String::new(), Verdict::Clean);
    }
    if text.is_ascii() {
        return (text.to_owned(), Verdict::Ascii);
    }
    if text.contains('\u{FFFD}') {
        return (text.to_owned(), Verdict::Lost);
    }
    // One accented letter in a title is a title, not damage: repair is attempted only when a good
    // share of the string is made of the characters broken bytes produce.
    if suspicious_ratio(text) < 0.3 {
        return (text.to_owned(), Verdict::Clean);
    }
    let original = text_score(text);
    let mut best = (text.to_owned(), Verdict::Clean, original);
    let consider =
        |candidate: Option<String>, verdict: Verdict, best: &mut (String, Verdict, f64)| {
            let Some(candidate) = candidate else {
                return;
            };
            if suspicious_ratio(&candidate) > 0.0 || !single_script(&candidate) {
                return;
            }
            let score = text_score(&candidate);
            if score > best.2 + 0.05 {
                *best = (candidate, verdict, score);
            }
        };
    if let Some(bytes) = latin1_bytes(text) {
        consider(Some(cp1251(&bytes)), Verdict::Repaired("cp1251"), &mut best);
    }
    if let Some(bytes) = cp1252_bytes(text)
        && let Ok(candidate) = std::str::from_utf8(&bytes)
    {
        consider(
            Some(candidate.to_owned()),
            Verdict::Repaired("utf8"),
            &mut best,
        );
    }
    (best.0, best.1)
}

fn read_tags(path: &Path) -> (Tags, Quality) {
    let file = std::fs::File::open(path).expect("open");
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("probe");
    let mut tags = Tags::default();
    let metadata = format.metadata();
    if let Some(revision) = metadata.current() {
        for tag in &revision.media.tags {
            tags.count += 1;
            match &tag.std {
                Some(StandardTag::Artist(value)) => tags.artist = value.to_string(),
                Some(StandardTag::TrackTitle(value)) => tags.title = value.to_string(),
                Some(StandardTag::Album(value)) => tags.album = value.to_string(),
                Some(StandardTag::Genre(value)) => tags.genre = value.to_string(),
                Some(StandardTag::RecordingDate(value)) => tags.year = value.to_string(),
                Some(StandardTag::TrackNumber(number)) => tags.track = number.to_string(),
                _ => {}
            }
        }
    }
    let params = format
        .default_track(TrackType::Audio)
        .and_then(|track| track.codec_params.as_ref())
        .and_then(|params| params.audio());
    let codec_name = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(dj_codec::MediaFormat::from_extension)
    {
        Some(dj_codec::MediaFormat::Wav) => "PCM/WAV",
        Some(dj_codec::MediaFormat::Aiff) => "PCM/AIFF",
        Some(dj_codec::MediaFormat::Flac) => "FLAC",
        Some(dj_codec::MediaFormat::Mp3) => "MP3",
        Some(dj_codec::MediaFormat::OggVorbis) => "OGG/Vorbis",
        _ => "неизвестно",
    }
    .to_owned();
    let lossless = matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(dj_codec::MediaFormat::from_extension),
        Some(dj_codec::MediaFormat::Wav)
            | Some(dj_codec::MediaFormat::Aiff)
            | Some(dj_codec::MediaFormat::Flac)
    );
    let (rate, channels, bits) = params
        .map(
            |audio: &symphonia::core::codecs::audio::AudioCodecParameters| {
                (
                    audio.sample_rate.unwrap_or(0),
                    audio
                        .channels
                        .as_ref()
                        .map(|channels| channels.count())
                        .unwrap_or(0),
                    audio.bits_per_sample.or(audio.bits_per_coded_sample),
                )
            },
        )
        .unwrap_or((0, 0, None));
    (
        tags,
        Quality {
            codec: codec_name,
            lossless,
            rate,
            channels: u16::try_from(channels).unwrap_or(0),
            bits,
            bitrate: None,
        },
    )
}

fn decode_all(path: &Path) -> Result<(Vec<i16>, u32, u16, f64), String> {
    let started = Instant::now();
    let mut decoder = dj_codec::StreamDecoder::open(path).map_err(|error| error.to_string())?;
    let rate = decoder.sample_rate();
    let channels = decoder.info().channels;
    let mut buffer = vec![0.0_f32; 8_192 * 2];
    let mut samples: Vec<i16> = Vec::new();
    loop {
        let read = decoder
            .read_frames(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        for value in &buffer[..read * 2] {
            samples.push((value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
        }
    }
    Ok((samples, rate, channels, started.elapsed().as_secs_f64()))
}

/// Words that name an edition rather than a credit.
///
/// Audio answers "same recording", tags answer "same edition": `(Dub Mix)` and `(Extended Mix)` of the
/// same track agree in the fingerprint even when their lengths match, and `(Bassboost …)` is another
/// mastering of the same take. `(feat. …)`, `(prod. …)` and `(Original Mix)` are not editions — the
/// first two credit people, the third is what a bare title means.
const EDITION_WORDS: [&str; 24] = [
    "dub",
    "extended",
    "radio",
    "club",
    "instrumental",
    "acoustic",
    "live",
    "remix",
    "remixed",
    "edit",
    "rework",
    "remaster",
    "remastered",
    "bassboost",
    "sped",
    "slowed",
    "vip",
    "bootleg",
    "mashup",
    "flip",
    "cover",
    "demo",
    "reprise",
    "orchestral",
];

/// The set of edition words a title mentions, matched by whole words.
///
/// Whole words, not substrings: `Alive` contains `live` as letters and is not a live recording.
fn edition_markers(title: &str) -> Vec<&'static str> {
    let lowered = title.to_lowercase();
    let mut found: Vec<&'static str> = Vec::new();
    for token in lowered.split(|character: char| !character.is_alphanumeric()) {
        if let Some(word) = EDITION_WORDS.iter().find(|word| **word == token)
            && !found.contains(word)
        {
            found.push(word);
        }
    }
    found.sort_unstable();
    found
}

/// The verdict of one candidate pair, decided by audio first and by tags second.
fn pair_verdict(
    agreement: f64,
    delta_ms: u64,
    tolerance_ms: u64,
    trimmed: bool,
    same_edition: bool,
) -> &'static str {
    if agreement < 0.90 {
        return "разные";
    }
    if !same_edition {
        return "ВЕРСИЯ (издание)";
    }
    if delta_ms <= tolerance_ms {
        "ДУБЛЬ"
    } else if trimmed {
        "ДУБЛЬ (обрезан)"
    } else {
        "ВЕРСИЯ"
    }
}

fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Ascii => "ascii",
        Verdict::Clean => "чисто",
        Verdict::Repaired("cp1251") => "починено из cp1251",
        Verdict::Repaired(_) => "починено из двойного UTF-8",
        Verdict::Lost => "байты потеряны",
        Verdict::Undecodable => "не декодируется",
    }
}

fn measure(path: &Path) -> Measured {
    let (tags, mut quality) = read_tags(path);
    let (samples, rate, channels, decode_s) = match decode_all(path) {
        Ok(decoded) => decoded,
        Err(reason) => {
            return Measured {
                path: path.to_path_buf(),
                tags,
                quality,
                verdict: Verdict::Undecodable,
                repaired_artist: String::new(),
                repaired_title: String::new(),
                duration_ms: 0,
                fingerprint: Vec::new(),
                decode_s: 0.0,
                fingerprint_s: 0.0,
                failure: Some(reason),
            };
        }
    };
    let frames = samples.len() / usize::from(channels.max(1));
    let duration_ms = if rate > 0 {
        u64::try_from(frames).unwrap_or(0) * 1_000 / u64::from(rate)
    } else {
        0
    };
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    quality.bitrate = size
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(duration_ms);
    let fingerprint_started = Instant::now();
    let fingerprint = dj_fingerprint::of_samples(&samples, rate, channels).unwrap_or_default();
    let fingerprint_s = fingerprint_started.elapsed().as_secs_f64();
    let (artist, artist_verdict) = repair(&tags.artist);
    let (title, title_verdict) = repair(&tags.title);
    let verdict = match (artist_verdict, title_verdict) {
        (Verdict::Repaired(encoding), _) | (_, Verdict::Repaired(encoding)) => {
            Verdict::Repaired(encoding)
        }
        (Verdict::Lost, _) | (_, Verdict::Lost) => Verdict::Lost,
        (Verdict::Ascii, Verdict::Ascii) => Verdict::Ascii,
        _ => Verdict::Clean,
    };
    Measured {
        path: path.to_path_buf(),
        tags,
        quality,
        verdict,
        duration_ms,
        fingerprint,
        decode_s,
        fingerprint_s,
        repaired_artist: artist,
        repaired_title: title,
        failure: None,
    }
}

/// Measure every file, `workers` of them at a time, and keep the order of the paths.
fn measure_all(files: &[PathBuf], workers: usize) -> Vec<Measured> {
    let workers = workers.clamp(1, files.len().max(1));
    let mut measured: Vec<Measured> = Vec::with_capacity(files.len());
    if workers == 1 {
        for path in files {
            measured.push(measure(path));
        }
        return measured;
    }
    // Round-robin, so a folder of small files does not land on one thread and a folder of long ones on
    // another: the work is per file, and the file sizes are not known before opening them.
    let mut chunks: Vec<Vec<&PathBuf>> = vec![Vec::new(); workers];
    for (index, path) in files.iter().enumerate() {
        chunks[index % workers].push(path);
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|path| measure(path))
                    .collect::<Vec<Measured>>()
            }));
        }
        for handle in handles {
            measured.extend(handle.join().expect("worker"));
        }
    });
    measured.sort_by(|left, right| left.path.cmp(&right.path));
    measured
}

struct Measured {
    path: PathBuf,
    tags: Tags,
    quality: Quality,
    verdict: Verdict,
    repaired_artist: String,
    repaired_title: String,
    duration_ms: u64,
    fingerprint: Vec<u32>,
    decode_s: f64,
    fingerprint_s: f64,
    /// Why the file could not be measured, when it could not.
    failure: Option<String>,
}

#[test]
fn probe_collection() {
    let root = main_root();
    if !root.is_dir() {
        println!("PROBE: no collection at {}", root.display());
        return;
    }
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    println!("PROBE: {} files under {}", files.len(), root.display());

    // Files are independent, and the work is a decode plus a fingerprint: the only shared thing is the
    // disk. Threads are what turns 111 hours on a hundred thousand tracks into a working evening, and
    // the count is the operator's choice — `DJ_THREADS=1` is a legitimate answer on a spinning disk.
    let workers = std::env::var("DJ_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(4)
        });
    println!("PROBE: {} threads", workers);

    let started = Instant::now();
    let measured = measure_all(&files, workers);
    let total_s = started.elapsed().as_secs_f64();
    for track in &measured {
        println!(
            "PROBE file {} | {} {} Hz {} ch {:?} бит {:?} кбит/с | {:.1} с | тегов {} | «{}» — «{}» | {} | отпечаток {} | декод {:.1} с + отпечаток {:.1} с",
            track.path.display(),
            track.quality.codec,
            track.quality.rate,
            track.quality.channels,
            track.quality.bits,
            track.quality.bitrate.map(|bitrate| bitrate / 1_000),
            track.duration_ms as f64 / 1_000.0,
            track.tags.count,
            track.repaired_artist,
            track.repaired_title,
            verdict_name(track.verdict),
            track.fingerprint.len(),
            track.decode_s,
            track.fingerprint_s,
        );
        if let Some(reason) = &track.failure {
            println!("PROBE undecodable {} | {reason}", track.path.display());
        }
    }
    // Duplicates: shared subfingerprints first, full comparison only for the candidates.
    let mut index: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut pairs: Vec<(usize, usize, f64, i32, u32)> = Vec::new();
    for (index_of, track) in measured.iter().enumerate() {
        let mut shared: HashMap<usize, u32> = HashMap::new();
        for hash in &track.fingerprint {
            if let Some(others) = index.get(hash) {
                for &other in others {
                    *shared.entry(other).or_default() += 1;
                }
            }
        }
        let mut candidates: Vec<(usize, u32)> = shared.into_iter().collect();
        candidates.sort_by_key(|(_, shared)| std::cmp::Reverse(*shared));
        for (other, count) in candidates.into_iter().take(3) {
            if count < MIN_SHARED {
                continue;
            }
            let agreement = dj_fingerprint::agreement(
                &measured[other].fingerprint,
                &track.fingerprint,
                MAX_LAG,
            );
            pairs.push((other, index_of, agreement.same(), agreement.lag, count));
        }
        for hash in &track.fingerprint {
            index.entry(*hash).or_default().push(index_of);
        }
    }
    let tolerance_of = |first: usize, second: usize| {
        let longer = measured[first]
            .duration_ms
            .max(measured[second].duration_ms);
        // An encoder pads: mp3 rounds the length to whole frames and keeps its delay. A lossy file
        // against a lossless one of the same recording therefore differs by a second or two without
        // being another edition, while radio and extended mixes differ by tens of seconds.
        if measured[first].quality.lossless == measured[second].quality.lossless {
            (longer / 200).max(1_500)
        } else {
            (longer / 100).max(3_000)
        }
    };
    let mut duplicates = 0;
    for (first, second, same, lag, shared) in &pairs {
        let delta = measured[*first]
            .duration_ms
            .abs_diff(measured[*second].duration_ms);
        let tolerance = tolerance_of(*first, *second);
        // A copy that was trimmed is shifted in the fingerprint by exactly the length it lost. That is
        // the same recording, not another version — a radio edit and an extended mix do not line up.
        let shifted_ms = (f64::from(lag.unsigned_abs()) * FRAME_SECONDS * 1_000.0) as u64;
        let trimmed = delta.abs_diff(shifted_ms) <= SHIFT_TOLERANCE_MS;
        let same_edition = edition_markers(&measured[*first].repaired_title)
            == edition_markers(&measured[*second].repaired_title);
        let verdict = pair_verdict(*same, delta, tolerance, trimmed, same_edition);
        if verdict.starts_with("ДУБЛЬ") {
            duplicates += 1;
        }
        println!(
            "PROBE pair {verdict} | {} <-> {} | совпадение {same:.4} лаг {lag} (сдвиг {shifted_ms} мс) | общих окон {shared} | дельта длительности {delta} мс (допуск {tolerance})",
            measured[*first].path.display(),
            measured[*second].path.display()
        );
    }

    let mut by_format: HashMap<String, usize> = HashMap::new();
    for track in &measured {
        *by_format.entry(track.quality.codec.clone()).or_default() += 1;
    }
    let lossless = measured
        .iter()
        .filter(|track| track.quality.lossless)
        .count();
    let undecodable = measured
        .iter()
        .filter(|track| track.failure.is_some())
        .count();
    let mut encodings: HashMap<String, usize> = HashMap::new();
    for track in &measured {
        let name = match track.verdict {
            Verdict::Ascii => "ascii",
            Verdict::Clean => "чисто",
            Verdict::Repaired("cp1251") => "починено cp1251",
            Verdict::Repaired(_) => "починено utf8",
            Verdict::Lost => "потеряно",
            Verdict::Undecodable => "не декодируется",
        };
        *encodings.entry(name.to_owned()).or_default() += 1;
    }
    let tagged = measured
        .iter()
        .filter(|track| !track.tags.artist.is_empty() && !track.tags.title.is_empty())
        .count();
    let decode_total: f64 = measured.iter().map(|track| track.decode_s).sum();
    let fingerprint_total: f64 = measured.iter().map(|track| track.fingerprint_s).sum();
    println!(
        "PROBE summary: файлов {} | с тегами artist+title {} | lossless {} | не декодируется {} | форматы {:?} | кодировки {:?} | пар-дублей {} | {:.1} с на всё (декод {:.1} с, отпечатки {:.1} с)",
        measured.len(),
        tagged,
        lossless,
        undecodable,
        by_format,
        encodings,
        duplicates,
        total_s,
        decode_total,
        fingerprint_total
    );
}

#[cfg(test)]
mod encoding {
    use super::*;

    /// What a cp1252 reader shows for these bytes; undefined slots keep their control character, which
    /// is exactly what makes the mojibake recognisable.
    fn cp1252_decode(bytes: &[u8]) -> String {
        const HIGH: [(u8, char); 27] = [
            (0x80, '\u{20AC}'),
            (0x82, '\u{201A}'),
            (0x83, '\u{0192}'),
            (0x84, '\u{201E}'),
            (0x85, '\u{2026}'),
            (0x86, '\u{2020}'),
            (0x87, '\u{2021}'),
            (0x88, '\u{02C6}'),
            (0x89, '\u{2030}'),
            (0x8A, '\u{0160}'),
            (0x8B, '\u{2039}'),
            (0x8C, '\u{0152}'),
            (0x8E, '\u{017D}'),
            (0x91, '\u{2018}'),
            (0x92, '\u{2019}'),
            (0x93, '\u{201C}'),
            (0x94, '\u{201D}'),
            (0x95, '\u{2022}'),
            (0x96, '\u{2013}'),
            (0x97, '\u{2014}'),
            (0x98, '\u{02DC}'),
            (0x99, '\u{2122}'),
            (0x9A, '\u{0161}'),
            (0x9B, '\u{203A}'),
            (0x9C, '\u{0153}'),
            (0x9E, '\u{017E}'),
            (0x9F, '\u{0178}'),
        ];
        bytes
            .iter()
            .map(|byte| {
                HIGH.iter()
                    .find(|(known, _)| known == byte)
                    .map(|(_, character)| *character)
                    .unwrap_or(*byte as char)
            })
            .collect()
    }

    #[test]
    fn a_cp1251_artist_read_as_latin1_is_repaired() {
        // «Москва» in cp1251, read by a reader that assumed Latin-1.
        let broken = cp1252_decode(b"\xcc\xee\xf1\xea\xe2\xe0");
        let (fixed, verdict) = repair(&broken);
        assert_eq!(fixed, "Москва", "получилось «{fixed}»");
        assert_eq!(verdict, Verdict::Repaired("cp1251"));
    }

    #[test]
    fn utf8_read_as_cp1252_is_repaired() {
        // «Москва» in UTF-8, shown by a cp1252 reader: the `ÐœÐ¾ÑÐºÐ²Ð°` case.
        let broken = cp1252_decode("Москва".as_bytes());
        let (fixed, verdict) = repair(&broken);
        assert_eq!(fixed, "Москва", "получилось «{fixed}»");
        assert_eq!(verdict, Verdict::Repaired("utf8"));
    }

    #[test]
    fn clean_text_is_left_alone() {
        for text in [
            "Москва",
            "Abba",
            "Ёжик — Туман",
            "Track 12",
            "Édith Piaf",
            "Björk — Jóga",
            "Sigur Rós",
        ] {
            let (fixed, verdict) = repair(text);
            assert_eq!(fixed, text, "испортили «{text}»");
            assert!(matches!(verdict, Verdict::Clean | Verdict::Ascii), "{text}");
        }
    }

    #[test]
    fn lost_bytes_are_reported_rather_than_guessed() {
        let (fixed, verdict) = repair("Москва\u{FFFD}");
        assert_eq!(fixed, "Москва\u{FFFD}");
        assert_eq!(verdict, Verdict::Lost);
    }

    #[test]
    fn a_correct_name_with_accents_is_not_repaired_into_cyrillic() {
        // Käärijä is a Finnish artist. Read as Latin-1 his four `ä` are 0xE4, and 0xE4 in cp1251 is
        // `д`, so a repair that only asks "does this look like text?" turns the name into `Kддrijд` —
        // a word in two alphabets at once, which a repair never produces.
        let (artist, verdict) = repair("Käärijä");
        assert_eq!(artist, "Käärijä", "испортили имя: «{artist}»");
        assert_eq!(verdict, Verdict::Clean);
    }

    #[test]
    fn a_guess_that_mixes_alphabets_is_rejected() {
        assert!(!single_script("Kддrijд"));
        assert!(single_script("Москва"));
        assert!(single_script("Käärijä"));
        assert!(single_script("Steve Levi - Lost"));
        assert!(single_script("Ёжик — Туман"));
    }
}
mod duplicates {
    use super::*;

    #[test]
    fn an_edition_is_told_from_a_credit() {
        // Credit: the same recording, tagged with who is on it.
        assert_eq!(
            edition_markers("touch grass (feat. Yung Gravy)"),
            edition_markers("Touch Grass")
        );
        assert_eq!(
            edition_markers("C’est La Vie"),
            edition_markers("C'est La Vie (feat. Rich Brian)")
        );
        // An original mix is what a bare title means.
        assert_eq!(edition_markers("X (Original Mix)"), edition_markers("X"));
        // Editions: the same take, sold as another thing.
        assert_ne!(
            edition_markers("Bones (Dub Mix)"),
            edition_markers("Bones (Extended Mix)")
        );
        assert_ne!(
            edition_markers("ПЫХ-ПЫХ (Bassboost для глухонемых)"),
            edition_markers("ПЫХ-ПЫХ")
        );
        assert_ne!(
            edition_markers("Dance All Night (Extended Mix)"),
            edition_markers("Dance All Night")
        );
        assert_ne!(edition_markers("X (Radio Edit)"), edition_markers("X"));
        // Whole words: `Alive` is not a live recording, `Slowed` is an edition.
        assert_eq!(edition_markers("Alive"), edition_markers(""));
        assert_ne!(edition_markers("Slowed + Reverb"), edition_markers(""));
    }

    #[test]
    fn audio_decides_the_recording_and_tags_decide_the_edition() {
        // Two mixes of one track: the fingerprints agree, the tags do not, and the tags win.
        assert_eq!(
            pair_verdict(0.9086, 20, 1_500, false, false),
            "ВЕРСИЯ (издание)"
        );
        // The same recording in FLAC and mp3, 1,69 с apart: encoder padding, not another edition.
        assert_eq!(pair_verdict(0.9707, 1_689, 3_000, false, true), "ДУБЛЬ");
        // The same recording, one copy trimmed by two seconds: the shift explains the difference.
        assert_eq!(
            pair_verdict(0.9751, 2_000, 1_500, true, true),
            "ДУБЛЬ (обрезан)"
        );
        // Different recordings stay different whatever the tags say.
        assert_eq!(pair_verdict(0.5312, 0, 1_500, false, true), "разные");
    }
}
