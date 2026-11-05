//! Live output check (A05): plays a deck through the mixer into the system output.
//!
//! This is the first time the engine makes a sound outside a WAV file, and it is deliberately the
//! smallest thing that can: one deck, one channel strip, the master bus to the default device.
//!
//!   dj-live-probe                    # 10 s of a 440 Hz tone through the mixer
//!   dj-live-probe --file track.mp3   # the same, but playing a real file
//!
//! The report says what the callback actually did — frames per callback, interval percentiles,
//! peaks per channel — because "I heard something" is not a measurement.

use std::path::PathBuf;
use std::process::ExitCode;

use dj_audio_io::{LiveConfig, LiveOutput, Renderer, RoutePlan};
use dj_codec::{PrefetchConfig, Prefetcher, StreamDecoder};
use dj_deck::Deck;
use dj_domain::{DeckId, TrackMeta};
use dj_mixer::{ChannelSettings, CrossAssign, Mixer};
use serde_json::json;

const USAGE: &str = "\
dj-live-probe — play through the system output and report the callback

Usage:
  dj-live-probe [--file PATH] [--device SUBSTR] [--seconds N] [--block N]
                [--rate HZ] [--tone-hz HZ] [--gain G] [--cache-dir DIR]

Defaults: default output device, 10 s, block 256, tone 440 Hz, gain 0.25.
Without --file a pure tone is played, so the device path can be checked alone.

Exit codes: 0 ok, 2 bad usage, 3 device or media error.
";

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(Error::Usage(message)) => {
            eprintln!("dj-live-probe: {message}");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::Device(message)) => {
            eprintln!("dj-live-probe: {message}");
            ExitCode::from(3)
        }
    }
}

enum Error {
    Usage(String),
    Device(String),
}

struct Options {
    file: Option<PathBuf>,
    device: Option<String>,
    seconds: f64,
    block: usize,
    rate: Option<u32>,
    tone_hz: f32,
    gain: f32,
    cache_dir: Option<PathBuf>,
}

fn run(argv: &[String]) -> Result<String, Error> {
    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        std::process::exit(0);
    }
    if argv.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("dj-live-probe {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    let mut options = Options {
        file: None,
        device: None,
        seconds: 10.0,
        block: 256,
        rate: None,
        tone_hz: 440.0,
        gain: 0.25,
        cache_dir: None,
    };

    let mut index = 0;
    while index < argv.len() {
        let flag = argv[index].as_str();
        let value = || {
            argv.get(index + 1)
                .cloned()
                .ok_or_else(|| Error::Usage(format!("{flag} needs a value")))
        };
        let number = |what: &str| -> Result<f64, Error> {
            let raw = value()?;
            raw.parse::<f64>()
                .map_err(|_| Error::Usage(format!("{what} expects a number, got {raw:?}")))
        };
        match flag {
            "--file" => {
                options.file = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--device" => {
                options.device = Some(value()?);
                index += 2;
            }
            "--cache-dir" => {
                options.cache_dir = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--seconds" => {
                options.seconds = number("--seconds")?;
                index += 2;
            }
            "--block" => {
                options.block = number("--block")? as usize;
                index += 2;
            }
            "--rate" => {
                options.rate = Some(number("--rate")? as u32);
                index += 2;
            }
            "--tone-hz" => {
                options.tone_hz = number("--tone-hz")? as f32;
                index += 2;
            }
            "--gain" => {
                options.gain = number("--gain")? as f32;
                index += 2;
            }
            other => return Err(Error::Usage(format!("unknown option {other:?}"))),
        }
        if options.block == 0 {
            return Err(Error::Usage("--block must be positive".to_string()));
        }
    }

    // The device decides the width, and its preferred rate is the one to ask for: the point of the
    // default path is "play through whatever the system uses", not "play through the S4".
    let shape = dj_audio_io::resolve_device(options.device.as_deref())
        .map_err(|err| Error::Device(err.to_string()))?;
    let rate = options.rate.unwrap_or(shape.sample_rate);
    let plan = RoutePlan::for_channels(shape.channels);

    let renderer: Box<dyn Renderer> = match &options.file {
        Some(path) => {
            let probe = StreamDecoder::open(path)
                .map_err(|err| Error::Device(format!("{}: {err}", path.display())))?;
            let info = probe.info().clone();
            drop(probe);

            let mut prefetcher = match &options.cache_dir {
                Some(dir) => {
                    let mut cache =
                        dj_codec::PcmCache::new(dir.clone(), dj_codec::CacheConfig::default())
                            .map_err(|err| Error::Device(format!("cache: {err}")))?;
                    Prefetcher::open_with_cache(
                        path,
                        PrefetchConfig::for_sample_rate(info.sample_rate, 2.0),
                        &mut cache,
                    )
                    .map_err(|err| Error::Device(format!("{}: {err}", path.display())))?
                }
                None => {
                    Prefetcher::open(path, PrefetchConfig::for_sample_rate(info.sample_rate, 2.0))
                        .map_err(|err| Error::Device(format!("{}: {err}", path.display())))?
                }
            };
            prefetcher
                .wait_ready(8_192, std::time::Duration::from_secs(10))
                .map_err(|err| Error::Device(format!("{}: {err}", path.display())))?;

            let mut deck = Deck::new(DeckId::A);
            deck.attach(
                Box::new(prefetcher),
                Some(TrackMeta {
                    title: path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("track")
                        .to_string(),
                    artist: String::new(),
                    duration_frames: info.frames,
                    bpm: None,
                    key: None,
                }),
            );
            // The audio callback must never wait for the prefetcher.
            deck.set_blocking(false);
            deck.set_gain(1.0);
            deck.play();

            Box::new(DeckRenderer::new(deck, rate, options.block, options.gain))
        }
        None => Box::new(ToneRenderer::new(options.tone_hz, rate, options.gain)),
    };

    let config = LiveConfig {
        device: options.device.clone(),
        sample_rate: rate,
        block_frames: options.block,
        seconds: options.seconds,
    };
    let output =
        LiveOutput::start(&config, plan, renderer).map_err(|err| Error::Device(err.to_string()))?;
    let report = output.finish(options.seconds);

    Ok(json!({
        "device": report.device,
        "channels": report.channels,
        "sample_rate": report.sample_rate,
        "block_frames_requested": report.block_frames_requested,
        "frames_per_callback": report.frames_per_callback,
        "block_honoured": report.honoured_block(),
        "callbacks": report.callbacks,
        "frames": report.frames,
        "audio_seconds": report.audio_seconds,
        "late_callbacks": report.late_callbacks,
        "early_callbacks": report.early_callbacks,
        "errors": report.errors,
        "interval_min_ms": report.interval_min_ms,
        "interval_p50_ms": report.interval_p50_ms,
        "interval_p99_ms": report.interval_p99_ms,
        "interval_max_ms": report.interval_max_ms,
        "peaks": report.peaks,
        "render_p50_us": report.render_p50_us,
        "render_p99_us": report.render_p99_us,
        "render_max_us": report.render_max_us,
        "render_budget": report.render_budget,
        "source": options.file.as_ref().map(|path| path.display().to_string()),
        "route": {
            "master": [plan.master.0, plan.master.1],
            "cue": plan.cue.map(|(left, right)| [left, right]),
        },
        "note": "live run through the system output; not the reference rig (DJP-NFR-003 needs A11)",
    })
    .to_string())
}

/// A plain tone: proves the device path without involving media at all.
struct ToneRenderer {
    phase: f32,
    step: f32,
    gain: f32,
}

impl ToneRenderer {
    fn new(frequency: f32, sample_rate: u32, gain: f32) -> Self {
        Self {
            phase: 0.0,
            step: std::f32::consts::TAU * frequency / sample_rate.max(1) as f32,
            gain,
        }
    }
}

impl Renderer for ToneRenderer {
    fn render(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize) {
        cue.fill(0.0);
        for frame in 0..frames {
            let value = self.phase.sin() * self.gain;
            self.phase = (self.phase + self.step) % std::f32::consts::TAU;
            master[frame * 2] = value;
            master[frame * 2 + 1] = value;
        }
    }
}

/// Deck → channel strip → master. The smallest real signal path the app has.
struct DeckRenderer {
    deck: Deck,
    mixer: Mixer,
    block: usize,
    scratch: Vec<f32>,
}

impl DeckRenderer {
    fn new(deck: Deck, sample_rate: u32, block: usize, gain: f32) -> Self {
        let mut mixer = Mixer::new(1);
        mixer.prepare(sample_rate, block);
        mixer.set_channel(
            0,
            ChannelSettings {
                trim_db: 0.0,
                fader: gain.clamp(0.0, 1.0),
                assign: CrossAssign::Thru,
                cue: false,
            },
        );
        Self {
            deck,
            mixer,
            block,
            scratch: vec![0.0f32; block * 2],
        }
    }
}

impl Renderer for DeckRenderer {
    fn render(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize) {
        // A block larger than prepared is truncated, not allocated for: allocation in the callback
        // is the one thing the RT contract forbids outright.
        let frames = frames.min(self.block);
        self.deck.render(&mut self.scratch[..frames * 2]);
        self.mixer.begin_block(frames);
        self.mixer.mix_channel(0, &self.scratch[..frames * 2]);
        self.mixer.finish_block();
        master[..frames * 2].copy_from_slice(self.mixer.master());
        master[frames * 2..].fill(0.0);
        cue.fill(0.0);
    }
}
