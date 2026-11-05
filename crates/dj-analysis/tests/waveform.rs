//! The waveform envelope and its cache, checked against real files (A07).

use std::path::{Path, PathBuf};

use dj_analysis::{Fingerprint, OVERVIEW_FRAMES_PER_BUCKET, Peaks, WaveformCache, analyse};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dj-codec/tests/fixtures/audio")
        .join(name)
}

fn temp_cache(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-analysis-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_envelope_describes_the_file_at_a_fixed_timebase() {
    let path = fixture("tone-1s-44100.flac");
    let peaks = analyse(&path, 512).unwrap_or_else(|err| panic!("{err}"));

    assert_eq!(peaks.frames_per_bucket, 512);
    assert_eq!(peaks.sample_rate, 44_100);
    // One second at 44.1 kHz in buckets of 512 frames: 86 full buckets and a short last one.
    assert!(
        (86..=87).contains(&peaks.buckets.len()),
        "one second is 86 buckets of 512 frames, got {}",
        peaks.buckets.len()
    );
    assert!(
        peaks.peak() > 0.3,
        "the fixture is a tone, not silence: peak {}",
        peaks.peak()
    );
    // A steady tone has the same extremes in every bucket: a flat envelope, not a decaying one.
    let first = peaks.buckets[0];
    let middle = peaks.buckets[peaks.buckets.len() / 2];
    assert!(
        (first.max - middle.max).abs() < 0.15,
        "{first:?} vs {middle:?}"
    );
}

#[test]
fn the_timebase_maps_a_playhead_to_the_right_column() {
    let path = fixture("tone-1s-44100.flac");
    let peaks = analyse(&path, 512).unwrap_or_else(|err| panic!("{err}"));

    assert_eq!(peaks.bucket_for(0), 0);
    assert_eq!(peaks.bucket_for(511), 0);
    assert_eq!(
        peaks.bucket_for(512),
        1,
        "the boundary belongs to the next bucket"
    );
    assert_eq!(
        peaks.bucket_for(1_000_000),
        peaks.buckets.len() - 1,
        "past the end clamps to the last column instead of panicking"
    );
    assert_eq!(peaks.frames(), peaks.buckets.len() as u64 * 512);
}

#[test]
fn silence_is_flat_and_a_tone_is_not() {
    let path = fixture("silence-then-tone-2s-44100.flac");
    let peaks = analyse(&path, 1024).unwrap_or_else(|err| panic!("{err}"));
    let first_half = &peaks.buckets[..peaks.buckets.len() / 4];
    let second_half = &peaks.buckets[peaks.buckets.len() * 3 / 4..];
    let quiet = first_half
        .iter()
        .map(|bucket| bucket.min.abs().max(bucket.max.abs()))
        .fold(0.0f32, f32::max);
    let loud = second_half
        .iter()
        .map(|bucket| bucket.min.abs().max(bucket.max.abs()))
        .fold(0.0f32, f32::max);
    assert!(
        quiet < 0.01,
        "the start of the fixture is silent, got {quiet}"
    );
    assert!(loud > 0.3, "the tail is a tone, got {loud}");
}

#[test]
fn the_cache_returns_what_it_stored() {
    let path = fixture("tone-1s-44100.flac");
    let root = temp_cache("round-trip");
    let cache = WaveformCache::new(root.clone());

    let (computed, from_cache) = cache
        .get_or_analyse(&path, OVERVIEW_FRAMES_PER_BUCKET)
        .unwrap_or_else(|err| panic!("{err}"));
    assert!(!from_cache, "the first call has to decode");

    let (cached, from_cache) = cache
        .get_or_analyse(&path, OVERVIEW_FRAMES_PER_BUCKET)
        .unwrap_or_else(|err| panic!("{err}"));
    assert!(from_cache, "the second call must come from the cache");
    // The envelope is stored as 16-bit extremes, so a round trip is close, not bit-identical: one
    // step of i16 is 1/32767, and a renderer cannot show the difference.
    assert_eq!(computed.buckets.len(), cached.buckets.len());
    for (index, (before, after)) in computed
        .buckets
        .iter()
        .zip(cached.buckets.iter())
        .enumerate()
    {
        assert!(
            (before.min - after.min).abs() < 1.0 / 30_000.0
                && (before.max - after.max).abs() < 1.0 / 30_000.0,
            "bucket {index} changed by more than the storage step: {before:?} vs {after:?}"
        );
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_changed_fingerprint_invalidates_the_cache() {
    // The acceptance criterion of A07: a re-encoded or replaced file must not be drawn with the old
    // envelope, because the old envelope describes different audio.
    let source = fixture("tone-1s-44100.flac");
    let dir = temp_cache("fingerprint");
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("{err}"));
    let copy = dir.join("track.flac");
    std::fs::copy(&source, &copy).unwrap_or_else(|err| panic!("{err}"));

    let cache = WaveformCache::new(dir.join("cache"));
    let (first, from_cache) = cache
        .get_or_analyse(&copy, OVERVIEW_FRAMES_PER_BUCKET)
        .unwrap_or_else(|err| panic!("{err}"));
    assert!(!from_cache);

    let before = Fingerprint::of(&copy).unwrap_or_else(|err| panic!("{err}"));

    // Replace the audio with silence of the same length: same name, different fingerprint.
    let silenced = dir.join("silence.flac");
    std::fs::copy(fixture("silence-then-tone-2s-44100.flac"), &silenced)
        .unwrap_or_else(|err| panic!("{err}"));
    std::fs::copy(&silenced, &copy).unwrap_or_else(|err| panic!("{err}"));
    std::fs::remove_file(&silenced).ok();
    let after = Fingerprint::of(&copy).unwrap_or_else(|err| panic!("{err}"));
    assert_ne!(before, after, "the fixture copy must look like a new file");

    let (second, from_cache) = cache
        .get_or_analyse(&copy, OVERVIEW_FRAMES_PER_BUCKET)
        .unwrap_or_else(|err| panic!("{err}"));
    assert!(
        !from_cache,
        "a stale entry must not be served after the file changed"
    );
    assert_ne!(
        first.buckets.len(),
        second.buckets.len(),
        "the new file is two seconds long, the old one second: the envelope must follow"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_different_resolution_never_reuses_another_ones_entry() {
    let path = fixture("tone-1s-44100.flac");
    let root = temp_cache("resolution");
    let cache = WaveformCache::new(root.clone());

    let (coarse, _) = cache
        .get_or_analyse(&path, 512)
        .unwrap_or_else(|err| panic!("{err}"));
    let (fine, from_cache) = cache
        .get_or_analyse(&path, 128)
        .unwrap_or_else(|err| panic!("{err}"));

    assert!(!from_cache, "a finer envelope is a different product");
    assert_eq!(fine.frames_per_bucket, 128);
    assert_eq!(coarse.frames_per_bucket, 512);
    // Four times the detail, minus the roundings of the last, partial bucket: the envelopes describe
    // the same audio, and neither invents columns for frames that do not exist.
    // The coarse envelope covers whole buckets, so its frame count is rounded up past the end of the
    // file; the fine one must still reach the file's own length, within one coarse bucket of slack.
    let coarse_columns = coarse.buckets.len();
    assert!(
        fine.buckets.len() * 4 + 3 >= coarse_columns * 4,
        "the fine envelope must reach the end of the file: {} columns for {coarse_columns} coarse",
        fine.buckets.len()
    );
    assert!(
        fine.buckets.len() <= coarse_columns * 4,
        "and must not exceed four times the columns: {} > {}",
        fine.buckets.len(),
        coarse_columns * 4
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_empty_envelope_is_harmless() {
    let empty = Peaks {
        frames_per_bucket: 512,
        sample_rate: 48_000,
        buckets: Vec::new(),
    };
    assert_eq!(empty.frames(), 0);
    assert_eq!(empty.peak(), 0.0);
    assert_eq!(empty.bucket_for(9_999), 0);
}
