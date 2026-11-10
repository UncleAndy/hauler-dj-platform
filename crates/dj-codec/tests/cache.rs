//! PCM cache behaviour (ADR-0008): built once, keyed by content, evicted by use, pinned by decks.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use dj_codec::{
    CacheConfig, CacheOutcome, CacheStorage, PcmCache, PcmSampleFormat, PrefetchConfig, Prefetcher,
    StreamDecoder,
};

use common::{fixture, peak, temp_path};

fn cache_root(name: &str) -> PathBuf {
    let root = temp_path(name);
    std::fs::create_dir_all(&root).unwrap_or_else(|err| panic!("create {}: {err}", root.display()));
    root
}

fn config(storage: CacheStorage, max_bytes: u64) -> CacheConfig {
    CacheConfig {
        enabled: true,
        lossless: false,
        storage,
        max_bytes,
        sample_format: PcmSampleFormat::I16,
    }
}

#[test]
fn a_lossless_file_is_cached_only_when_the_policy_asks() {
    // Lossless can be streamed with an index, so caching it is a choice: the engine makes that choice to
    // keep whole tracks in memory. The default policy leaves lossless alone.
    let file = fixture("tone-1s-44100.flac");
    let mut refusing = PcmCache::new(
        cache_root("lossless-off"),
        config(CacheStorage::Memory, 1_000_000_000),
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    assert_eq!(
        refusing
            .prepare(&file)
            .unwrap_or_else(|err| panic!("prepare: {err}")),
        CacheOutcome::NotNeeded,
        "the default policy streams lossless files"
    );

    let mut asking = PcmCache::new(
        cache_root("lossless-on"),
        CacheConfig {
            lossless: true,
            ..config(CacheStorage::Memory, 1_000_000_000)
        },
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    assert_eq!(
        asking
            .prepare(&file)
            .unwrap_or_else(|err| panic!("prepare: {err}")),
        CacheOutcome::Built,
        "a policy that asks for lossless gets it"
    );
    let info = StreamDecoder::open(&file)
        .unwrap_or_else(|err| panic!("open: {err}"))
        .info()
        .clone();
    let key =
        PcmCache::key_for(&file, &info, asking.config()).unwrap_or_else(|err| panic!("key: {err}"));
    let mut reader = asking
        .open_reader(&key)
        .unwrap_or_else(|err| panic!("reader: {err}"));
    let mut block = vec![0.0f32; 512];
    let read = reader
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(read > 0, "the entry holds audio");
    assert!(
        block[..read * 2].iter().any(|sample| *sample != 0.0),
        "the cached tone is not silence"
    );
}

#[test]
fn an_mp3_is_built_once_then_reused() {
    let mut cache = PcmCache::new(
        cache_root("reuse"),
        config(CacheStorage::Disk, 1_000_000_000),
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    let file = fixture("tone-1s-44100-cbr.mp3");

    assert_eq!(
        cache
            .prepare(&file)
            .unwrap_or_else(|err| panic!("prepare: {err}")),
        CacheOutcome::Built
    );
    assert_eq!(cache.len(), 1);
    let size = cache.size_bytes();
    assert!(
        size > 100_000,
        "one second of stereo i16 is about 176 KB, got {size}"
    );

    assert_eq!(
        cache
            .prepare(&file)
            .unwrap_or_else(|err| panic!("prepare again: {err}")),
        CacheOutcome::Hit
    );
    assert_eq!(cache.len(), 1, "a hit must not add a second entry");
}

#[test]
fn the_entry_plays_the_same_audio_as_the_decoder() {
    let mut cache = PcmCache::new(
        cache_root("same-audio"),
        config(CacheStorage::Disk, 1_000_000_000),
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    let file = fixture("tone-1s-44100-cbr.mp3");
    cache
        .prepare(&file)
        .unwrap_or_else(|err| panic!("prepare: {err}"));

    let info = StreamDecoder::open(&file)
        .unwrap_or_else(|err| panic!("open: {err}"))
        .info()
        .clone();
    let key =
        PcmCache::key_for(&file, &info, cache.config()).unwrap_or_else(|err| panic!("key: {err}"));
    let mut reader = cache
        .open_reader(&key)
        .unwrap_or_else(|err| panic!("reader: {err}"));
    assert_eq!(reader.frames_expected(), info.frames);

    let mut decoder = StreamDecoder::open(&file).unwrap_or_else(|err| panic!("decoder: {err}"));
    let mut direct = vec![0.0f32; 4096 * 2];
    let frames = decoder
        .read_frames(&mut direct)
        .unwrap_or_else(|err| panic!("read: {err}"));
    let mut cached = vec![0.0f32; 4096 * 2];
    let cached_frames = reader
        .read_frames(&mut cached)
        .unwrap_or_else(|err| panic!("cached read: {err}"));

    assert_eq!(frames, cached_frames, "the entry must hold the same frames");
    let tolerance = 2.0 / 32_767.0;
    for index in 0..frames * 2 {
        let diff = (direct[index] - cached[index]).abs();
        assert!(diff <= tolerance, "sample {index} differs by {diff}");
    }
    assert!(peak(&cached[..cached_frames * 2]) > 0.4);
}

#[test]
fn seeking_inside_the_entry_is_exact_and_cheap() {
    let mut cache = PcmCache::new(
        cache_root("seek"),
        config(CacheStorage::Disk, 1_000_000_000),
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    let file = fixture("tone-1s-44100-cbr.mp3");
    cache
        .prepare(&file)
        .unwrap_or_else(|err| panic!("prepare: {err}"));
    let info = StreamDecoder::open(&file)
        .unwrap_or_else(|err| panic!("open: {err}"))
        .info()
        .clone();
    let key =
        PcmCache::key_for(&file, &info, cache.config()).unwrap_or_else(|err| panic!("key: {err}"));
    let mut reader = cache
        .open_reader(&key)
        .unwrap_or_else(|err| panic!("reader: {err}"));

    let landed = reader
        .seek(22_050)
        .unwrap_or_else(|err| panic!("seek: {err}"));
    assert_eq!(landed, 22_050, "a cache seek is exact by construction");
    let mut block = vec![0.0f32; 1024 * 2];
    let frames = reader
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert_eq!(frames, 1024);
    assert_eq!(reader.position(), 22_050 + 1024);
    assert!(peak(&block) > 0.1);
}

#[test]
fn a_reader_keeps_playing_after_its_entry_is_evicted() {
    // The engine does not pin whole tracks; it relies on a reader owning its data. That makes eviction
    // safe for a deck that is already playing, and this is the witness of it.
    let root = cache_root("evict-reader");
    let first = fixture("tone-1s-44100.flac");
    let second = fixture("drums-8s-48000.wav");

    let mut sizing = PcmCache::new(
        root.clone(),
        CacheConfig {
            lossless: true,
            ..config(CacheStorage::Memory, 1_000_000_000)
        },
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    sizing
        .prepare(&first)
        .unwrap_or_else(|err| panic!("prepare: {err}"));
    let small = sizing.size_bytes();
    sizing
        .prepare(&second)
        .unwrap_or_else(|err| panic!("prepare: {err}"));
    let big = sizing.size_bytes() - small;

    // Room for the big one and half of the small one: the small entry has to be dropped when the big
    // one arrives, and the big one must still fit.
    let budget = big + small / 2;
    assert!(
        big <= budget && small + big > budget,
        "the budget must hold the big entry and force the small one out ({small} + {big} against {budget})"
    );
    let mut cache = PcmCache::new(
        root,
        CacheConfig {
            lossless: true,
            ..config(CacheStorage::Memory, budget)
        },
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));

    cache
        .prepare(&first)
        .unwrap_or_else(|err| panic!("prepare first: {err}"));
    let info = StreamDecoder::open(&first)
        .unwrap_or_else(|err| panic!("open: {err}"))
        .info()
        .clone();
    let key =
        PcmCache::key_for(&first, &info, cache.config()).unwrap_or_else(|err| panic!("key: {err}"));
    let mut playing = cache
        .open_reader(&key)
        .unwrap_or_else(|err| panic!("reader: {err}"));
    let mut block = vec![0.0f32; 512];
    let heard = playing
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(heard > 0, "the reader had material before the eviction");

    cache
        .prepare(&second)
        .unwrap_or_else(|err| panic!("prepare second: {err}"));
    assert!(
        cache.frames(&key).is_none(),
        "the budget had to drop the small entry to keep the big one"
    );

    let mut after = vec![0.0f32; 512];
    let still = playing
        .read_frames(&mut after)
        .unwrap_or_else(|err| panic!("read after eviction: {err}"));
    assert!(
        still > 0,
        "a deck that is already playing keeps its material after its entry leaves the cache"
    );
    assert!(
        after[..still * 2].iter().any(|sample| sample.abs() > 0.0),
        "and the material is the audio, not silence"
    );
}

#[test]
fn the_limit_evicts_the_least_recently_used_and_spares_pinned_entries() {
    let root = cache_root("evict");
    let probe = fixture("tone-1s-44100-cbr.mp3");
    let mut sizing = PcmCache::new(root.clone(), config(CacheStorage::Disk, 1_000_000_000))
        .unwrap_or_else(|err| panic!("cache: {err}"));
    sizing
        .prepare(&probe)
        .unwrap_or_else(|err| panic!("prepare: {err}"));
    let one_entry = sizing.size_bytes();

    // Room for two entries only.
    let mut cache = PcmCache::new(
        root,
        config(CacheStorage::Disk, one_entry * 2 + one_entry / 2),
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    let first = probe.clone();
    let second = fixture("tone-1s-44100-vbr.mp3");
    cache
        .prepare(&first)
        .unwrap_or_else(|err| panic!("prepare first: {err}"));
    let first_info = StreamDecoder::open(&first)
        .unwrap_or_else(|err| panic!("open: {err}"))
        .info()
        .clone();
    let first_key = PcmCache::key_for(&first, &first_info, cache.config())
        .unwrap_or_else(|err| panic!("key: {err}"));
    cache.pin(&first_key);
    cache
        .prepare(&second)
        .unwrap_or_else(|err| panic!("prepare second: {err}"));
    assert_eq!(cache.len(), 2);

    cache
        .prepare(&fixture("tone-1s-44100.ogg"))
        .unwrap_or_else(|err| panic!("prepare third: {err}"));

    assert!(
        cache.frames(&first_key).is_some(),
        "a pinned entry must survive eviction"
    );
    assert!(cache.size_bytes() <= cache.config().max_bytes);
}

#[test]
fn a_lossless_file_is_not_cached() {
    let mut cache = PcmCache::new(
        cache_root("lossless"),
        config(CacheStorage::Disk, 1_000_000_000),
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    assert_eq!(
        cache
            .prepare(&fixture("tone-1s-44100.flac"))
            .unwrap_or_else(|err| panic!("prepare: {err}")),
        CacheOutcome::NotNeeded
    );
    assert!(cache.is_empty());
    assert_eq!(cache.size_bytes(), 0);
}

#[test]
fn a_corrupt_entry_is_dropped_on_scan_instead_of_being_played() {
    let root = cache_root("corrupt");
    let mut first = PcmCache::new(root.clone(), config(CacheStorage::Disk, 1_000_000_000))
        .unwrap_or_else(|err| panic!("cache: {err}"));
    first
        .prepare(&fixture("tone-1s-44100-cbr.mp3"))
        .unwrap_or_else(|err| panic!("prepare: {err}"));
    let entry = std::fs::read_dir(&root)
        .unwrap_or_else(|err| panic!("read_dir: {err}"))
        .flatten()
        .map(|item| item.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("pcm"))
        .unwrap_or_else(|| panic!("no cache entry written"));
    std::fs::write(&entry, b"not a cache entry").unwrap_or_else(|err| panic!("clobber: {err}"));

    let cache = PcmCache::new(root, config(CacheStorage::Disk, 1_000_000_000))
        .unwrap_or_else(|err| panic!("rescan: {err}"));
    assert!(
        cache.is_empty(),
        "a corrupt entry must not survive a rescan"
    );
    assert!(!entry.exists(), "the corrupt file should be removed");
}

#[test]
fn memory_storage_never_touches_the_disk() {
    let root = cache_root("memory");
    let mut cache = PcmCache::new(root.clone(), config(CacheStorage::Memory, 1_000_000_000))
        .unwrap_or_else(|err| panic!("cache: {err}"));
    let file = fixture("tone-1s-44100-cbr.mp3");
    assert_eq!(
        cache
            .prepare(&file)
            .unwrap_or_else(|err| panic!("prepare: {err}")),
        CacheOutcome::Built
    );
    assert_eq!(
        cache
            .prepare(&file)
            .unwrap_or_else(|err| panic!("prepare: {err}")),
        CacheOutcome::Hit
    );
    let info = StreamDecoder::open(&file)
        .unwrap_or_else(|err| panic!("open: {err}"))
        .info()
        .clone();
    let key =
        PcmCache::key_for(&file, &info, cache.config()).unwrap_or_else(|err| panic!("key: {err}"));
    let mut reader = cache
        .open_reader(&key)
        .unwrap_or_else(|err| panic!("reader: {err}"));
    let mut block = vec![0.0f32; 2048 * 2];
    let frames = reader
        .read_frames(&mut block)
        .unwrap_or_else(|err| panic!("read: {err}"));
    assert!(frames > 0);
    assert!(peak(&block) > 0.1);

    let written = std::fs::read_dir(&root).map(|dir| dir.count()).unwrap_or(0);
    assert_eq!(written, 0, "memory storage must not write files");
}

#[test]
fn a_prefetch_over_the_cache_plays_without_a_decoder() {
    let mut cache = PcmCache::new(
        cache_root("prefetch"),
        config(CacheStorage::Disk, 1_000_000_000),
    )
    .unwrap_or_else(|err| panic!("cache: {err}"));
    let file = fixture("tone-1s-44100-cbr.mp3");
    let mut prefetcher = Prefetcher::open_with_cache(
        &file,
        PrefetchConfig {
            window_frames: 16_384,
            block_frames: 1_024,
        },
        &mut cache,
    )
    .unwrap_or_else(|err| panic!("open_with_cache: {err}"));

    let ready = prefetcher
        .wait_ready(8_192, Duration::from_secs(5))
        .unwrap_or_else(|err| panic!("ready: {err}"));
    assert!(ready >= 8_192, "got {ready}");

    let mut block = vec![0.0f32; 4_096 * 2];
    let frames = prefetcher.read_frames(&mut block);
    assert_eq!(frames, 4_096);
    assert!(peak(&block) > 0.1);
    assert_eq!(
        prefetcher.stats().decoder_calls,
        0,
        "a cached source must not run the decoder at all"
    );
}
