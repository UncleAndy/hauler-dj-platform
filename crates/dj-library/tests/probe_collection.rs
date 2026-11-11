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

use dj_library::tags::{Verdict as TextVerdict, repair};

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

impl From<TextVerdict> for Verdict {
    fn from(verdict: TextVerdict) -> Self {
        match verdict {
            TextVerdict::Ascii => Self::Ascii,
            TextVerdict::Clean => Self::Clean,
            TextVerdict::Repaired(encoding) => Self::Repaired(encoding),
            TextVerdict::Lost => Self::Lost,
        }
    }
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
    // Декодер пишет стерео всегда; info().channels — про файл, а не про выход.
    let channels = decoder.output_channels();
    let mut buffer = vec![0.0_f32; 8_192 * 2];
    let mut samples: Vec<i16> = Vec::new();
    loop {
        let read = decoder
            .read_frames(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        let count = (read * usize::from(channels)).min(buffer.len());
        for value in &buffer[..count] {
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
    let artist_verdict: Verdict = artist_verdict.into();
    let (title, title_verdict) = repair(&tags.title);
    let title_verdict: Verdict = title_verdict.into();
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
