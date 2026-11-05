//! Two (or more) decks rendered together, headlessly (A03 acceptance).
//!
//! A mix scenario names files, a gain per deck, and commands bound to audio-clock samples. The
//! renderer cuts every block at the next command, so a command takes effect exactly at the sample
//! it names — the same rule the RT scheduler follows (A01), checked here without a sound card.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dj_codec::{CacheStorage, PcmCache, PrefetchConfig, Prefetcher, StreamDecoder};
use dj_deck::Deck;
use dj_domain::{DeckId, FrameRange, TrackMeta};
use serde::Deserialize;

/// Default block size driven through the decks.
pub const DEFAULT_BLOCK_FRAMES: usize = 256;
/// Default prefetch window.
pub const DEFAULT_WINDOW_SECONDS: f64 = 2.0;

/// How long a deck may take to become ready before the run gives up.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// One mix run.
#[derive(Debug, Clone, Deserialize)]
pub struct MixScenario {
    /// Sample rate the render is produced at; files must match it (resampling is A04).
    pub sample_rate: u32,
    /// Frames to render.
    pub frames: u64,
    /// Block size handed to the decks.
    #[serde(default = "default_block_frames")]
    pub block_frames: usize,
    /// Prefetch window per deck.
    #[serde(default = "default_window_seconds")]
    pub window_seconds: f64,
    /// Decks and their files.
    pub decks: Vec<DeckSpec>,
    /// Commands, each bound to an audio-clock sample.
    #[serde(default)]
    pub commands: Vec<MixCommand>,
}

/// A deck and the file it plays.
#[derive(Debug, Clone, Deserialize)]
pub struct DeckSpec {
    /// Which deck.
    pub id: DeckId,
    /// File to play.
    pub file: PathBuf,
    /// Channel gain, linear.
    #[serde(default = "unity")]
    pub gain: f32,
}

/// One command in a mix scenario.
#[derive(Debug, Clone, Deserialize)]
pub struct MixCommand {
    /// Audio-clock sample (frames rendered so far) at which it applies.
    pub at: u64,
    /// Target deck.
    pub deck: DeckId,
    /// What to do.
    pub action: MixAction,
    /// Frame argument for `Seek` and the hotcue actions.
    #[serde(default)]
    pub frame: Option<u64>,
    /// Loop start.
    #[serde(default)]
    pub start: Option<u64>,
    /// Loop end.
    #[serde(default)]
    pub end: Option<u64>,
    /// Hotcue slot, 0-based.
    #[serde(default)]
    pub slot: Option<usize>,
}

/// Deck actions a scenario can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum MixAction {
    /// Start the transport.
    Play,
    /// Stop the transport.
    Pause,
    /// Seek to `frame`.
    Seek,
    /// Activate a loop from `start` to `end`.
    LoopOn,
    /// Clear the loop.
    LoopOff,
    /// Store the current position in `slot`.
    HotcueStore,
    /// Jump to `slot`.
    HotcueJump,
    /// Delete `slot`.
    HotcueDelete,
}

/// What one deck did during the run.
#[derive(Debug, Clone)]
pub struct DeckReport {
    /// Deck id, as a short string.
    pub deck: &'static str,
    /// Preparation state at the end.
    pub load: String,
    /// Final source position.
    pub position: u64,
    /// Underruns counted by the deck.
    pub underruns: u64,
    /// Loop wraps.
    pub loop_wraps: u64,
    /// Hotcue jumps.
    pub hotcue_jumps: u64,
    /// Time from open to ready, milliseconds.
    pub ready_ms: f64,
}

/// Result of a mix render.
#[derive(Debug, Clone)]
pub struct MixReport {
    /// Interleaved stereo output.
    pub samples: Vec<f32>,
    /// Per-deck state.
    pub decks: Vec<DeckReport>,
    /// Commands that were applied.
    pub applied: usize,
}

/// Render a mix scenario into interleaved stereo.
pub fn render(scenario: &MixScenario, cache: Option<&mut PcmCache>) -> Result<MixReport, String> {
    if scenario.decks.is_empty() {
        return Err("a mix scenario needs at least one deck".to_string());
    }
    if scenario.frames == 0 {
        return Err("frames must be greater than zero".to_string());
    }

    let block = scenario.block_frames.max(1);
    let mut decks: Vec<(DeckId, Deck)> = Vec::with_capacity(scenario.decks.len());
    let mut reports = Vec::with_capacity(scenario.decks.len());

    let mut cache = cache;
    for spec in &scenario.decks {
        let opened = Instant::now();
        let info = StreamDecoder::open(&spec.file)
            .map_err(|err| format!("{}: {err}", spec.file.display()))?
            .info()
            .clone();
        if info.sample_rate != scenario.sample_rate {
            return Err(format!(
                "{}: {} Hz, but the scenario renders at {} Hz (resampling is A04)",
                spec.file.display(),
                info.sample_rate,
                scenario.sample_rate
            ));
        }

        let config = PrefetchConfig::for_sample_rate(info.sample_rate, scenario.window_seconds);
        let mut prefetcher = match cache.as_deref_mut() {
            Some(cache) => Prefetcher::open_with_cache(&spec.file, config, cache)
                .map_err(|err| format!("{}: {err}", spec.file.display()))?,
            None => Prefetcher::open(&spec.file, config)
                .map_err(|err| format!("{}: {err}", spec.file.display()))?,
        };
        prefetcher
            .wait_ready(config.window_frames, READY_TIMEOUT)
            .map_err(|err| format!("{}: {err}", spec.file.display()))?;

        let track = TrackMeta {
            title: spec
                .file
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_string(),
            artist: String::new(),
            duration_frames: info.frames,
            bpm: None,
            key: None,
        };

        let mut deck = Deck::new(spec.id);
        deck.attach(Box::new(prefetcher), Some(track));
        deck.set_gain(spec.gain);

        reports.push(DeckReport {
            deck: deck_name(spec.id),
            load: format!("{:?}", deck.load_state()),
            position: 0,
            underruns: 0,
            loop_wraps: 0,
            hotcue_jumps: 0,
            ready_ms: opened.elapsed().as_secs_f64() * 1000.0,
        });
        decks.push((spec.id, deck));
    }

    let total = usize::try_from(scenario.frames).map_err(|_| "frames do not fit".to_string())?;
    let mut out = vec![0.0f32; total * 2];
    let mut deck_block = vec![0.0f32; block * 2];

    let mut commands: Vec<&MixCommand> = scenario.commands.iter().collect();
    commands.sort_by_key(|command| command.at);
    let mut next_command = 0usize;
    let mut applied = 0usize;
    let mut written = 0usize;

    while written < total {
        // Commands due exactly here apply before the next sample is rendered.
        while let Some(command) = commands.get(next_command) {
            if command.at > written as u64 {
                break;
            }
            apply(command, &mut decks)?;
            applied += 1;
            next_command += 1;
        }

        let boundary = commands
            .get(next_command)
            .map_or(total as u64, |command| command.at);
        let until = boundary.min((written + block) as u64).min(total as u64);
        let count = usize::try_from(until.saturating_sub(written as u64))
            .unwrap_or(0)
            .max(1);
        let count = count.min(total - written);

        for (_, deck) in decks.iter_mut() {
            deck.render(&mut deck_block[..count * 2]);
            for (index, sample) in deck_block[..count * 2].iter().enumerate() {
                out[written * 2 + index] += *sample;
            }
        }
        written += count;
    }

    for (report, (_, deck)) in reports.iter_mut().zip(decks.iter()) {
        report.load = format!("{:?}", deck.load_state());
        report.position = deck.position();
        let stats = deck.stats();
        report.underruns = stats.underruns;
        report.loop_wraps = stats.loop_wraps;
        report.hotcue_jumps = stats.hotcue_jumps;
    }

    Ok(MixReport {
        samples: out,
        decks: reports,
        applied,
    })
}

fn apply(command: &MixCommand, decks: &mut [(DeckId, Deck)]) -> Result<(), String> {
    let Some((_, deck)) = decks.iter_mut().find(|(id, _)| *id == command.deck) else {
        return Err(format!(
            "command targets an unknown deck {:?}",
            command.deck
        ));
    };
    match command.action {
        MixAction::Play => {
            if !deck.play() {
                return Err(format!("deck {} is not loaded", deck_name(command.deck)));
            }
        }
        MixAction::Pause => deck.pause(),
        MixAction::Seek => {
            let frame = command
                .frame
                .ok_or_else(|| "Seek needs a frame".to_string())?;
            deck.seek(frame);
        }
        MixAction::LoopOn => {
            let start = command
                .start
                .ok_or_else(|| "LoopOn needs a start".to_string())?;
            let end = command
                .end
                .ok_or_else(|| "LoopOn needs an end".to_string())?;
            let region = FrameRange::new(start, end)
                .ok_or_else(|| format!("loop {start}..{end} is empty or inverted"))?;
            deck.set_loop(Some(region));
        }
        MixAction::LoopOff => deck.set_loop(None),
        MixAction::HotcueStore => {
            let slot = command.slot.unwrap_or(0);
            if !deck.store_hotcue(slot) {
                return Err(format!("hotcue slot {slot} does not exist"));
            }
        }
        MixAction::HotcueJump => {
            let slot = command.slot.unwrap_or(0);
            if deck.jump_hotcue(slot).is_none() {
                return Err(format!("hotcue slot {slot} is empty"));
            }
        }
        MixAction::HotcueDelete => {
            deck.delete_hotcue(command.slot.unwrap_or(0));
        }
    }
    Ok(())
}

/// Per-deck cache used by the `mix` mode when `--cache-dir` is given.
pub fn cache_for(root: PathBuf) -> Result<PcmCache, String> {
    PcmCache::new(
        root,
        dj_codec::CacheConfig {
            enabled: true,
            storage: CacheStorage::Disk,
            ..dj_codec::CacheConfig::default()
        },
    )
    .map_err(|err| err.to_string())
}

/// Short deck name for reports.
pub fn deck_name(id: DeckId) -> &'static str {
    match id {
        DeckId::A => "A",
        DeckId::B => "B",
        DeckId::C => "C",
        DeckId::D => "D",
    }
}

fn unity() -> f32 {
    1.0
}

fn default_block_frames() -> usize {
    DEFAULT_BLOCK_FRAMES
}

fn default_window_seconds() -> f64 {
    DEFAULT_WINDOW_SECONDS
}
