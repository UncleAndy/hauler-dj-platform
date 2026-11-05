//! Headless runner (A01): renders scripted command scenarios to WAV, so every audio
//! feature is checkable without Qt, a sound card or a controller (DJP-NFR-001).
//!
//! Two modes:
//! - `sine` — a pure test tone, the golden-file baseline;
//! - `scenario` — the tone plus commands bound to exact audio-clock samples.
//!
//! Status: **A01**. Decoding, decks and effects arrive in A02+; this runner keeps working
//! against whatever the engine implements.

mod mix;
mod scenario;

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::ExitCode;

use dj_audio_core::{
    Engine, TimingSummary, render_measured, render_offline, write_pcm16_interleaved,
    write_pcm16_mono,
};
use dj_domain::{Ack, AckStatus, DeckId, DjCommand, ScheduledCommand};
use scenario::Scenario;
use serde_json::json;

const USAGE: &str = "\
dj-headless — offline renderer and scenario runner

Usage:
  dj-headless sine --frames N [--rate HZ] [--frequency HZ] --out FILE
  dj-headless scenario --file S.json [--out FILE] [--block N]
  dj-headless mix --file MIX.json [--out FILE] [--cache-dir DIR]
  dj-headless --help | --version

Options:
  --frames N       frames to render (exact: the WAV has this many frames)
  --rate HZ        sample rate, default 48000
  --frequency HZ   test tone, default 440
  --file FILE      scenario JSON (see apps/dj-headless/tests/scenarios/)
  --out FILE       output WAV (scenario: defaults to the scenario's own path with .wav)
  --block N        block size driven through the callback, default 256
  --measure        also time every callback and report p50/p99/max
  --cache-dir DIR  use a PCM cache for lossy files (ADR-0008)
  --cue-out FILE   mix mode: also write the cue (headphone) bus

Exit codes: 0 ok, 2 bad usage, 3 I/O or parse error.
";

const EXIT_USAGE: u8 = 2;
const EXIT_IO: u8 = 3;

/// How many applied-command lines the report carries (the engine holds 64 acks).
const MAX_REPORTED_ACKS: usize = 64;

enum Action {
    Help,
    Version,
    Render(Render),
    Mix(MixRun),
}

/// A two-deck mix render (A03).
struct MixRun {
    path: PathBuf,
    out: PathBuf,
    cue_out: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
}

struct Render {
    scenario: Scenario,
    out: PathBuf,
    /// Start the transport at sample zero (the `sine` mode).
    transport_on: bool,
    /// Time every callback and include the percentiles in the report.
    measure: bool,
}

enum CliError {
    Usage(String),
    Io(String),
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match parse(&argv) {
        Ok(Action::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Action::Version) => {
            println!("dj-headless {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Action::Mix(run)) => match run_mix(run) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("dj-headless: {message}");
                ExitCode::from(EXIT_IO)
            }
        },
        Ok(Action::Render(render)) => match run(render) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("dj-headless: {message}");
                ExitCode::from(EXIT_IO)
            }
        },
        Err(CliError::Usage(message)) => {
            eprintln!("dj-headless: {message}");
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
        Err(CliError::Io(message)) => {
            eprintln!("dj-headless: {message}");
            ExitCode::from(EXIT_IO)
        }
    }
}

fn parse(argv: &[String]) -> Result<Action, CliError> {
    let Some(command) = argv.first() else {
        return Err(CliError::Usage("no arguments given".to_string()));
    };

    match command.as_str() {
        "--help" | "-h" | "help" => return Ok(Action::Help),
        "--version" | "-V" => return Ok(Action::Version),
        "sine" | "scenario" | "mix" => {}
        other => return Err(CliError::Usage(format!("unknown command {other:?}"))),
    }

    let mut frames: Option<u64> = None;
    let mut rate = scenario::DEFAULT_SAMPLE_RATE;
    let mut frequency = scenario::DEFAULT_FREQUENCY_HZ;
    let mut block: Option<usize> = None;
    let mut measure = false;
    let mut cache_dir: Option<PathBuf> = None;
    let mut cue_out: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut file: Option<PathBuf> = None;

    let mut index = 1;
    while index < argv.len() {
        let flag = argv[index].as_str();
        let value = || {
            argv.get(index + 1)
                .cloned()
                .ok_or_else(|| CliError::Usage(format!("{flag} needs a value")))
        };
        match flag {
            "--frames" => {
                frames = Some(number(&value()?, flag)?);
                index += 2;
            }
            "--rate" => {
                rate = number(&value()?, flag)?;
                index += 2;
            }
            "--frequency" => {
                frequency = number(&value()?, flag)?;
                index += 2;
            }
            "--block" => {
                block = Some(number(&value()?, flag)?);
                index += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--file" => {
                file = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--measure" => {
                measure = true;
                index += 1;
            }
            "--cache-dir" => {
                cache_dir = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--cue-out" => {
                cue_out = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--help" | "-h" => return Ok(Action::Help),
            other => return Err(CliError::Usage(format!("unknown option {other:?}"))),
        }
    }

    if command == "mix" {
        let path = file.ok_or_else(|| CliError::Usage("mix needs --file".into()))?;
        let out = out.unwrap_or_else(|| path.with_extension("wav"));
        return Ok(Action::Mix(MixRun {
            path,
            out,
            cue_out,
            cache_dir,
        }));
    }

    let render = match command.as_str() {
        "sine" => {
            let frames = frames.ok_or_else(|| CliError::Usage("sine needs --frames".into()))?;
            let out = out.ok_or_else(|| CliError::Usage("sine needs --out".into()))?;
            Render {
                scenario: Scenario::new(frames, rate, frequency),
                out,
                transport_on: true,
                measure,
            }
        }
        _ => {
            let file = file.ok_or_else(|| CliError::Usage("scenario needs --file".into()))?;
            let mut scenario = Scenario::from_path(&file).map_err(CliError::Io)?;
            if let Some(block) = block {
                scenario.block_frames = block;
            }
            if frames.is_some() || rate != scenario::DEFAULT_SAMPLE_RATE {
                return Err(CliError::Usage(
                    "a scenario file carries its own frames and rate; drop --frames/--rate"
                        .to_string(),
                ));
            }
            let out = out.unwrap_or_else(|| file.with_extension("wav"));
            Render {
                scenario,
                out,
                transport_on: false,
                measure,
            }
        }
    };

    if render.scenario.frames == 0 {
        return Err(CliError::Usage("frames must be greater than zero".into()));
    }
    Ok(Action::Render(render))
}

/// Percentiles as JSON, with the host caveat spelled out in the payload.
fn timing_json(timing: Option<TimingSummary>, scenario: &Scenario) -> serde_json::Value {
    match timing {
        None => serde_json::Value::Null,
        Some(summary) => json!({
            "blocks": summary.blocks,
            "p50_us": summary.p50_us,
            "p99_us": summary.p99_us,
            "max_us": summary.max_us,
            "period_us": summary.period_us,
            "p99_of_period": summary.p99_of_period,
            "max_of_period": summary.max_of_period,
            "block_frames": scenario.block_frames,
            "sample_rate": scenario.sample_rate,
            "note": "offline render on this host, not the reference rig (DJP-NFR-003 needs A11)",
        }),
    }
}

fn number<T: std::str::FromStr>(raw: &str, flag: &str) -> Result<T, CliError> {
    raw.parse::<T>()
        .map_err(|_| CliError::Usage(format!("{flag} expects a number, got {raw:?}")))
}

fn run(render: Render) -> Result<String, String> {
    let scenario = render.scenario;
    let frames = usize::try_from(scenario.frames)
        .map_err(|_| "frames do not fit in memory on this platform".to_string())?;

    let mut engine = Engine::new(scenario.sample_rate, scenario.frequency_hz);
    // `sine` renders a tone, not silence: the transport is on from sample zero.
    if render.transport_on {
        engine.submit(ScheduledCommand {
            id: 0,
            effective_sample: 0,
            command: DjCommand::Play {
                deck: DeckId::A,
                enabled: true,
            },
        });
    }

    let mut rejected = 0usize;
    for (position, entry) in scenario.commands.iter().enumerate() {
        let id = entry.id.unwrap_or(position as u64 + 1);
        let ack = engine.submit(ScheduledCommand {
            id,
            effective_sample: entry.at,
            command: entry.command.clone(),
        });
        if matches!(ack.status, AckStatus::Rejected(_)) {
            rejected += 1;
        }
    }

    let (samples, timing) = if render.measure {
        let (samples, timings) = render_measured(&mut engine, frames, scenario.block_frames);
        (samples, timings.summary())
    } else {
        (
            render_offline(&mut engine, frames, scenario.block_frames),
            None,
        )
    };

    let mut file = File::create(&render.out)
        .map_err(|err| format!("cannot create {}: {err}", render.out.display()))?;
    {
        let mut writer = BufWriter::new(&mut file);
        write_pcm16_mono(&mut writer, scenario.sample_rate, &samples)
            .map_err(|err| format!("cannot write {}: {err}", render.out.display()))?;
    }

    let peak = samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
    let energy: f64 = samples
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>();
    let rms = if samples.is_empty() {
        0.0
    } else {
        (energy / samples.len() as f64).sqrt()
    };

    let mut ack_buffer: [Ack; MAX_REPORTED_ACKS] = std::array::from_fn(|_| Ack::accepted(0));
    let drained = engine.drain_acks(&mut ack_buffer);
    let applied: Vec<_> = ack_buffer
        .iter()
        .take(drained)
        .filter_map(|ack| match ack.status {
            AckStatus::Applied { at_sample } => Some(json!({
                "id": ack.command_id,
                "applied_at_sample": at_sample
            })),
            _ => None,
        })
        .collect();

    let timing_json = timing_json(timing, &scenario);
    let report = json!({
        "out": render.out.display().to_string(),
        "frames": frames,
        "sample_rate": scenario.sample_rate,
        "block_frames": scenario.block_frames,
        "frequency_hz": scenario.frequency_hz,
        "peak": peak,
        "rms": rms,
        "commands": scenario.commands.len(),
        "rejected": rejected,
        "applied": applied,
        "timing": timing_json,
    });
    Ok(report.to_string())
}

/// Render a two-deck mix scenario to a stereo WAV and report what the decks did.
fn run_mix(run: MixRun) -> Result<String, String> {
    let text = std::fs::read_to_string(&run.path)
        .map_err(|err| format!("cannot read {}: {err}", run.path.display()))?;
    let scenario: mix::MixScenario = serde_json::from_str(&text).map_err(|err| {
        format!(
            "cannot parse {} as a mix scenario: {err}",
            run.path.display()
        )
    })?;

    let mut cache = match &run.cache_dir {
        Some(dir) => Some(mix::cache_for(dir.clone())?),
        None => None,
    };
    let report = mix::render(&scenario, cache.as_mut())?;

    let file = File::create(&run.out)
        .map_err(|err| format!("cannot create {}: {err}", run.out.display()))?;
    {
        let mut writer = BufWriter::new(&file);
        write_pcm16_interleaved(&mut writer, scenario.sample_rate, 2, &report.samples)
            .map_err(|err| format!("cannot write {}: {err}", run.out.display()))?;
    }

    if let (Some(path), Some(cue)) = (&run.cue_out, report.cue_samples.as_ref()) {
        let file =
            File::create(path).map_err(|err| format!("cannot create {}: {err}", path.display()))?;
        let mut writer = BufWriter::new(&file);
        write_pcm16_interleaved(&mut writer, scenario.sample_rate, 2, cue)
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    }

    let peak = report
        .samples
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    let frames = report.samples.len() / 2;
    let decks: Vec<serde_json::Value> = report
        .decks
        .iter()
        .map(|deck| {
            json!({
                "deck": deck.deck,
                "load": deck.load,
                "position": deck.position,
                "underruns": deck.underruns,
                "loop_wraps": deck.loop_wraps,
                "hotcue_jumps": deck.hotcue_jumps,
                "ready_ms": deck.ready_ms,
                "fader": deck.fader,
                "assign": deck.assign,
                "cue": deck.cue,
                "channel_peak": deck.meter.peak,
            })
        })
        .collect();

    Ok(json!({
        "out": run.out.display().to_string(),
        "cue_out": run.cue_out.as_ref().map(|path| path.display().to_string()),
        "frames": frames,
        "sample_rate": scenario.sample_rate,
        "block_frames": scenario.block_frames,
        "commands": scenario.commands.len(),
        "applied": report.applied,
        "peak": peak,
        "decks": decks,
        "mixer": {
            "master_peak": report.meters.master_peak,
            "clipped_samples": report.meters.clipped_samples,
            "limiter_gain": report.meters.limiter_gain,
            "non_finite_samples": report.meters.non_finite_samples,
        },
    })
    .to_string())
}
