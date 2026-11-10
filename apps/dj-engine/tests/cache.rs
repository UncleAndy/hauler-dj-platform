//! The track cache in the engine: a loaded file is played from memory, and the memory outlives the file.
//!
//! The promise of the cache is not "less decoding" — it is that the playback path cannot be late. The only
//! honest witness of that is a deck that keeps playing while the file it came from is no longer there, so
//! that is what these tests do.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-engine-cache-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("create {}: {err}", dir.display()));
    dir
}

/// A stereo tone written as a WAV the engine can load, the same way its own tests make fixtures.
fn tone_wav(dir: &Path, name: &str, seconds: f32) -> PathBuf {
    let path = dir.join(format!("{name}.wav"));
    let rate: u32 = 48_000;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (seconds * rate as f32) as usize;
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        #[allow(clippy::cast_precision_loss)]
        let value =
            (std::f64::consts::TAU * 220.0 * frame as f64 / f64::from(rate)).sin() as f32 * 0.5;
        samples.push(value);
        samples.push(value);
    }
    let mut writer = std::fs::File::create(&path)
        .unwrap_or_else(|err| panic!("create {}: {err}", path.display()));
    dj_audio_core::wav::write_pcm16_interleaved(&mut writer, rate, 2, &samples)
        .unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    path
}

fn memory_cache(dir: &Path, lossless: bool) -> dj_codec::PcmCache {
    dj_codec::PcmCache::new(
        dir.join("pcm"),
        dj_codec::CacheConfig {
            enabled: true,
            lossless,
            storage: dj_codec::CacheStorage::Memory,
            max_bytes: 64 * 1024 * 1024,
            sample_format: dj_codec::PcmSampleFormat::F32,
        },
    )
    .unwrap_or_else(|err| panic!("cache: {err}"))
}

#[test]
fn a_track_the_cache_holds_keeps_playing_after_the_file_is_gone() {
    let dir = temp_dir("cached");
    let path = tone_wav(&dir, "tone", 3.0);
    let mut cache = memory_cache(&dir, true);

    // The production order: the deck starts on the stream, the entry is asked for behind it.
    let streamed = dj_engine::prepare_track(&path, 48_000, 2.0, 0.7, None)
        .unwrap_or_else(|err| panic!("prepare: {err}"));
    assert!(streamed.cache_key.is_none(), "a streamed load has no key");
    let (source, _key) =
        dj_engine::cached_source(&path, &mut cache).expect("the cache takes a lossless file");
    assert_eq!(cache.len(), 1, "one entry for one file");
    assert!(cache.size_bytes() > 0, "the entry holds samples");

    // The material is in memory now: take the file away and keep reading. A streaming reader would stop
    // here — that is the whole difference the cache buys.
    let hidden = dir.join("gone.wav");
    std::fs::rename(&path, &hidden)
        .unwrap_or_else(|err| panic!("rename {}: {err}", path.display()));

    let mut source = source;
    let mut block = vec![0.0f32; 512];
    let mut audible = 0usize;
    for round in 0..200 {
        let read = source
            .read(&mut block)
            .unwrap_or_else(|err| panic!("read {round}: {err}"));
        assert!(read > 0, "the cache had more to give at block {round}");
        audible += block[..read * 2]
            .iter()
            .filter(|sample| sample.abs() > 1e-3)
            .count();
    }
    assert!(audible > 0, "the cached tone is audible, not silence");
}

#[test]
fn the_engine_asks_the_cache_only_when_the_policy_wants_lossless() {
    let dir = temp_dir("streamed");
    let path = tone_wav(&dir, "tone", 1.0);

    let mut refusing = memory_cache(&dir, false);
    assert!(
        dj_engine::cached_source(&path, &mut refusing).is_none(),
        "lossless is streamed unless the policy asks for it"
    );
    assert_eq!(refusing.len(), 0, "a refusal writes nothing");

    let mut asking = memory_cache(&dir, true);
    assert!(
        dj_engine::cached_source(&path, &mut asking).is_some(),
        "a policy that asks gets the entry"
    );
}
