//! Deck behaviour: transport, loop boundaries, hotcues, display state (A03).

use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::{Deck, DeckSource};
use dj_domain::{DeckId, DeckLoad, FrameRange, SourceFrame, TrackMeta};

/// A source whose sample value encodes its own frame number, so any position is verifiable.
struct RampSource {
    frames: u64,
    position: u64,
}

impl RampSource {
    fn new(frames: u64) -> Self {
        Self {
            frames,
            position: 0,
        }
    }
}

fn ramp(frame: u64) -> f32 {
    ((frame % 20_000) as f32 / 20_000.0) * 0.5
}

impl DeckSource for RampSource {
    fn sample_rate(&self) -> u32 {
        44_100
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for index in 0..take {
            let value = ramp(self.position + index as u64);
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }
}

fn deck_with_ramp(frames: u64) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    // The ramp is 44.1 kHz material on a deck that plays 44.1 kHz: these tests are about transport and
    // loops, not about rate conversion, and a conversion nobody asked for would move every position.
    deck.set_output_rate(44_100);
    deck.attach(Box::new(RampSource::new(frames)), None);
    deck
}

/// A deck that plays a 48 kHz device's rate, holding whatever source the test attaches.
fn deck_at_48k(frames: u64, source_rate: u32) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(48_000);
    deck.attach(
        Box::new(ToneAtRate {
            frames,
            position: 0,
            rate: source_rate,
            hertz: 1_000.0,
        }),
        None,
    );
    deck
}

#[test]
fn a_deck_is_silent_until_the_transport_runs() {
    let mut deck = deck_with_ramp(44_100);
    let mut block = vec![1.0f32; 512 * 2];
    deck.render(&mut block);
    assert!(
        block.iter().all(|sample| *sample == 0.0),
        "paused deck must be silent"
    );
    assert_eq!(deck.position(), 0);

    assert!(deck.play());
    deck.render(&mut block);
    assert!(block.iter().any(|sample| *sample != 0.0));
    assert_eq!(deck.position(), 512);
}

#[test]
fn a_loop_wraps_at_the_exact_sample() {
    let mut deck = deck_with_ramp(44_100);
    let region = FrameRange::new(1_000, 3_000).unwrap_or_else(|| panic!("region"));
    deck.set_loop(Some(region));
    deck.play();

    let mut block = vec![0.0f32; 4_000 * 2];
    deck.render(&mut block);

    // 0..3000 plays straight through, then the loop restarts at 1000.
    assert!((block[0] - ramp(0)).abs() < 1e-6);
    assert!((block[(2_999) * 2] - ramp(2_999)).abs() < 1e-6);
    assert!(
        (block[3_000 * 2] - ramp(1_000)).abs() < 1e-6,
        "frame 3000 must be the loop start, got {}",
        block[3_000 * 2]
    );
    assert!((block[3_999 * 2] - ramp(1_999)).abs() < 1e-6);
    assert_eq!(deck.position(), 2_000);
    assert_eq!(deck.stats().loop_wraps, 1);
}

#[test]
fn switching_loop_mode_off_stops_the_copy_repeating() {
    // A loop played from a pad is captured the same way the button's loop is. Switching the mode off
    // must stop the copy repeating: it runs to its end, the track carries on from there, and the switch
    // costs no seek.
    let mut deck = deck_with_ramp(44_100);
    let region = FrameRange::new(1_000, 2_000).unwrap_or_else(|| panic!("region"));
    assert!(deck.set_pad_loop(0, Some(region)));
    deck.set_loop_mode(true);
    assert_eq!(
        deck.jump_pad(0),
        Some(1_000),
        "the pad takes the playhead to the loop start"
    );
    deck.play();

    // One pass fills the capture; the next wrap is served from memory.
    let mut pass = vec![0.0f32; 1_000 * 2];
    deck.render(&mut pass);
    assert!(deck.loop_is_buffered(), "the loop is playing from memory");
    assert_eq!(deck.position(), 1_000, "it wrapped once");

    let seeks_before = deck.stats().seeks;
    deck.set_loop_mode(false);
    let mut after = vec![0.0f32; 2_000 * 2];
    deck.render(&mut after);
    for frame in 0..2_000usize {
        let expected = ramp(1_000 + frame as u64);
        assert!(
            (after[frame * 2] - expected).abs() < 1e-6,
            "frame {frame}: the track must run on unbroken after the switch, expected {expected}, got {}",
            after[frame * 2]
        );
    }
    assert_eq!(
        deck.stats().seeks,
        seeks_before,
        "switching the mode off is not a seek"
    );
    assert_eq!(
        deck.position(),
        3_000,
        "the playhead ran past the loop's end"
    );
}

#[test]
fn loop_mode_repeats_every_saved_loop() {
    // A track can hold many loops. Loop mode is one switch for all of them: a playhead that comes to any
    // saved loop repeats that loop, wherever it was dropped.
    let mut deck = deck_with_ramp(44_100);
    let first = FrameRange::new(1_000, 2_000).unwrap_or_else(|| panic!("first"));
    let second = FrameRange::new(5_000, 6_000).unwrap_or_else(|| panic!("second"));
    assert!(deck.set_pad_loop(1, Some(first)));
    assert!(deck.set_pad_loop(2, Some(second)));
    deck.set_loop_mode(true);
    assert_eq!(deck.active_loops().len(), 2, "both saved loops are armed");

    // Dropped inside the second loop: it wraps at its own end into its own start.
    deck.seek(5_500);
    deck.play();
    let mut block = vec![0.0f32; 1_000 * 2];
    deck.render(&mut block);
    assert!((block[0] - ramp(5_500)).abs() < 1e-6);
    assert!(
        (block[500 * 2] - ramp(5_000)).abs() < 1e-6,
        "the second loop must repeat: expected {}, got {}",
        ramp(5_000),
        block[500 * 2]
    );
    assert!((block[999 * 2] - ramp(5_499)).abs() < 1e-6);

    // Dropped before the first loop: it plays up to that loop and repeats it, not the whole file.
    deck.seek(500);
    let mut early = vec![0.0f32; 2_000 * 2];
    deck.render(&mut early);
    assert!((early[1_499 * 2] - ramp(1_999)).abs() < 1e-6);
    assert!(
        (early[1_500 * 2] - ramp(1_000)).abs() < 1e-6,
        "the first loop must repeat too: expected {}, got {}",
        ramp(1_000),
        early[1_500 * 2]
    );

    // Switched off, nothing repeats.
    deck.set_loop_mode(false);
    assert!(deck.active_loops().is_empty());
    deck.seek(500);
    let mut free = vec![0.0f32; 2_000 * 2];
    deck.render(&mut free);
    assert!(
        (free[1_999 * 2] - ramp(2_499)).abs() < 1e-6,
        "with loop mode off the track runs on"
    );
}

#[test]
fn a_loop_armed_while_playing_inside_it_has_no_silent_head() {
    // Arming a loop from inside it starts the capture mid-region. If that partial capture counts as
    // progress, the wrap plays the uncaptured head of the buffer — silence — until it reaches the point
    // where the loop was switched on.
    let mut deck = deck_with_ramp(44_100);
    let region = FrameRange::new(1_000, 3_000).unwrap_or_else(|| panic!("region"));
    deck.seek(2_000);
    deck.set_loop(Some(region));
    deck.play();

    let mut block = vec![0.0f32; 1_500 * 2];
    deck.render(&mut block);

    assert!(
        (block[0] - ramp(2_000)).abs() < 1e-6,
        "the armed loop must not move the playhead"
    );
    assert!(
        (block[1_000 * 2] - ramp(1_000)).abs() < 1e-6,
        "the wrap must play the loop start, got {} (silence here means the buffer lied about itself)",
        block[1_000 * 2]
    );
    assert!((block[1_499 * 2] - ramp(1_499)).abs() < 1e-6);
}

#[test]
fn clearing_a_loop_is_inaudible() {
    // Switching a loop off inside it must be inaudible: no repeat of the tail already heard, no jump,
    // no click. The playhead finishes the copy it is inside and the decoder takes over where it stood.
    let mut deck = deck_with_ramp(44_100);
    let region = FrameRange::new(1_000, 3_000).unwrap_or_else(|| panic!("region"));
    deck.seek(1_000);
    deck.set_loop(Some(region));
    deck.play();

    // One pass through the loop fills the capture, so the next wrap is served from memory.
    let mut pass = vec![0.0f32; 2_000 * 2];
    deck.render(&mut pass);
    assert!(deck.loop_is_buffered(), "one pass must fill the capture");
    assert_eq!(deck.position(), 1_000, "the wrap returns to the loop start");

    let mut inside = vec![0.0f32; 500 * 2];
    deck.render(&mut inside);
    assert_eq!(deck.position(), 1_500);

    let seeks_before = deck.stats().seeks;
    deck.set_loop(None);
    assert_eq!(
        deck.position(),
        1_500,
        "clearing a loop must not move the playhead"
    );
    assert_eq!(
        deck.stats().seeks,
        seeks_before,
        "clearing a loop must not ask the decoder to seek: that request is the click the DJ hears"
    );

    // The next block crosses the loop's end: the copy runs out at 3_000 and the source carries on.
    let mut next = vec![0.0f32; 2_000 * 2];
    deck.render(&mut next);
    for frame in 0..2_000usize {
        let expected = ramp(1_500 + frame as u64);
        assert!(
            (next[frame * 2] - expected).abs() < 1e-6,
            "frame {frame}: the sound must run on unbroken, expected {expected}, got {}",
            next[frame * 2]
        );
    }
}

#[test]
fn a_loop_pad_takes_the_sound_with_it() {
    // The marker and the sound have to move together: a pad that arms a loop must move the decoder too,
    // otherwise the strip shows the loop start while the ears still hear the old position.
    let mut deck = deck_with_ramp(44_100);
    let region = FrameRange::new(10_000, 14_000).unwrap_or_else(|| panic!("region"));
    assert!(deck.set_pad_loop(4, Some(region)));
    deck.seek(30_000);

    let landed = deck
        .jump_pad(4)
        .unwrap_or_else(|| panic!("pad 4 holds a loop"));
    assert_eq!(
        landed, 10_000,
        "the pad took the playhead to the loop start"
    );
    assert_eq!(deck.position(), 10_000);

    deck.play();
    let mut block = vec![0.0f32; 64 * 2];
    deck.render(&mut block);
    assert!(
        (block[0] - ramp(10_000)).abs() < 1e-6,
        "the first sample after the press must come from the loop start, got {}",
        block[0]
    );
    assert!((block[63 * 2] - ramp(10_063)).abs() < 1e-6);
}

#[test]
fn a_hotcue_jumps_to_the_stored_frame() {
    let mut deck = deck_with_ramp(44_100);
    deck.seek(5_000);
    assert!(deck.store_hotcue(3));
    deck.seek(0);
    let landed = deck
        .jump_hotcue(3)
        .unwrap_or_else(|| panic!("hotcue stored"));
    assert_eq!(landed, 5_000);
    assert_eq!(deck.position(), 5_000);

    deck.play();
    let mut block = vec![0.0f32; 128 * 2];
    deck.render(&mut block);
    assert!((block[0] - ramp(5_000)).abs() < 1e-6);
    assert_eq!(deck.stats().hotcue_jumps, 1);
}

#[test]
fn eight_hotcues_are_independent() {
    let mut deck = deck_with_ramp(44_100);
    let targets: [u64; 8] = [100, 1_000, 2_000, 3_000, 10_000, 20_000, 30_000, 40_000];
    for (slot, frame) in targets.iter().enumerate() {
        deck.seek(*frame);
        assert!(deck.store_hotcue(slot));
    }
    for (slot, frame) in targets.iter().enumerate() {
        assert_eq!(deck.jump_hotcue(slot), Some(*frame));
    }
    assert_eq!(deck.hotcues().len(), 8);
    assert!(!deck.store_hotcue(8), "a ninth slot does not exist");
    assert!(deck.delete_hotcue(0));
    assert_eq!(deck.jump_hotcue(0), None);
}

#[test]
fn gain_scales_the_block() {
    let mut deck = deck_with_ramp(44_100);
    deck.play();
    deck.set_gain(0.5);
    let mut block = vec![0.0f32; 256 * 2];
    deck.render(&mut block);
    let expected = ramp(0) * 0.5;
    assert!((block[0] - expected).abs() < 1e-6);
    assert!(deck.snapshot().levels.peak_left <= 0.5 + 1e-6);
}

#[test]
fn a_snapshot_carries_everything_a_display_needs() {
    let mut deck = Deck::new(DeckId::B);
    let track = TrackMeta {
        title: "Test Track".to_string(),
        artist: "Nobody".to_string(),
        duration_frames: Some(44_100),
        bpm: Some(128.0),
        key: Some("8A".to_string()),
    };
    deck.attach(Box::new(RampSource::new(44_100)), Some(track));
    deck.seek(10_000);

    let snapshot = deck.snapshot();
    assert_eq!(snapshot.deck, DeckId::B);
    assert_eq!(snapshot.load, DeckLoad::Ready);
    assert_eq!(snapshot.source_frame, SourceFrame(10_000));
    assert_eq!(snapshot.remaining_frames, Some(34_100));
    let ratio = snapshot.position_ratio.unwrap_or_default();
    assert!((ratio - 10_000.0 / 44_100.0).abs() < 1e-6);
    assert_eq!(snapshot.hotcues.len(), 8);
    assert_eq!(
        snapshot.track.map(|track| track.title),
        Some("Test Track".to_string())
    );
    assert_eq!(snapshot.loop_region, None);
}

#[test]
fn a_failed_load_is_a_state_not_a_silent_empty_deck() {
    let mut deck = Deck::new(DeckId::C);
    deck.set_progress(0.4);
    assert!(matches!(deck.load_state(), DeckLoad::Loading { .. }));
    deck.fail("cannot decode: unsupported codec");
    match deck.load_state() {
        DeckLoad::Failed { message } => assert!(message.contains("unsupported codec")),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(!deck.play(), "a failed deck cannot be played");
}

#[test]
fn an_empty_read_is_an_underrun_not_a_panic() {
    struct Dry;
    impl DeckSource for Dry {
        fn sample_rate(&self) -> u32 {
            44_100
        }
        fn frames(&self) -> Option<u64> {
            Some(1_000)
        }
        fn read(&mut self, _out: &mut [f32]) -> Result<usize, String> {
            Ok(0)
        }
        fn seek(&mut self, frame: u64) -> Result<u64, String> {
            Ok(frame)
        }
    }

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(Dry), None);
    deck.play();
    let mut block = vec![0.7f32; 256 * 2];
    deck.render(&mut block);
    assert!(block.iter().all(|sample| *sample == 0.0));
    assert_eq!(deck.stats().underruns, 1);
}

#[test]
fn a_real_file_plays_through_the_prefetcher() {
    // The wiring test: dj-codec produces the frames, the deck decides which ones.
    let path = std::env::temp_dir().join(format!("dj-deck-{}.wav", std::process::id()));
    write_tone(&path);
    let prefetcher = Prefetcher::open(
        &path,
        PrefetchConfig {
            window_frames: 16_384,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open: {err}"));

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(prefetcher), None);
    assert!(deck.play());
    std::thread::sleep(std::time::Duration::from_millis(20));

    let mut block = vec![0.0f32; 4_096 * 2];
    deck.render(&mut block);
    let peak = block
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    assert!(peak > 0.1, "expected audio, peak was {peak}");
    assert!(deck.snapshot().remaining_frames.unwrap_or(0) > 0);
}

fn write_tone(path: &std::path::Path) {
    let rate = 44_100u32;
    let frames = rate;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 4).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 4).to_le_bytes());
    for frame in 0..frames {
        let value = 0.6 * (std::f32::consts::TAU * 440.0 * frame as f32 / rate as f32).sin();
        let sample = (value * 32_000.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|err| panic!("write: {err}"));
}

/// A source whose blocking seek is slow and whose deferred seek is instant.
///
/// It stands in for the prefetcher: outside the window a real seek spins until the decoder
/// answers, which is fine offline and unacceptable in the audio callback.
struct SlowSeekSource {
    position: u64,
    frames: u64,
    blocking_seeks: std::sync::Arc<std::sync::atomic::AtomicU64>,
    deferred_seeks: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl DeckSource for SlowSeekSource {
    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for sample in &mut out[..take * 2] {
            *sample = 0.25;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.blocking_seeks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(1_500));
        self.position = frame;
        Ok(frame)
    }

    fn request_seek(&mut self, frame: u64) -> Result<u64, String> {
        self.deferred_seeks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.position = frame;
        Ok(frame)
    }
}

#[test]
fn a_live_deck_never_waits_for_a_seek() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let blocking = Arc::new(AtomicU64::new(0));
    let deferred = Arc::new(AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: Arc::clone(&blocking),
        deferred_seeks: Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(source), None);
    // A short loop forces a wrap inside one block, which is where the deck seeks.
    let Some(region) = dj_domain::FrameRange::new(0, 200) else {
        panic!("a 0..200 loop region must be valid");
    };
    // Buffering off: this test is about the fallback that long loops take, where wrapping really
    // does ask the source for a seek.
    deck.set_loop_config(dj_deck::LoopConfig {
        buffer_max_seconds: 0.0,
    });
    deck.set_loop(Some(region));
    // The live path: the deck must ask, not wait.
    deck.set_blocking(false);
    assert!(deck.play());

    let started = std::time::Instant::now();
    let mut out = vec![0.0f32; 512];
    deck.render(&mut out);
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "a live render must not wait for a seek, took {elapsed:?}"
    );
    assert!(
        deferred.load(Ordering::Relaxed) >= 1,
        "the wrap must ask for a seek"
    );
    assert_eq!(
        blocking.load(Ordering::Relaxed),
        0,
        "the callback must never take the blocking path"
    );
}

#[test]
fn an_offline_deck_still_waits_for_a_seek() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let blocking = Arc::new(AtomicU64::new(0));
    let deferred = Arc::new(AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: Arc::clone(&blocking),
        deferred_seeks: Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(source), None);
    assert!(deck.play());
    deck.seek(1_000);

    // Offline rendering trades time for determinism, and that is deliberate.
    assert_eq!(blocking.load(Ordering::Relaxed), 1);
    assert_eq!(deferred.load(Ordering::Relaxed), 0);
    assert_eq!(deck.position(), 1_000);
}

#[test]
fn a_short_loop_plays_through_its_seam_without_a_gap() {
    // A loop that fits the capture budget must wrap by copying, not by asking the source for a
    // seek: that seek is where the audible block of silence came from (ADR-0009).
    let blocking = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let deferred = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: std::sync::Arc::clone(&blocking),
        deferred_seeks: std::sync::Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(source), None);
    // 9 600 frames is 200 ms at 48 kHz — a musical loop, well inside the default budget.
    let Some(region) = dj_domain::FrameRange::new(0, 9_600) else {
        panic!("a 0..9600 loop region must be valid");
    };
    deck.set_loop(Some(region));
    deck.set_blocking(false);
    assert!(deck.play());

    // 512 samples per render is 256 frames, so 60 renders cover the 9 600-frame loop with room
    // to wrap once.
    let mut out = vec![0.0f32; 512];
    for _ in 0..60 {
        deck.render(&mut out);
    }
    assert!(deck.loop_is_buffered(), "the loop must be captured");

    let wraps_before = deck.stats().loop_wraps;
    for _ in 0..200 {
        deck.render(&mut out);
    }

    assert_eq!(
        deck.stats().underruns,
        0,
        "a captured loop must not starve the deck at its seam"
    );
    assert!(
        deck.stats().loop_wraps > wraps_before,
        "the loop must actually have wrapped again"
    );
    assert_eq!(
        deferred.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "a captured loop must not ask the source for a seek"
    );
}

#[test]
fn a_loop_longer_than_the_budget_is_not_captured() {
    let blocking = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let deferred = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: std::sync::Arc::clone(&blocking),
        deferred_seeks: std::sync::Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(source), None);
    deck.set_loop_config(dj_deck::LoopConfig {
        buffer_max_seconds: 0.1,
    });
    // 48 000 frames is one second, ten times the budget.
    let Some(region) = dj_domain::FrameRange::new(0, 48_000) else {
        panic!("a 0..48000 loop region must be valid");
    };
    deck.set_loop(Some(region));
    deck.set_blocking(false);
    assert!(deck.play());

    let mut out = vec![0.0f32; 512];
    for _ in 0..100 {
        deck.render(&mut out);
    }

    assert!(
        !deck.loop_is_buffered(),
        "a loop over the budget must stay unbuffered rather than silently allocate"
    );
}

/// A pure tone, so the tempo stage can be measured in pitch and not only in position.
struct ToneSource {
    frames: u64,
    position: u64,
}

impl DeckSource for ToneSource {
    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for index in 0..take {
            let frame = self.position + index as u64;
            let value =
                (std::f64::consts::TAU * 440.0 * frame as f64 / 48_000.0).sin() as f32 * 0.5;
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }
}

fn tone_deck(frames: u64) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(48_000);
    deck.attach(
        Box::new(ToneSource {
            frames,
            position: 0,
        }),
        None,
    );
    deck
}

/// A tone at the rate the file was recorded at, so a test can ask what pitch comes out of the device.
struct ToneAtRate {
    frames: u64,
    position: u64,
    rate: u32,
    hertz: f64,
}

impl DeckSource for ToneAtRate {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for index in 0..take {
            let frame = self.position + index as u64;
            let value = (std::f64::consts::TAU * self.hertz * frame as f64 / f64::from(self.rate))
                .sin() as f32
                * 0.5;
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }
}

/// Zero crossings on the left channel: two per cycle, so the pitch is countable.
fn crossings(block: &[f32]) -> usize {
    let mut count = 0usize;
    let mut last = 0.0f32;
    for frame in block.chunks_exact(2) {
        let value = frame[0];
        if (last < 0.0 && value >= 0.0) || (last > 0.0 && value <= 0.0) {
            count += 1;
        }
        last = value;
    }
    count
}

#[test]
fn a_deck_at_its_own_speed_renders_the_same_bytes_as_before_the_tempo_stage() {
    // The golden render compares bytes: a tempo stage that touched the signal at unity would move every
    // baseline in the repository.
    let mut deck = deck_with_ramp(48_000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);
    let mut reference = vec![0.0f32; 512];
    deck_with_ramp(48_000);
    let mut other = deck_with_ramp(48_000);
    other.play();
    other.render(&mut reference);
    assert_eq!(block, reference);
    assert_eq!(deck.tempo_ratio(), 1.0);
}

#[test]
fn a_faster_tempo_plays_the_material_faster_and_says_so() {
    let mut deck = deck_with_ramp(48_000);
    deck.set_tempo(1.08);
    assert_eq!(deck.tempo_ratio(), 1.08);
    deck.play();
    // A thousand frames is two thousand samples: the block is interleaved stereo.
    let mut block = vec![0.0f32; 2000];
    deck.render(&mut block);
    // The playhead moved through eight per cent more material than it played.
    let played = deck.position() as f64;
    assert!(
        (played - 1080.0).abs() < 3.0,
        "1000 frames at 1.08 advanced the source to {played}"
    );
    // And the value at the end of the block is the material from the end of that advance, not the
    // material 1000 frames in: the ramp encodes its own frame number.
    let last = block[block.len() - 2] as f64;
    let expected = f64::from(ramp(1079));
    assert!(
        (last - expected).abs() < 1e-4,
        "the last frame played {last} instead of {expected}"
    );
}

#[test]
fn a_slower_tempo_stretches_the_material() {
    let mut deck = deck_with_ramp(48_000);
    deck.set_tempo(0.92);
    deck.play();
    let mut block = vec![0.0f32; 2000];
    deck.render(&mut block);
    let played = deck.position() as f64;
    assert!(
        (played - 920.0).abs() < 3.0,
        "1000 frames at 0.92 advanced the source to {played}"
    );
    let last = block[block.len() - 2] as f64;
    let expected = f64::from(ramp(919));
    assert!(
        (last - expected).abs() < 1e-4,
        "the last frame played {last} instead of {expected}"
    );
}

#[test]
fn the_pitch_follows_the_rate_as_on_a_turntable() {
    // Key lock is a separate task (DJP-DECK-008); this is the vinyl behaviour R0 asks for, and it is
    // measured rather than assumed: the same tone, eight per cent faster, crosses zero eight per cent
    // more often.
    let mut plain = tone_deck(480_000);
    plain.play();
    let mut reference = vec![0.0f32; 9600];
    plain.render(&mut reference);
    let baseline = crossings(&reference);

    let mut fast = tone_deck(480_000);
    fast.set_tempo(1.08);
    fast.play();
    let mut moved = vec![0.0f32; 9600];
    fast.render(&mut moved);
    let higher = crossings(&moved);

    let ratio = higher as f64 / baseline as f64;
    assert!(
        (ratio - 1.08).abs() < 0.04,
        "eight per cent faster gave {ratio:.3} times the crossings ({baseline} -> {higher})"
    );
}

#[test]
fn a_seek_drops_the_frames_carried_by_the_tempo_stage() {
    // Without this the first block after a seek would start with a fraction of the material from before
    // it — an audible tick at every jump, and a picture that disagrees with the sound.
    let mut deck = deck_with_ramp(48_000);
    deck.set_tempo(1.08);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);

    deck.seek(5_000);
    deck.render(&mut block);
    let first = block[0] as f64;
    let expected = f64::from(ramp(5_000));
    assert!(
        (first - expected).abs() < 1e-4,
        "the block after the seek started at {first} instead of {expected}"
    );
}

#[test]
fn a_loop_still_wraps_at_tempo() {
    let mut deck = deck_with_ramp(48_000);
    deck.set_tempo(1.05);
    deck.set_loop(Some(FrameRange {
        start: SourceFrame(1000),
        end: SourceFrame(2000),
    }));
    deck.seek(1000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    for _ in 0..20 {
        deck.render(&mut block);
    }
    let position = deck.position();
    assert!(
        (1000..2000).contains(&position),
        "the playhead left the loop while playing at tempo: {position}"
    );
    assert!(
        deck.stats().loop_wraps > 0,
        "twenty blocks at 1.05 did not wrap a 1000-frame loop"
    );
}

#[test]
fn a_pad_keeps_its_name_and_its_colour_separately() {
    // Renaming a pad must not wipe the colour the DJ chose for it, and the other way round: the two
    // halves are set independently and an absent half means "leave it alone" (DJP-DECK-003).
    let mut deck = deck_with_ramp(48_000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);
    deck.store_hotcue(4);

    assert!(deck.set_cue_meta(4, Some("Drop".to_string()), None));
    assert_eq!(deck.cue_labels()[4].as_deref(), Some("Drop"));
    assert_eq!(deck.cue_colors()[4], None);

    assert!(deck.set_cue_meta(4, None, Some("#5fb85f".to_string())));
    assert_eq!(
        deck.cue_labels()[4].as_deref(),
        Some("Drop"),
        "the name survived"
    );
    assert_eq!(deck.cue_colors()[4].as_deref(), Some("#5fb85f"));

    let state = deck.snapshot();
    assert_eq!(state.cue_labels[4].as_deref(), Some("Drop"));
    assert_eq!(state.cue_colors[4].as_deref(), Some("#5fb85f"));
    assert!(state.hotcues[4].is_some());

    // A new track carries its own pads.
    deck.attach(Box::new(RampSource::new(48_000)), None);
    assert_eq!(deck.cue_labels()[4], None);
    assert_eq!(deck.cue_colors()[4], None);
}

#[test]
fn a_new_track_does_not_inherit_the_old_pads() {
    // Loading B over A must not leave A's cues on B's pads: an inherited cue jumps into a position the
    // new track never had, and an inherited name labels a pad that holds nothing.
    let mut deck = deck_with_ramp(48_000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);
    deck.store_hotcue(3);
    deck.set_cue_meta(3, Some("Drop".to_string()), Some("#5fb85f".to_string()));
    assert!(deck.hotcues()[3].is_some());

    deck.attach(Box::new(RampSource::new(48_000)), None);
    assert_eq!(deck.hotcues()[3], None, "the pad outlived its track");
    assert_eq!(deck.cue_labels()[3], None);
    assert_eq!(deck.cue_colors()[3], None);
}

#[test]
fn a_forty_four_one_file_plays_at_its_own_speed_on_a_forty_eight_device() {
    // The defect this guards: 44.1 kHz material read as if it were 48 kHz plays 8.8 % fast, which the
    // ear reports as "a little faster than it should be" on every file, not as a broken one.
    let mut deck = deck_at_48k(480_000, 44_100);
    deck.play();
    // Two blocks of 4800 frames: a tenth of a second of device time each.
    let mut block = vec![0.0f32; 9_600];
    for _ in 0..2 {
        deck.render(&mut block);
    }
    let consumed = deck.position() as f64;
    let expected = 9_600.0 * 44_100.0 / 48_000.0;
    assert!(
        (consumed - expected).abs() < 3.0,
        "a fifth of a second of 44.1 kHz material consumed {consumed} source frames, expected {expected}"
    );
}

#[test]
fn a_forty_four_one_tone_keeps_its_pitch_on_a_forty_eight_device() {
    // Same tone, same half second, two rates: the device must hear the same number of cycles. Reading
    // 44.1 kHz as 48 kHz would multiply the pitch by 1.088.
    let mut native = deck_at_48k(480_000, 48_000);
    native.play();
    let mut reference = vec![0.0f32; 19_200];
    native.render(&mut reference);

    let mut foreign = deck_at_48k(480_000, 44_100);
    assert!(
        (foreign.rate_ratio() - 44_100.0 / 48_000.0).abs() < 1e-9,
        "the deck did not notice the file's rate: {}",
        foreign.rate_ratio()
    );
    foreign.play();
    let mut converted = vec![0.0f32; 19_200];
    foreign.render(&mut converted);

    let baseline = crossings(&reference);
    let measured = crossings(&converted);
    let ratio = measured as f64 / baseline as f64;
    assert!(
        (ratio - 1.0).abs() < 0.02,
        "a 1 kHz tone recorded at 44.1 kHz came out at {ratio:.3} times the pitch ({baseline} -> {measured})"
    );
}
