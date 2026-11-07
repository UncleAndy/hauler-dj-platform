//! Decks through the mixer, headlessly (A03 + A04).
//!
//! A mix scenario names files, channel settings (fader, crossfader side, cue), and commands bound
//! to audio-clock samples. The renderer cuts every block at the next command, renders each deck
//! into its own block, feeds the channel strips, and writes the master bus — the same order the RT
//! graph uses (DJP-ARCH-006), without a sound card.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dj_codec::{CacheStorage, PcmCache, PrefetchConfig, Prefetcher, StreamDecoder};
use dj_deck::Deck;
use dj_domain::{DeckId, FrameRange, TrackMeta};
use dj_mixer::{ChannelSettings, CrossAssign, CrossfaderCurve, Meter, Meters, Mixer};
use serde::Deserialize;

/// Default block size driven through the mixer.
pub const DEFAULT_BLOCK_FRAMES: usize = 256;
/// Default prefetch window.
pub const DEFAULT_WINDOW_SECONDS: f64 = 2.0;

const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// One mix run.
#[derive(Debug, Clone, Deserialize)]
pub struct MixScenario {
    /// Sample rate the render is produced at; files must match it (resampling is A04/M2).
    pub sample_rate: u32,
    /// Frames to render.
    pub frames: u64,
    /// Block size handed to the decks and the mixer.
    #[serde(default = "default_block_frames")]
    pub block_frames: usize,
    /// Prefetch window per deck.
    #[serde(default = "default_window_seconds")]
    pub window_seconds: f64,
    /// Initial crossfader position (0 = A, 1 = B); default 0.5.
    #[serde(default)]
    pub crossfader: Option<f32>,
    /// Crossfader law: `linear` or `constant` (default).
    #[serde(default)]
    pub curve: Option<String>,
    /// Decks and their files.
    pub decks: Vec<DeckSpec>,
    /// Commands, each bound to an audio-clock sample.
    #[serde(default)]
    pub commands: Vec<MixCommand>,
}

/// A deck and its channel strip settings.
#[derive(Debug, Clone, Deserialize)]
pub struct DeckSpec {
    /// Which deck.
    pub id: DeckId,
    /// File to play.
    pub file: PathBuf,
    /// Channel fader, 0 … 1 (default 1).
    #[serde(default)]
    pub fader: Option<f32>,
    /// Crossfader side: `A`, `B` or `thru` (default `thru`, i.e. unaffected).
    #[serde(default)]
    pub assign: Option<String>,
    /// Send the channel to the cue bus (default false).
    #[serde(default)]
    pub cue: Option<bool>,
}

/// One command in a mix scenario.
#[derive(Debug, Clone, Deserialize)]
pub struct MixCommand {
    /// Audio-clock sample at which it applies.
    pub at: u64,
    /// Target deck. Required for deck actions, ignored for mixer actions.
    #[serde(default)]
    pub deck: Option<DeckId>,
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
    /// Value for fader/master-gain actions.
    #[serde(default)]
    pub value: Option<f32>,
}

/// Deck and mixer actions a scenario can ask for.
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
    /// Set the channel fader to `value`.
    SetFader,
    /// Move the crossfader to `value` (0 = A, 1 = B).
    SetCrossfader,
    /// Send a channel to the cue bus.
    CueOn,
    /// Remove a channel from the cue bus.
    CueOff,
    /// Set the master gain to `value`.
    SetMasterGain,
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
    /// Channel fader at the end.
    pub fader: f32,
    /// Crossfader side.
    pub assign: &'static str,
    /// Whether the channel is cued.
    pub cue: bool,
    /// Channel meter of the last block.
    pub meter: Meter,
}

/// Result of a mix render.
#[derive(Debug, Clone)]
pub struct MixReport {
    /// Interleaved stereo master bus.
    pub samples: Vec<f32>,
    /// Cue bus, when the scenario asked for it.
    pub cue_samples: Option<Vec<f32>>,
    /// Per-deck state.
    pub decks: Vec<DeckReport>,
    /// Commands that were applied.
    pub applied: usize,
    /// Mixer metering of the whole run.
    pub meters: RunMeters,
}

/// Mixer figures accumulated over the run.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunMeters {
    /// Highest master peak seen.
    pub master_peak: f32,
    /// Samples over full scale before the limiter.
    pub clipped_samples: u64,
    /// Smallest limiter gain applied.
    pub limiter_gain: f32,
    /// Adapted non-finite input samples.
    pub non_finite_samples: u64,
}

/// Render a mix scenario.
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
    let mut mixer = Mixer::new(scenario.decks.len());
    mixer.prepare(scenario.sample_rate, block);
    if let Some(position) = scenario.crossfader {
        mixer.set_crossfader(position);
    }
    if let Some(curve) = scenario.curve.as_deref() {
        mixer.set_curve(match curve.to_ascii_lowercase().as_str() {
            "linear" => CrossfaderCurve::Linear,
            "constant" | "constant-power" => CrossfaderCurve::ConstantPower,
            other => return Err(format!("unknown crossfader curve {other:?}")),
        });
    }

    let mut cache = cache;
    for (index, spec) in scenario.decks.iter().enumerate() {
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
        deck.set_gain(1.0);

        let assign = match spec.assign.as_deref() {
            Some(value) if value.eq_ignore_ascii_case("a") => CrossAssign::A,
            Some(value) if value.eq_ignore_ascii_case("b") => CrossAssign::B,
            Some(value) if value.eq_ignore_ascii_case("thru") => CrossAssign::Thru,
            Some(other) => return Err(format!("unknown crossfader assignment {other:?}")),
            None => CrossAssign::Thru,
        };
        let fader = spec.fader.unwrap_or(1.0);
        let cue = spec.cue.unwrap_or(false);
        mixer.set_channel(
            index,
            ChannelSettings {
                trim_db: 0.0,
                fader,
                assign,
                cue,
            },
        );

        reports.push(DeckReport {
            deck: deck_name(spec.id),
            load: format!("{:?}", deck.load_state()),
            position: 0,
            underruns: 0,
            loop_wraps: 0,
            hotcue_jumps: 0,
            ready_ms: opened.elapsed().as_secs_f64() * 1000.0,
            fader,
            assign: assign_name(assign),
            cue,
            meter: Meter::default(),
        });
        decks.push((spec.id, deck));
    }

    let total = usize::try_from(scenario.frames).map_err(|_| "frames do not fit".to_string())?;
    let mut master = vec![0.0f32; total * 2];
    let mut cue = vec![0.0f32; total * 2];
    let mut deck_block = vec![0.0f32; block * 2];

    let mut commands: Vec<&MixCommand> = scenario.commands.iter().collect();
    commands.sort_by_key(|command| command.at);
    let mut next_command = 0usize;
    let mut applied = 0usize;
    let mut written = 0usize;
    let mut run = RunMeters {
        limiter_gain: 1.0,
        ..RunMeters::default()
    };

    while written < total {
        while let Some(command) = commands.get(next_command) {
            if command.at > written as u64 {
                break;
            }
            apply(command, &mut decks, &mut mixer)?;
            applied += 1;
            next_command += 1;
        }

        let boundary = commands
            .get(next_command)
            .map_or(total as u64, |command| command.at);
        let until = boundary.min((written + block) as u64).min(total as u64);
        let count = usize::try_from(until.saturating_sub(written as u64))
            .unwrap_or(0)
            .max(1)
            .min(total - written);

        mixer.begin_block(count);
        for (index, (_, deck)) in decks.iter_mut().enumerate() {
            deck.render(&mut deck_block[..count * 2]);
            mixer.mix_channel(index, &deck_block[..count * 2]);
        }
        mixer.finish_block();

        master[written * 2..(written + count) * 2].copy_from_slice(mixer.master());
        cue[written * 2..(written + count) * 2].copy_from_slice(mixer.cue());

        accumulate(&mut run, mixer.meters());
        for (index, report) in reports.iter_mut().enumerate() {
            if let Some(meter) = mixer.meters().channels.get(index) {
                report.meter.peak = report.meter.peak.max(meter.peak);
                report.meter.rms = report.meter.rms.max(meter.rms);
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
    for (index, report) in reports.iter_mut().enumerate() {
        if let Some(settings) = mixer.channel(index) {
            report.fader = settings.fader;
            report.assign = assign_name(settings.assign);
            report.cue = settings.cue;
        }
    }

    Ok(MixReport {
        samples: master,
        cue_samples: Some(cue),
        decks: reports,
        applied,
        meters: run,
    })
}

fn accumulate(run: &mut RunMeters, meters: &Meters) {
    run.master_peak = run.master_peak.max(meters.master.peak);
    run.clipped_samples += meters.clipped_samples;
    run.non_finite_samples += meters.non_finite_samples;
    if meters.limiter_gain < run.limiter_gain {
        run.limiter_gain = meters.limiter_gain;
    }
}

fn apply(
    command: &MixCommand,
    decks: &mut [(DeckId, Deck)],
    mixer: &mut Mixer,
) -> Result<(), String> {
    // Mixer-wide actions first: they do not need a deck.
    match command.action {
        MixAction::SetCrossfader => {
            let value = command
                .value
                .ok_or_else(|| "SetCrossfader needs a value".to_string())?;
            mixer.set_crossfader(value);
            return Ok(());
        }
        MixAction::SetMasterGain => {
            let value = command
                .value
                .ok_or_else(|| "SetMasterGain needs a value".to_string())?;
            mixer.set_master_gain(value);
            return Ok(());
        }
        _ => {}
    }

    let deck_id = command
        .deck
        .ok_or_else(|| format!("{:?} needs a deck", command.action))?;
    let Some(index) = decks.iter().position(|(id, _)| *id == deck_id) else {
        return Err(format!("command targets an unknown deck {deck_id:?}"));
    };

    // Channel actions apply to the strip of that deck.
    match command.action {
        MixAction::SetFader => {
            let value = command
                .value
                .ok_or_else(|| "SetFader needs a value".to_string())?;
            let mut settings = mixer
                .channel(index)
                .copied()
                .ok_or_else(|| "channel strip missing".to_string())?;
            settings.fader = value;
            mixer.set_channel(index, settings);
            return Ok(());
        }
        MixAction::CueOn | MixAction::CueOff => {
            let mut settings = mixer
                .channel(index)
                .copied()
                .ok_or_else(|| "channel strip missing".to_string())?;
            settings.cue = command.action == MixAction::CueOn;
            mixer.set_channel(index, settings);
            return Ok(());
        }
        _ => {}
    }

    let Some((_, deck)) = decks.get_mut(index) else {
        return Err(format!("deck {deck_id:?} is missing"));
    };
    match command.action {
        MixAction::Play => {
            if !deck.play() {
                return Err(format!("deck {deck_id:?} is not loaded"));
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
            let _ = deck.set_loop(Some(region));
        }
        MixAction::LoopOff => {
            deck.set_loop(None);
        }
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
        MixAction::SetFader
        | MixAction::SetCrossfader
        | MixAction::CueOn
        | MixAction::CueOff
        | MixAction::SetMasterGain => unreachable!("handled above"),
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

fn assign_name(assign: CrossAssign) -> &'static str {
    match assign {
        CrossAssign::A => "A",
        CrossAssign::B => "B",
        CrossAssign::Thru => "thru",
    }
}

fn default_block_frames() -> usize {
    DEFAULT_BLOCK_FRAMES
}

fn default_window_seconds() -> f64 {
    DEFAULT_WINDOW_SECONDS
}
