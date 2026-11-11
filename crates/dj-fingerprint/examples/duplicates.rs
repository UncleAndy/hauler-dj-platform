//! Measures what a duplicate check would see: bit error rates between copies and between strangers,
//! and how long a fingerprint takes.
//!
//! Run it with the files to compare:
//!
//! ```text
//! cargo run -p dj-fingerprint --example duplicates -- a.wav b.mp3 c.ogg
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use dj_codec::{PrefetchConfig, Prefetcher};

/// How much of each file is fingerprinted, and from where in it.
const WINDOW_SECONDS: f64 = 40.0;
const WINDOW_START: f64 = 0.1;

fn fingerprint(path: &Path) -> (Vec<u32>, f64, u32) {
    let mut prefetcher = Prefetcher::open(
        path,
        PrefetchConfig {
            window_frames: 16_384,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open {}: {err}", path.display()));
    // The prefetcher fills its window on its own thread: reading before it has anything hands back zeros,
    // and a window of zeros fingerprints as nothing at all.
    prefetcher
        .wait_ready(8_192, std::time::Duration::from_secs(10))
        .unwrap_or_else(|err| panic!("ready {}: {err}", path.display()));
    let info = prefetcher.info();
    let rate = info.sample_rate;
    let channels = info.channels.max(1);
    let frames = info.frames.unwrap_or(u64::MAX);
    let skip = (frames as f64 * WINDOW_START) as u64;

    // Read from the front and drop what comes before the window: a seek into a file that has just been
    // opened returns before the decoder has anything, and a window of silence fingerprints as nothing.
    let wanted = (WINDOW_SECONDS * f64::from(rate)) as usize;
    let mut samples: Vec<i16> = Vec::with_capacity(wanted * usize::from(channels));
    let mut skipped = 0_u64;
    let mut block = vec![0.0_f32; 4_096 * usize::from(channels)];
    while samples.len() < wanted * usize::from(channels) {
        let read = prefetcher.read_frames(&mut block);
        if read == 0 {
            // A starved read is not the end of the file: the decoder is behind, and the stream is still
            // coming. Only the prefetcher's own end marker ends the loop.
            if prefetcher.stats().eof {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }
        let per_channel = usize::from(channels);
        let mut from = 0_usize;
        if skipped < skip {
            let room = (skip - skipped).min(read as u64) as usize;
            skipped += room as u64;
            from = room;
        }
        for sample in &block[from * per_channel..read * per_channel] {
            samples.push((sample.clamp(-1.0, 1.0) * 32_767.0) as i16);
        }
    }
    let loudest = samples
        .iter()
        .map(|sample| sample.unsigned_abs())
        .max()
        .unwrap_or(0);
    if loudest == 0 {
        panic!("{}: the window came back silent", path.display());
    }

    let started = Instant::now();
    let print = dj_fingerprint::of_samples(&samples, rate, channels)
        .unwrap_or_else(|err| panic!("fingerprint {}: {err}", path.display()));
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    (print, elapsed, rate)
}

fn main() {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.len() < 2 {
        eprintln!("usage: duplicates <file> <file> [file…]");
        std::process::exit(2);
    }

    let mut prints = Vec::new();
    for path in &paths {
        let (fingerprint, millis, rate) = fingerprint(path);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("?");
        println!(
            "{name}: {} frames of fingerprint from {WINDOW_SECONDS} s at {rate} Hz, {millis:.1} ms",
            fingerprint.len()
        );
        if fingerprint.is_empty() {
            panic!("{name}: the fingerprinter produced nothing");
        }
        prints.push((name.to_string(), fingerprint));
    }

    println!();
    for (index, (name, print)) in prints.iter().enumerate() {
        for (other_name, other) in prints.iter().skip(index + 1) {
            let agreement = dj_fingerprint::agreement(print, other, 24);
            println!(
                "{:>10.4} bits differ at lag {:>3}   {name}  ~  {other_name}",
                agreement.ber, agreement.lag
            );
        }
    }
}
