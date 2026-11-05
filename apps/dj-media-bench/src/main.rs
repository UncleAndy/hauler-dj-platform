//! Measurement harness for `dj-codec` (A02).
//!
//! Answers the questions ADR-0008 leaves open, in numbers rather than in prose:
//!
//! * how much faster than realtime does each format decode (`decode`);
//! * how long does a load-to-ready take, cold file cache (`load`);
//! * what is the p99 cost of a seek that falls outside the prefetch window (`seek`).
//!
//! Usage:
//!   dj-media-bench decode --file X [--repeat N]
//!   dj-media-bench load   --file X [--window-seconds S]
//!   dj-media-bench seek   --file X [--count N]
//!
//! Output: one JSON object per `--file`, so results can be appended to evidence verbatim.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use dj_codec::{
    CacheConfig, CacheStorage, PcmCache, PcmSampleFormat, PrefetchConfig, Prefetcher, StreamDecoder,
};
use serde_json::json;

const USAGE: &str = "\
dj-media-bench — decode/load/seek measurements for dj-codec

Usage:
  dj-media-bench decode --file F [--repeat N]
  dj-media-bench load   --file F [--window-seconds S]
  dj-media-bench seek   --file F [--count N]
  dj-media-bench cache  --file F [--cache-dir D]

Exit codes: 0 ok, 2 bad usage, 3 media or I/O error.
";

const BLOCK_FRAMES: usize = 1024;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(Error::Usage(message)) => {
            eprintln!("dj-media-bench: {message}");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::Media(message)) => {
            eprintln!("dj-media-bench: {message}");
            ExitCode::from(3)
        }
    }
}

enum Error {
    Usage(String),
    Media(String),
}

fn run(argv: &[String]) -> Result<Vec<String>, Error> {
    let Some(mode) = argv.first() else {
        return Err(Error::Usage("no arguments given".to_string()));
    };
    if matches!(mode.as_str(), "--help" | "-h" | "help") {
        print!("{USAGE}");
        std::process::exit(0);
    }
    if matches!(mode.as_str(), "--version" | "-V") {
        println!("dj-media-bench {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut repeat: usize = 1;
    let mut window_seconds: f64 = 2.0;
    let mut seeks: usize = 50;
    let mut cache_dir: Option<PathBuf> = None;

    let mut index = 1;
    while index < argv.len() {
        let flag = argv[index].as_str();
        let value = || {
            argv.get(index + 1)
                .cloned()
                .ok_or_else(|| Error::Usage(format!("{flag} needs a value")))
        };
        match flag {
            "--file" => {
                files.push(PathBuf::from(value()?));
                index += 2;
            }
            "--repeat" => {
                repeat = value()?
                    .parse()
                    .map_err(|_| Error::Usage("--repeat expects a number".to_string()))?;
                index += 2;
            }
            "--count" => {
                seeks = value()?
                    .parse()
                    .map_err(|_| Error::Usage("--count expects a number".to_string()))?;
                index += 2;
            }
            "--cache-dir" => {
                cache_dir = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--window-seconds" => {
                window_seconds = value()?
                    .parse()
                    .map_err(|_| Error::Usage("--window-seconds expects a number".to_string()))?;
                index += 2;
            }
            other => return Err(Error::Usage(format!("unknown option {other:?}"))),
        }
    }

    if files.is_empty() {
        return Err(Error::Usage("at least one --file is required".to_string()));
    }

    let mut lines = Vec::new();
    for file in &files {
        let line = match mode.as_str() {
            "decode" => measure_decode(file, repeat)?,
            "load" => measure_load(file, window_seconds)?,
            "seek" => measure_seek(file, seeks)?,
            "cache" => measure_cache(file, cache_dir.as_deref())?,
            other => return Err(Error::Usage(format!("unknown mode {other:?}"))),
        };
        lines.push(line.to_string());
    }
    Ok(lines)
}

fn header(file: &std::path::Path, info: &dj_codec::MediaInfo) -> serde_json::Value {
    json!({
        "file": file.display().to_string(),
        "format": info.format.as_str(),
        "lossy": info.format.is_lossy(),
        "sample_rate": info.sample_rate,
        "channels": info.channels,
        "frames": info.frames,
        "duration_s": info.duration_seconds(),
        "gapless": info.gapless.map(|g| json!({ "delay": g.delay, "padding": g.padding })),
    })
}

fn measure_decode(file: &std::path::Path, repeat: usize) -> Result<serde_json::Value, Error> {
    let mut best: Option<(f64, u64, u64, u64)> = None;
    let mut info = None;

    for _ in 0..repeat.max(1) {
        let mut decoder = StreamDecoder::open(file)
            .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
        info = Some(decoder.info().clone());
        let rate = f64::from(decoder.sample_rate());
        let mut block = vec![0.0f32; BLOCK_FRAMES * 2];
        let mut frames: u64 = 0;
        let started = Instant::now();
        loop {
            let read = decoder
                .read_frames(&mut block)
                .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
            if read == 0 {
                break;
            }
            frames += read as u64;
        }
        let wall = started.elapsed().as_secs_f64();
        let audio_seconds = frames as f64 / rate;
        let speedup = audio_seconds / wall.max(1e-9);
        let stats = decoder.stats();
        let candidate = (speedup, frames, stats.decoder_calls, stats.corrupt_packets);
        if best.map(|(current, ..)| speedup > current).unwrap_or(true) {
            best = Some(candidate);
        }
    }

    let info = info.ok_or_else(|| Error::Media("no run produced info".to_string()))?;
    let (speedup, frames, calls, corrupt) =
        best.ok_or_else(|| Error::Media("no run completed".to_string()))?;
    let mut report = header(file, &info);
    if let Some(object) = report.as_object_mut() {
        object.insert("mode".to_string(), json!("decode"));
        object.insert("frames_decoded".to_string(), json!(frames));
        object.insert("speedup_x_realtime".to_string(), json!(speedup));
        object.insert("decoder_calls".to_string(), json!(calls));
        object.insert("corrupt_packets".to_string(), json!(corrupt));
    }
    Ok(report)
}

fn measure_load(file: &std::path::Path, window_seconds: f64) -> Result<serde_json::Value, Error> {
    let info = {
        let decoder = StreamDecoder::open(file)
            .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
        decoder.info().clone()
    };

    let config = PrefetchConfig::for_sample_rate(info.sample_rate, window_seconds);
    let started = Instant::now();
    let mut prefetcher = Prefetcher::open(file, config)
        .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
    let ready = prefetcher
        .wait_ready(config.window_frames, std::time::Duration::from_secs(30))
        .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
    let elapsed = started.elapsed().as_secs_f64();
    let stats = prefetcher.stats();

    let mut report = header(file, &info);
    if let Some(object) = report.as_object_mut() {
        object.insert("mode".to_string(), json!("load"));
        object.insert("window_frames".to_string(), json!(config.window_frames));
        object.insert("window_seconds".to_string(), json!(window_seconds));
        object.insert("ready_frames".to_string(), json!(ready));
        object.insert("t_ready_ms".to_string(), json!(elapsed * 1000.0));
        object.insert("decoder_calls".to_string(), json!(stats.decoder_calls));
        object.insert("eof".to_string(), json!(stats.eof));
    }
    Ok(report)
}

fn measure_seek(file: &std::path::Path, count: usize) -> Result<serde_json::Value, Error> {
    let mut decoder = StreamDecoder::open(file)
        .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
    let info = decoder.info().clone();
    let total = info.frames.unwrap_or(0);
    if total < 10_000 {
        return Err(Error::Media(format!(
            "{}: too short for a seek measurement ({total} frames)",
            file.display()
        )));
    }

    let mut block = vec![0.0f32; BLOCK_FRAMES * 2];
    let mut timings = Vec::with_capacity(count);
    // Deterministic spread of targets: no RNG, so two runs (and two machines) compare.
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    for _ in 0..count.max(1) {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let target = state % total.saturating_sub(1).max(1);
        let started = Instant::now();
        decoder
            .seek(target)
            .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
        let read = decoder
            .read_frames(&mut block)
            .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
        if read == 0 {
            continue;
        }
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }

    timings.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let at = |fraction: f64| -> f64 {
        if timings.is_empty() {
            return 0.0;
        }
        let last = timings.len() - 1;
        let index = ((last as f64) * fraction).round() as usize;
        timings[index.min(last)]
    };

    let mut report = header(file, &info);
    if let Some(object) = report.as_object_mut() {
        object.insert("mode".to_string(), json!("seek"));
        object.insert("seeks".to_string(), json!(timings.len()));
        object.insert("p50_ms".to_string(), json!(at(0.5)));
        object.insert("p99_ms".to_string(), json!(at(0.99)));
        object.insert(
            "max_ms".to_string(),
            json!(timings.last().copied().unwrap_or(0.0)),
        );
        object.insert(
            "note".to_string(),
            json!("includes landing after a fresh format seek and one decoded block; not the in-window path"),
        );
    }
    Ok(report)
}

/// Time to build a PCM cache entry (the load cost of ADR-0008), and the entry size.
fn measure_cache(
    file: &std::path::Path,
    dir: Option<&std::path::Path>,
) -> Result<serde_json::Value, Error> {
    let info = StreamDecoder::open(file)
        .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?
        .info()
        .clone();

    let root = match dir {
        Some(path) => path.to_path_buf(),
        None => std::env::temp_dir().join("dj-media-bench-cache"),
    };
    let config = CacheConfig {
        enabled: true,
        storage: CacheStorage::Disk,
        max_bytes: 64 * 1_000_000_000,
        sample_format: PcmSampleFormat::I16,
    };
    let mut cache =
        PcmCache::new(root, config).map_err(|err| Error::Media(format!("cache: {err}")))?;

    let cold_started = Instant::now();
    let cold = cache
        .prepare(file)
        .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
    let cold_ms = cold_started.elapsed().as_secs_f64() * 1000.0;

    let warm_started = Instant::now();
    let warm = cache
        .prepare(file)
        .map_err(|err| Error::Media(format!("{}: {err}", file.display())))?;
    let warm_ms = warm_started.elapsed().as_secs_f64() * 1000.0;

    let key = PcmCache::key_for(file, &info, cache.config())
        .map_err(|err| Error::Media(format!("{err}")))?;
    let cached_frames = cache.frames(&key);

    let mut report = header(file, &info);
    if let Some(object) = report.as_object_mut() {
        object.insert("mode".to_string(), json!("cache"));
        object.insert("cold_outcome".to_string(), json!(format!("{cold:?}")));
        object.insert("warm_outcome".to_string(), json!(format!("{warm:?}")));
        object.insert("t_build_ms".to_string(), json!(cold_ms));
        object.insert("t_hit_ms".to_string(), json!(warm_ms));
        object.insert("entry_bytes".to_string(), json!(cache.size_bytes()));
        object.insert("cached_frames".to_string(), json!(cached_frames));
        object.insert(
            "cache_root".to_string(),
            json!(cache.config().storage == CacheStorage::Disk),
        );
    }
    Ok(report)
}
