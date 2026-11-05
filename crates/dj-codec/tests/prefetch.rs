//! The prefetch boundary: what the audio thread may and may not pay for (A02, ADR-0008).

mod common;

use std::time::Duration;

use dj_codec::{PrefetchConfig, Prefetcher, SeekOutcome, StreamDecoder};

use common::{fixture, peak, temp_path, write_wav};

#[test]
fn the_window_delivers_the_same_samples_as_the_decoder() {
    let path = temp_path("compare.wav");
    let frames = write_wav(&path, 44_100, 1.0, 0);

    let mut decoder = StreamDecoder::open(&path).unwrap_or_else(|err| panic!("open: {err}"));
    let mut reference = vec![0.0f32; frames as usize * 2];
    let read = decoder
        .read_frames(&mut reference)
        .unwrap_or_else(|err| panic!("read: {err}"));
    reference.truncate(read * 2);

    let mut prefetcher = Prefetcher::open(
        &path,
        PrefetchConfig {
            window_frames: 8_192,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open prefetch: {err}"));
    let ready = prefetcher
        .wait_ready(4_096, Duration::from_secs(5))
        .unwrap_or_else(|err| panic!("ready: {err}"));
    assert!(ready >= 4_096, "the window should hold several blocks, got {ready}");

    let mut out = vec![0.0f32; 4_096 * 2];
    let mut read = prefetcher.read_frames(&mut out);
    assert_eq!(read, 4_096);
    for index in 0..read * 2 {
        let diff = (out[index] - reference[index]).abs();
        assert!(diff < 1e-6, "sample {index} differs by {diff}");
    }

    let more = prefetcher.read_frames(&mut out);
    assert!(more > 0);
}

#[test]
fn a_seek_inside_the_window_never_calls_the_decoder() {
    let path = temp_path("inside.wav");
    write_wav(&path, 44_100, 1.0, 0);

    let mut prefetcher = Prefetcher::open(
        &path,
        PrefetchConfig {
            window_frames: 16_384,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open: {err}"));
    prefetcher
        .wait_ready(12_288, Duration::from_secs(5))
        .unwrap_or_else(|err| panic!("ready: {err}"));

    let before = prefetcher.stats();
    let frame_in_window = 1_024;
    match prefetcher.seek(frame_in_window) {
        Ok(SeekOutcome::InsideWindow) => {}
        other => panic!("a seek inside the window must not refill: {other:?}"),
    }
    let mut out = vec![0.0f32; 1_024 * 2];
    let read = prefetcher.read_frames(&mut out);
    assert_eq!(read, 1_024);

    let after = prefetcher.stats();
    assert_eq!(
        after.decoder_calls, before.decoder_calls,
        "the decoder must not run for a seek inside the window"
    );
    assert_eq!(after.seeks, before.seeks, "no producer seek should be issued");
    assert_eq!(after.starved_reads, before.starved_reads);
}

#[test]
fn a_seek_outside_the_window_refills_from_the_new_position() {
    let path = temp_path("outside.wav");
    write_wav(&path, 44_100, 1.0, 0);

    let mut prefetcher = Prefetcher::open(
        &path,
        PrefetchConfig {
            window_frames: 8_192,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open: {err}"));
    prefetcher
        .wait_ready(4_096, Duration::from_secs(5))
        .unwrap_or_else(|err| panic!("ready: {err}"));

    let outcome = prefetcher.seek(38_000).unwrap_or_else(|err| panic!("seek: {err}"));
    assert!(matches!(outcome, SeekOutcome::Refilled { .. }), "got {outcome:?}");

    let ready = prefetcher
        .wait_ready(1_024, Duration::from_secs(5))
        .unwrap_or_else(|err| panic!("ready after seek: {err}"));
    assert!(ready >= 1_024);
    let mut out = vec![0.0f32; 1_024 * 2];
    let read = prefetcher.read_frames(&mut out);
    assert_eq!(read, 1_024);
    assert!(peak(&out) > 0.1);
    assert!(prefetcher.stats().seeks >= 1);
}

#[test]
fn an_mp3_window_reports_the_format_and_reaches_the_end() {
    let mut prefetcher = Prefetcher::open(
        &fixture("tone-1s-44100-cbr.mp3"),
        PrefetchConfig {
            window_frames: 8_192,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open: {err}"));
    assert!(prefetcher.info().needs_pcm_cache());

    let mut out = vec![0.0f32; 1_024 * 2];
    let mut total = 0usize;
    for _ in 0..200 {
        let frames = prefetcher.read_frames(&mut out);
        if frames == 0 {
            if prefetcher.stats().eof && prefetcher.stats().buffered_frames == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        total += frames;
    }
    assert!(total > 40_000, "expected about a second of audio, got {total}");
}
