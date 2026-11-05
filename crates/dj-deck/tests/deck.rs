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
    deck.attach(Box::new(RampSource::new(frames)), None);
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
