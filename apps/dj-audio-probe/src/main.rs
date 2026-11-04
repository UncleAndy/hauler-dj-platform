//! Read-only audio device probe (A05, ADR-002).
//!
//! Answers the question that decides the audio backend, without any engine code:
//! what does CPAL enumerate on this machine, how many channels does each output
//! offer, does the device expose anything beyond stereo (master / cue / booth),
//! and how do the callbacks behave for a short 48 kHz / 256-frame trial.
//!
//! It never opens input streams, never claims a result it did not measure, and
//! exits 3 when no output device is visible — "nothing measured" is a distinct
//! answer from "measured and failed".
//!
//! Usage:
//!   dj-audio-probe                     # human report, includes a 2 s trial
//!   dj-audio-probe --list-only         # enumeration only, no sound
//!   dj-audio-probe --json --out a.json # machine-readable evidence
//!   dj-audio-probe --seconds 10 --silent

use std::fmt::Write as _;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde_json::{Value, json};

const SCHEMA_VERSION: u64 = 1;
const TRIAL_RATE: u32 = 48_000;
const TRIAL_FRAMES: u32 = 256;
const TRIAL_CHANNELS: u16 = 2;

const USAGE: &str = "\
dj-audio-probe — read-only audio device probe (ADR-002 / A05)

Usage:
  dj-audio-probe                     enumerate devices and run a short output trial
  dj-audio-probe --list-only         enumeration only (no sound, no stream)
  dj-audio-probe --json              machine-readable output
  dj-audio-probe --out FILE          write the report to FILE
  dj-audio-probe --seconds N         trial length in seconds (default 2)
  dj-audio-probe --silent            run the trial with silence instead of a 440 Hz tone
  dj-audio-probe --help | --version

Exit codes:
  0  report produced and at least one output device was enumerated
  3  report produced, but no output device is visible (nothing was measured)
  2  usage error or the report could not be written
";

#[derive(Debug, Default)]
struct Args {
    json: bool,
    list_only: bool,
    silent: bool,
    seconds: u64,
    out: Option<String>,
}

impl Args {
    fn parse(argv: &[String]) -> Result<Option<Self>, String> {
        let mut args = Args {
            seconds: 2,
            ..Args::default()
        };
        let mut iter = argv.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--json" => args.json = true,
                "--list-only" => args.list_only = true,
                "--silent" => args.silent = true,
                "--seconds" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--seconds needs a value".to_string())?;
                    let parsed: u64 = value
                        .parse()
                        .map_err(|_| format!("--seconds expects a number, got {value:?}"))?;
                    if parsed == 0 || parsed > 3600 {
                        return Err("--seconds must be between 1 and 3600".to_string());
                    }
                    args.seconds = parsed;
                }
                "--out" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--out needs a file path".to_string())?;
                    args.out = Some(value.clone());
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(Some(args))
    }
}

/// One enumerated output device, in the shape the report and the JSON share.
#[derive(Debug, Clone)]
struct OutputDevice {
    name: String,
    default: bool,
    channels: Vec<u16>,
    max_channels: u16,
    sample_rates: Vec<u32>,
    buffer_frames: Option<(u32, u32)>,
    sample_formats: Vec<String>,
}

fn configs_of(device: &cpal::Device) -> Vec<cpal::SupportedStreamConfigRange> {
    match device.supported_output_configs() {
        Ok(configs) => configs.collect(),
        Err(_) => Vec::new(),
    }
}

fn describe_device(device: &cpal::Device, default: &Option<String>) -> OutputDevice {
    let name = device
        .name()
        .unwrap_or_else(|_| "<unnamed device>".to_string());
    let configs = configs_of(device);

    let mut channels: Vec<u16> = configs.iter().map(|c| c.channels()).collect();
    channels.sort_unstable();
    channels.dedup();

    let mut sample_rates: Vec<u32> = Vec::new();
    for config in &configs {
        for rate in [config.min_sample_rate().0, config.max_sample_rate().0] {
            if !sample_rates.contains(&rate) {
                sample_rates.push(rate);
            }
        }
    }
    sample_rates.sort_unstable();

    let buffer_frames = configs.iter().find_map(|c| match c.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    });

    let mut sample_formats: Vec<String> = configs
        .iter()
        .map(|c| format!("{:?}", c.sample_format()))
        .collect();
    sample_formats.sort();
    sample_formats.dedup();

    let max_channels = channels.iter().copied().max().unwrap_or(0);

    OutputDevice {
        default: default.as_deref() == Some(name.as_str()),
        name,
        channels,
        max_channels,
        sample_rates,
        buffer_frames,
        sample_formats,
    }
}

/// Nearest-rank percentile of an already sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

#[derive(Debug, Clone)]
struct TrialReport {
    device: String,
    sample_rate: u32,
    frames: u32,
    channels: u16,
    callbacks: usize,
    avg_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    gaps: usize,
    error: Option<String>,
}

impl TrialReport {
    fn period_ms(&self) -> f64 {
        f64::from(self.frames) * 1000.0 / f64::from(self.sample_rate)
    }
}

/// Summary of callback start times: interval statistics plus a gap count.
fn summarize(
    device: &str,
    timestamps_ns: &[u64],
    frames: u32,
    sample_rate: u32,
    channels: u16,
) -> TrialReport {
    let mut intervals: Vec<f64> = timestamps_ns
        .windows(2)
        .map(|w| (w[1] - w[0]) as f64 / 1_000_000.0)
        .collect();
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let period = f64::from(frames) * 1000.0 / f64::from(sample_rate);
    let gap_threshold = period * 1.5;
    let gaps = intervals.iter().filter(|i| **i > gap_threshold).count();
    let avg = if intervals.is_empty() {
        0.0
    } else {
        intervals.iter().sum::<f64>() / intervals.len() as f64
    };

    TrialReport {
        device: device.to_string(),
        sample_rate,
        frames,
        channels,
        callbacks: timestamps_ns.len(),
        avg_ms: avg,
        p99_ms: percentile(&intervals, 99.0),
        max_ms: intervals.last().copied().unwrap_or(0.0),
        gaps,
        error: None,
    }
}

fn run_trial(seconds: u64, silent: bool) -> TrialReport {
    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(device) => device,
        None => {
            return TrialReport {
                device: "<no default output>".to_string(),
                sample_rate: TRIAL_RATE,
                frames: TRIAL_FRAMES,
                channels: TRIAL_CHANNELS,
                callbacks: 0,
                avg_ms: 0.0,
                p99_ms: 0.0,
                max_ms: 0.0,
                gaps: 0,
                error: Some("no default output device".to_string()),
            };
        }
    };

    let name = device
        .name()
        .unwrap_or_else(|_| "<unnamed device>".to_string());

    let config = cpal::StreamConfig {
        channels: TRIAL_CHANNELS,
        sample_rate: cpal::SampleRate(TRIAL_RATE),
        buffer_size: cpal::BufferSize::Fixed(TRIAL_FRAMES),
    };

    let (tx, rx) = mpsc::channel::<u64>();
    let started = Instant::now();
    let mut phase: f32 = 0.0;
    let step = std::f32::consts::TAU * 440.0 / TRIAL_RATE as f32;
    // The probe is the one place where allocation and a mutex are acceptable: it is a
    // measurement tool, not the audio path (the RT contract applies to dj-audio-core).
    let data_fn = move |buffer: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let _ = tx.send(started.elapsed().as_nanos() as u64);
        for frame in buffer.chunks_mut(usize::from(TRIAL_CHANNELS)) {
            let sample = if silent { 0.0 } else { phase.sin() * 0.1 };
            phase = (phase + step) % std::f32::consts::TAU;
            for channel in frame.iter_mut() {
                *channel = sample;
            }
        }
    };
    let err_fn = |err: cpal::StreamError| eprintln!("dj-audio-probe: stream error: {err}");

    let stream = match device.build_output_stream(&config, data_fn, err_fn, None) {
        Ok(stream) => stream,
        Err(err) => {
            return TrialReport {
                device: name,
                sample_rate: TRIAL_RATE,
                frames: TRIAL_FRAMES,
                channels: TRIAL_CHANNELS,
                callbacks: 0,
                avg_ms: 0.0,
                p99_ms: 0.0,
                max_ms: 0.0,
                gaps: 0,
                error: Some(format!("build_output_stream: {err}")),
            };
        }
    };

    if let Err(err) = stream.play() {
        return TrialReport {
            device: name,
            sample_rate: TRIAL_RATE,
            frames: TRIAL_FRAMES,
            channels: TRIAL_CHANNELS,
            callbacks: 0,
            avg_ms: 0.0,
            p99_ms: 0.0,
            max_ms: 0.0,
            gaps: 0,
            error: Some(format!("play: {err}")),
        };
    }

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut timestamps: Vec<u64> = Vec::with_capacity(2048);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ts) => timestamps.push(ts),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(stream);

    summarize(&name, &timestamps, TRIAL_FRAMES, TRIAL_RATE, TRIAL_CHANNELS)
}

fn pipewire_snapshot() -> Value {
    let run = |program: &str, args: &[&str]| -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    };

    let ports = run("pw-cli", &["ls", "Port"]);
    let links = run("pw-link", &["-l"]);
    json!({
        "available": ports.is_some() || links.is_some(),
        "ports": ports,
        "links": links,
    })
}

fn human_report(
    hosts: &[(String, Vec<OutputDevice>)],
    trial: Option<&TrialReport>,
    pipewire: &Value,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "dj-audio-probe (schema {SCHEMA_VERSION})");
    let _ = writeln!(out, "hosts: {}", hosts.len());

    for (host_id, devices) in hosts {
        let _ = writeln!(out, "\n[{host_id}] output devices: {}", devices.len());
        for device in devices {
            let _ = writeln!(
                out,
                "  - {}{}",
                device.name,
                if device.default { "  <-- default" } else { "" }
            );
            let _ = writeln!(out, "      channels: {:?}", device.channels);
            let _ = writeln!(
                out,
                "      sample rates (min/max): {:?}",
                device.sample_rates
            );
            let _ = writeln!(out, "      sample formats: {:?}", device.sample_formats);
            match device.buffer_frames {
                Some((min, max)) => {
                    let _ = writeln!(out, "      buffer frames: {min}..{max}");
                }
                None => {
                    let _ = writeln!(out, "      buffer frames: unspecified");
                }
            }
            if device.max_channels > 2 {
                let _ = writeln!(
                    out,
                    "      >2 channels: a separate cue/booth route may be addressable here"
                );
            }
        }
    }

    match trial {
        Some(trial) => {
            let _ = writeln!(
                out,
                "\ntrial: {} @ {} Hz / {} frames / {} ch",
                trial.device, trial.sample_rate, trial.frames, trial.channels
            );
            let _ = writeln!(
                out,
                "  callbacks: {}  avg {:.3} ms  p99 {:.3} ms  max {:.3} ms  (period {:.3} ms)",
                trial.callbacks,
                trial.avg_ms,
                trial.p99_ms,
                trial.max_ms,
                trial.period_ms()
            );
            let _ = writeln!(out, "  gaps > 1.5x period: {}", trial.gaps);
            if let Some(error) = &trial.error {
                let _ = writeln!(out, "  error: {error}");
            }
        }
        None => {
            let _ = writeln!(out, "\ntrial: skipped (--list-only)");
        }
    }

    let _ = writeln!(
        out,
        "\npipewire snapshot: {}",
        if pipewire["available"].as_bool().unwrap_or(false) {
            "captured (see JSON or run pw-cli ls Port yourself)"
        } else {
            "unavailable (pw-cli/pw-link not running or not installed)"
        }
    );
    out
}

fn json_report(
    hosts: &[(String, Vec<OutputDevice>)],
    trial: Option<&TrialReport>,
    pipewire: &Value,
) -> Value {
    let host_values: Vec<Value> = hosts
        .iter()
        .map(|(host_id, devices)| {
            let device_values: Vec<Value> = devices
                .iter()
                .map(|device| {
                    json!({
                        "name": device.name,
                        "default": device.default,
                        "channels": device.channels,
                        "max_channels": device.max_channels,
                        "sample_rates": device.sample_rates,
                        "buffer_frames": device.buffer_frames.map(|(min, max)| json!({"min": min, "max": max})),
                        "sample_formats": device.sample_formats,
                    })
                })
                .collect();
            json!({"host": host_id, "devices": device_values})
        })
        .collect();

    let trial_value = match trial {
        Some(trial) => json!({
            "device": trial.device,
            "sample_rate": trial.sample_rate,
            "frames": trial.frames,
            "channels": trial.channels,
            "callbacks": trial.callbacks,
            "period_ms": trial.period_ms(),
            "avg_ms": trial.avg_ms,
            "p99_ms": trial.p99_ms,
            "max_ms": trial.max_ms,
            "gaps_over_1_5x_period": trial.gaps,
            "error": trial.error,
        }),
        None => Value::Null,
    };

    let output_count: usize = hosts.iter().map(|(_, devices)| devices.len()).sum();
    let multichannel = hosts
        .iter()
        .flat_map(|(_, devices)| devices.iter())
        .filter(|device| device.max_channels > 2)
        .count();

    json!({
        "schema_version": SCHEMA_VERSION,
        "generated_at_unix": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "hosts": host_values,
        "output_devices": output_count,
        "devices_with_more_than_2_channels": multichannel,
        "trial": trial_value,
        "pipewire": pipewire,
    })
}

fn collect_hosts() -> Vec<(String, Vec<OutputDevice>)> {
    let mut hosts = Vec::new();
    for host_id in cpal::available_hosts() {
        let host = match cpal::host_from_id(host_id) {
            Ok(host) => host,
            Err(err) => {
                eprintln!("dj-audio-probe: host {host_id:?}: {err}");
                continue;
            }
        };
        let default_name = host
            .default_output_device()
            .and_then(|device| device.name().ok());
        let mut devices: Vec<OutputDevice> = Vec::new();
        match host.output_devices() {
            Ok(iter) => {
                for device in iter {
                    devices.push(describe_device(&device, &default_name));
                }
            }
            Err(err) => eprintln!("dj-audio-probe: host {host_id:?}: {err}"),
        }
        hosts.push((format!("{host_id:?}"), devices));
    }
    hosts
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if argv.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("dj-audio-probe {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let args = match Args::parse(&argv) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("dj-audio-probe: {message}");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let hosts = collect_hosts();
    let output_devices: usize = hosts.iter().map(|(_, devices)| devices.len()).sum();
    let trial = if args.list_only || output_devices == 0 {
        None
    } else {
        Some(run_trial(args.seconds, args.silent))
    };
    let pipewire = pipewire_snapshot();

    let rendered = if args.json {
        let value = json_report(&hosts, trial.as_ref(), &pipewire);
        match serde_json::to_string_pretty(&value) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("dj-audio-probe: cannot render JSON: {err}");
                return ExitCode::from(2);
            }
        }
    } else {
        human_report(&hosts, trial.as_ref(), &pipewire)
    };

    match &args.out {
        Some(path) => {
            if let Err(err) = std::fs::write(path, format!("{rendered}\n")) {
                eprintln!("dj-audio-probe: cannot write {path}: {err}");
                return ExitCode::from(2);
            }
            eprintln!("dj-audio-probe: report written to {path}");
        }
        None => print!("{rendered}"),
    }

    if output_devices == 0 {
        eprintln!(
            "dj-audio-probe: no output device is visible — nothing was measured (running as a user \
             without a session? no PipeWire/ALSA device?)"
        );
        ExitCode::from(3)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse_the_documented_flags() {
        let argv = [
            "--json",
            "--list-only",
            "--seconds",
            "5",
            "--out",
            "report.json",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
        let args = match Args::parse(&argv) {
            Ok(Some(args)) => args,
            other => panic!("expected parsed args, got {other:?}"),
        };
        assert!(args.json && args.list_only);
        assert_eq!(args.seconds, 5);
        assert_eq!(args.out.as_deref(), Some("report.json"));
    }

    #[test]
    fn seconds_outside_the_range_is_an_error() {
        for value in ["0", "3601"] {
            let argv = vec!["--seconds".to_string(), value.to_string()];
            assert!(Args::parse(&argv).is_err(), "{value} must be rejected");
        }
    }

    #[test]
    fn unknown_flag_is_an_error() {
        let argv = vec!["--nope".to_string()];
        assert!(Args::parse(&argv).is_err());
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let sorted = [1.0, 2.0, 3.0, 4.0, 100.0];
        assert_eq!(percentile(&sorted, 0.0), 1.0);
        assert_eq!(percentile(&sorted, 50.0), 3.0);
        assert_eq!(percentile(&sorted, 99.0), 100.0);
        assert_eq!(percentile(&[], 99.0), 0.0);
    }

    #[test]
    fn summarize_counts_intervals_and_gaps() {
        // 5.33 ms period: five "good" intervals, then one 20 ms stall.
        let ns = |ms: f64| (ms * 1_000_000.0) as u64;
        let stamps = [0.0, 5.33, 10.66, 15.99, 21.32, 26.65, 46.65]
            .iter()
            .map(|ms| ns(*ms))
            .collect::<Vec<_>>();
        let report = summarize("test", &stamps, 256, 48_000, 2);
        assert_eq!(report.callbacks, 7);
        assert_eq!(report.gaps, 1);
        assert!((report.period_ms() - 5.333_333).abs() < 1e-5);
        assert!(report.max_ms > 19.0 && report.max_ms < 21.0);
    }

    #[test]
    fn json_report_has_the_documented_shape() {
        let hosts = vec![(
            "Alsa".to_string(),
            vec![OutputDevice {
                name: "S4 MK3".to_string(),
                default: true,
                channels: vec![2, 8],
                max_channels: 8,
                sample_rates: vec![44_100, 48_000],
                buffer_frames: Some((16, 8192)),
                sample_formats: vec!["F32".to_string()],
            }],
        )];
        let value = json_report(&hosts, None, &json!({"available": false}));
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["output_devices"], 1);
        assert_eq!(value["devices_with_more_than_2_channels"], 1);
        assert_eq!(value["hosts"][0]["devices"][0]["channels"][1], 8);
        assert!(value["trial"].is_null());
    }
}
