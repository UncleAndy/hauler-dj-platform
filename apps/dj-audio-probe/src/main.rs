//! Read-only audio device probe (A05, ADR-002).
//!
//! The question it answers: what can this machine actually do, and on which
//! interface — not "what does the API claim". Three independent sources are
//! reported side by side, because they disagree:
//!
//! 1. **USB descriptors** (`/proc/asound/card*/stream0`) — the hardware truth about
//!    channel counts, formats and rates. Readable without `/dev/snd`.
//! 2. **CPAL enumeration** — what the Rust API sees. On ALSA this is the *plugin*
//!    layer, where "up to 32 channels" is an API limit and not the device.
//! 3. **Channel trials** — an actual `build_output_stream` at 48 kHz for 2/4/6/8
//!    channels, so "supported" is a measured fact rather than a range.
//!
//! Plus a short callback-timing trial on the default output and a PipeWire node/port
//! summary from `pw-dump` (ports are what decides whether master and cue can be
//! addressed separately).
//!
//! It never opens input streams, never sends anything to a controller, and exits 3
//! when no output device is visible — "nothing measured" is a distinct answer from
//! "measured and unsupported".

mod asound;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde_json::{Value, json};

const SCHEMA_VERSION: u64 = 2;
const TRIAL_RATE: u32 = 48_000;
const TRIAL_FRAMES: u32 = 256;
const TRIAL_CHANNELS: u16 = 2;
const CHANNEL_TRIALS: [u16; 4] = [2, 4, 6, 8];

const USAGE: &str = "\
dj-audio-probe — read-only audio device probe (ADR-002 / A05)

Usage:
  dj-audio-probe                     enumerate, probe channels, run a short timing trial
  dj-audio-probe --list-only         enumeration and descriptors only (opens no stream)
  dj-audio-probe --json              machine-readable output
  dj-audio-probe --out FILE          write the report to FILE
  dj-audio-probe --seconds N         timing trial length in seconds (default 2)
  dj-audio-probe --silent            timing trial with silence instead of a 440 Hz tone
  dj-audio-probe --devices SUBSTR    only devices whose name contains SUBSTR (case-insensitive)
  dj-audio-probe --open-timeout-ms N give up on one stream open after N ms (default 2000)
  dj-audio-probe --trial-budget-s N  stop starting new channel trials after N s (default 25)
  dj-audio-probe --tone-device SUBSTR  run the trial on that device (default: system default)
  dj-audio-probe --tone-pair K        audible mapping check: 440 Hz only on channel pair K
                                      (0 = first two channels, 1 = channels 3-4, ...)
  dj-audio-probe --help | --version

What the sections mean:
  USB descriptors  channel counts written in the hardware descriptor (authoritative)
  CPAL             what the API reports; on ALSA these are plugin ranges, not hardware
  channel trials   a real 48 kHz stream opened with 2/4/6/8 channels (measured)
  pipewire         node/port names from pw-dump (how master/cue can be addressed)

A device whose open blocks is reported as TIMEOUT and never waited on further: the
whole run is bounded by --trial-budget-s plus the timing trial.

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
    devices: Option<String>,
    open_timeout_ms: u64,
    budget_s: u64,
    tone_pair: Option<u16>,
    tone_device: Option<String>,
}

impl Args {
    fn parse(argv: &[String]) -> Result<Option<Self>, String> {
        let mut args = Args {
            seconds: 2,
            open_timeout_ms: 2000,
            budget_s: 25,
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
                "--devices" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--devices needs a name substring".to_string())?;
                    args.devices = Some(value.to_lowercase());
                }
                "--open-timeout-ms" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--open-timeout-ms needs a value".to_string())?;
                    let parsed: u64 = value.parse().map_err(|_| {
                        format!("--open-timeout-ms expects a number, got {value:?}")
                    })?;
                    if !(200..=30_000).contains(&parsed) {
                        return Err("--open-timeout-ms must be between 200 and 30000".to_string());
                    }
                    args.open_timeout_ms = parsed;
                }
                "--tone-pair" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--tone-pair needs a channel pair index".to_string())?;
                    let parsed: u16 = value
                        .parse()
                        .map_err(|_| format!("--tone-pair expects a number, got {value:?}"))?;
                    if parsed > 3 {
                        return Err("--tone-pair must be 0..3 (channel pair index)".to_string());
                    }
                    args.tone_pair = Some(parsed);
                }
                "--tone-device" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--tone-device needs a name substring".to_string())?;
                    args.tone_device = Some(value.to_lowercase());
                }
                "--trial-budget-s" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--trial-budget-s needs a value".to_string())?;
                    let parsed: u64 = value
                        .parse()
                        .map_err(|_| format!("--trial-budget-s expects a number, got {value:?}"))?;
                    if !(1..=600).contains(&parsed) {
                        return Err("--trial-budget-s must be between 1 and 600".to_string());
                    }
                    args.budget_s = parsed;
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(Some(args))
    }
}

// ---------------------------------------------------------------- CPAL layer

#[derive(Debug, Clone)]
struct OutputDevice {
    name: String,
    default: bool,
    plugin_channels: Vec<u16>,
    sample_rates: Vec<u32>,
    buffer_frames: Option<(u32, u32)>,
    sample_formats: Vec<String>,
}

fn describe_device(device: &cpal::Device, default: &Option<String>) -> OutputDevice {
    let name = device.to_string();
    let configs: Vec<cpal::SupportedStreamConfigRange> = match device.supported_output_configs() {
        Ok(configs) => configs.collect(),
        Err(_) => Vec::new(),
    };

    let mut plugin_channels: Vec<u16> = configs.iter().map(|c| c.channels()).collect();
    plugin_channels.sort_unstable();
    plugin_channels.dedup();

    let mut sample_rates: Vec<u32> = Vec::new();
    for config in &configs {
        for rate in [config.min_sample_rate(), config.max_sample_rate()] {
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

    OutputDevice {
        default: default.as_deref() == Some(name.as_str()),
        name,
        plugin_channels,
        sample_rates,
        buffer_frames,
        sample_formats,
    }
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
            .map(|device| device.to_string());
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

// --------------------------------------------------------- channel trials

#[derive(Debug, Clone)]
struct ChannelTrial {
    device: String,
    channels: u16,
    ok: bool,
    callbacks: usize,
    buffer: String,
    error: Option<String>,
    timed_out: bool,
}

/// Run `work` on a worker thread and wait at most `timeout`.
///
/// `None` means "still blocked": the caller reports a timeout and moves on. The worker
/// is abandoned deliberately — a stuck ALSA open cannot be cancelled from outside, and
/// the probe must never hang on it. Every blocking call in this tool goes through here.
fn bounded<T: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel::<T>();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(timeout).ok()
}

#[derive(Debug, Clone)]
enum Attempt {
    Opened { callbacks: usize },
    Failed { error: String },
    Timeout,
}

/// Open a stream and count callbacks, but never wait longer than `timeout`.
///
/// `build_output_stream` can block indefinitely on a busy or half-dead ALSA device
/// (PipeWire holding the card, a stuck `default` plugin, a device that stopped
/// answering). The whole attempt runs on a worker thread, so a blocked open becomes
/// a reported result instead of a hung tool.
fn attempt_stream(
    device: &cpal::Device,
    channels: u16,
    buffer: cpal::BufferSize,
    timeout: Duration,
) -> Attempt {
    let device = device.clone();

    match bounded(timeout, move || {
        (|| -> Result<usize, String> {
            let config = cpal::StreamConfig {
                channels,
                sample_rate: TRIAL_RATE,
                buffer_size: buffer,
            };
            let (done_tx, done_rx) = mpsc::channel::<()>();

            // Silence only: these trials answer "can this channel count be opened",
            // they must not surprise the listener with a tone.
            let data_fn = move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for sample in out.iter_mut() {
                    *sample = 0.0;
                }
                let _ = done_tx.send(());
            };

            let stream = device
                .build_output_stream(
                    config,
                    data_fn,
                    |err| eprintln!("dj-audio-probe: stream error: {err}"),
                    None,
                )
                .map_err(|err| err.to_string())?;
            stream.play().map_err(|err| err.to_string())?;

            let deadline = Instant::now() + Duration::from_millis(150);
            let mut callbacks = 0usize;
            while Instant::now() < deadline {
                match done_rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(()) => callbacks += 1,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            drop(stream);
            Ok(callbacks)
        })()
    }) {
        Some(Ok(callbacks)) => Attempt::Opened { callbacks },
        Some(Err(error)) => Attempt::Failed { error },
        None => Attempt::Timeout,
    }
}

fn probe_channels(
    device: &cpal::Device,
    name: &str,
    channels: u16,
    timeout: Duration,
) -> ChannelTrial {
    let mut trial = ChannelTrial {
        device: name.to_string(),
        channels,
        ok: false,
        callbacks: 0,
        buffer: format!("{TRIAL_FRAMES}"),
        error: None,
        timed_out: false,
    };

    match attempt_stream(
        device,
        channels,
        cpal::BufferSize::Fixed(TRIAL_FRAMES),
        timeout,
    ) {
        Attempt::Opened { callbacks } if callbacks > 0 => {
            trial.ok = true;
            trial.callbacks = callbacks;
            return trial;
        }
        Attempt::Opened { .. } => {
            trial.error = Some("stream opened but produced no callbacks".to_string());
        }
        Attempt::Failed { error } => trial.error = Some(error),
        Attempt::Timeout => {
            // A blocked device is a result, not a reason to wait twice.
            trial.timed_out = true;
            trial.error = Some(format!(
                "open blocked for more than {} ms",
                timeout.as_millis()
            ));
            return trial;
        }
    }

    // Some devices reject a fixed buffer size; retry with the backend default.
    match attempt_stream(device, channels, cpal::BufferSize::Default, timeout) {
        Attempt::Opened { callbacks } if callbacks > 0 => {
            trial.ok = true;
            trial.callbacks = callbacks;
            trial.buffer = "default".to_string();
            trial.error = None;
        }
        Attempt::Opened { .. } => {}
        Attempt::Failed { error } => {
            if trial.error.is_none() {
                trial.error = Some(error);
            }
        }
        Attempt::Timeout => {
            trial.timed_out = true;
            trial.error = Some(format!(
                "open blocked for more than {} ms",
                timeout.as_millis()
            ));
        }
    }
    trial
}

fn matches_filter(name: &str, filter: Option<&str>) -> bool {
    match filter {
        Some(needle) => name.to_lowercase().contains(needle),
        None => true,
    }
}

fn channel_trials(
    filter: Option<&str>,
    timeout: Duration,
    budget: Duration,
) -> (Vec<ChannelTrial>, bool) {
    let host = cpal::default_host();
    let devices: Vec<cpal::Device> = match host.output_devices() {
        Ok(iter) => iter.collect(),
        Err(_) => return (Vec::new(), false),
    };

    let started = Instant::now();
    let mut trials = Vec::new();
    let mut budget_exhausted = false;

    for device in devices {
        let name = device.to_string();
        if !matches_filter(&name, filter) {
            continue;
        }
        for channels in CHANNEL_TRIALS {
            if started.elapsed() > budget {
                budget_exhausted = true;
                break;
            }
            eprintln!("dj-audio-probe: trying {name} with {channels} ch");
            trials.push(probe_channels(&device, &name, channels, timeout));
        }
        if budget_exhausted {
            break;
        }
    }

    (trials, budget_exhausted)
}

// ------------------------------------------------------------ timing trial

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
    tone_pair: Option<u16>,
}

impl TrialReport {
    /// Period we asked for — what a 256-frame block would take.
    fn period_ms(&self) -> f64 {
        f64::from(self.frames) * 1000.0 / f64::from(self.sample_rate)
    }

    /// Period the backend actually delivered, derived from the callback intervals.
    /// The ALSA plugin layer routinely ignores the requested buffer size, so this is
    /// the number that describes the measurement.
    fn effective_period_ms(&self) -> f64 {
        self.avg_ms
    }

    fn effective_frames(&self) -> f64 {
        self.avg_ms * f64::from(self.sample_rate) / 1000.0
    }

    fn requested_size_was_honoured(&self) -> bool {
        let requested = self.period_ms();
        requested > 0.0 && (self.effective_period_ms() - requested).abs() / requested < 0.2
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
    let gaps = intervals.iter().filter(|i| **i > period * 1.5).count();
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
        tone_pair: None,
    }
}

fn failed_trial(device: String, error: String) -> TrialReport {
    TrialReport {
        device,
        sample_rate: TRIAL_RATE,
        frames: TRIAL_FRAMES,
        channels: TRIAL_CHANNELS,
        callbacks: 0,
        avg_ms: 0.0,
        p99_ms: 0.0,
        max_ms: 0.0,
        gaps: 0,
        error: Some(error),
        tone_pair: None,
    }
}

fn run_trial(
    seconds: u64,
    silent: bool,
    timeout: Duration,
    tone_pair: Option<u16>,
    tone_device: Option<String>,
) -> TrialReport {
    match bounded(timeout, move || {
        run_trial_inner(seconds, silent, tone_pair, tone_device.as_deref())
    }) {
        Some(report) => report,
        None => failed_trial(
            "<timing trial>".to_string(),
            format!(
                "device did not answer within {} s (open blocked)",
                timeout.as_secs()
            ),
        ),
    }
}

fn run_trial_inner(
    seconds: u64,
    silent: bool,
    tone_pair: Option<u16>,
    tone_device: Option<&str>,
) -> TrialReport {
    let host = cpal::default_host();

    let device = match tone_device {
        Some(needle) => {
            let devices: Vec<cpal::Device> = match host.output_devices() {
                Ok(iter) => iter.collect(),
                Err(err) => {
                    return failed_trial("<enumeration failed>".to_string(), err.to_string());
                }
            };
            match devices
                .into_iter()
                .find(|device| matches_filter(&device.to_string(), Some(needle)))
            {
                Some(device) => device,
                None => {
                    return failed_trial(
                        format!("<no device matching {needle:?}>"),
                        "no output device matched --tone-device".to_string(),
                    );
                }
            }
        }
        None => match host.default_output_device() {
            Some(device) => device,
            None => {
                return failed_trial(
                    "<no default output>".to_string(),
                    "no default output device".to_string(),
                );
            }
        },
    };
    let name = device.to_string();

    // An audible mapping check needs the full width up to the requested pair.
    let channels = match tone_pair {
        Some(pair) => (pair + 1) * 2,
        None => TRIAL_CHANNELS,
    };
    let config = cpal::StreamConfig {
        channels,
        sample_rate: TRIAL_RATE,
        buffer_size: cpal::BufferSize::Fixed(TRIAL_FRAMES),
    };

    let (tx, rx) = mpsc::channel::<u64>();
    let started = Instant::now();
    let mut phase: f32 = 0.0;
    let step = std::f32::consts::TAU * 440.0 / TRIAL_RATE as f32;
    // The probe is the one place where a mutex-free channel and a small buffer are
    // acceptable: it is a measurement tool, not the audio path (the RT contract
    // applies to dj-audio-core).
    let data_fn = move |buffer: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let _ = tx.send(started.elapsed().as_nanos() as u64);
        for frame in buffer.chunks_mut(usize::from(channels)) {
            let sample = if silent { 0.0 } else { phase.sin() * 0.1 };
            phase = (phase + step) % std::f32::consts::TAU;
            for (index, channel) in frame.iter_mut().enumerate() {
                // With --tone-pair only that pair is audible; everything else stays silent
                // so the listener can tell which physical output the pair reaches.
                *channel = match tone_pair {
                    Some(pair) => {
                        let first = usize::from(pair) * 2;
                        if index == first || index == first + 1 {
                            sample
                        } else {
                            0.0
                        }
                    }
                    None => sample,
                };
            }
        }
    };

    let stream = match device.build_output_stream(
        config,
        data_fn,
        |err| eprintln!("dj-audio-probe: stream error: {err}"),
        None,
    ) {
        Ok(stream) => stream,
        Err(err) => return failed_trial(name, format!("build_output_stream: {err}")),
    };

    if let Err(err) = stream.play() {
        return failed_trial(name, format!("play: {err}"));
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

    let report = summarize(&name, &timestamps, TRIAL_FRAMES, TRIAL_RATE, channels);
    TrialReport {
        tone_pair,
        ..report
    }
}

// --------------------------------------------------------------- PipeWire

#[derive(Debug, Clone)]
struct PwNode {
    id: u64,
    name: String,
    media_class: String,
    channels: Option<u64>,
    out_ports: Vec<String>,
    in_ports: Vec<String>,
}

fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn summarize_pw_dump(dump: &Value) -> Vec<PwNode> {
    let Some(objects) = dump.as_array() else {
        return Vec::new();
    };

    let mut nodes: BTreeMap<u64, PwNode> = BTreeMap::new();
    let mut ports: Vec<(u64, String, String)> = Vec::new();

    for object in objects {
        let Some(kind) = object["type"].as_str() else {
            continue;
        };
        let Some(id) = object["id"].as_u64() else {
            continue;
        };
        let props = &object["info"]["props"];

        match kind {
            "PipeWire:Interface:Node" => {
                let name = props["node.name"]
                    .as_str()
                    .or_else(|| props["node.description"].as_str())
                    .unwrap_or("<unnamed node>")
                    .to_string();
                let media_class = props["media.class"].as_str().unwrap_or("").to_string();
                let channels = props["audio.channels"].as_u64();
                nodes.insert(
                    id,
                    PwNode {
                        id,
                        name,
                        media_class,
                        channels,
                        out_ports: Vec::new(),
                        in_ports: Vec::new(),
                    },
                );
            }
            "PipeWire:Interface:Port" => {
                let Some(node_id) = props["node.id"].as_u64() else {
                    continue;
                };
                let direction = props["port.direction"].as_str().unwrap_or("");
                let name = props["port.name"]
                    .as_str()
                    .or_else(|| props["audio.channel"].as_str())
                    .unwrap_or("")
                    .to_string();
                ports.push((node_id, direction.to_string(), name));
            }
            _ => {}
        }
    }

    for (node_id, direction, name) in ports {
        if let Some(node) = nodes.get_mut(&node_id) {
            if direction == "out" {
                node.out_ports.push(name);
            } else {
                node.in_ports.push(name);
            }
        }
    }

    nodes.into_values().collect()
}

fn pipewire_nodes() -> Vec<PwNode> {
    let Some(text) = run_capture("pw-dump", &[]) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(dump) => summarize_pw_dump(&dump),
        Err(_) => Vec::new(),
    }
}

// ---------------------------------------------------------------- reports

fn human_report(
    hosts: &[(String, Vec<OutputDevice>)],
    cards: &[asound::UsbCard],
    trials: &[ChannelTrial],
    trial: Option<&TrialReport>,
    nodes: &[PwNode],
    budget_exhausted: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "dj-audio-probe (schema {SCHEMA_VERSION})");

    let _ = writeln!(
        out,
        "\n== USB descriptors (/proc/asound/card*/stream0) — hardware truth =="
    );
    if cards.is_empty() {
        let _ = writeln!(out, "  none readable");
    }
    for card in cards {
        let _ = writeln!(
            out,
            "  {} [{}] playback max {} ch, capture max {} ch",
            card.name,
            card.id,
            card.playback_max_channels(),
            card.capture_max_channels()
        );
        for altset in &card.playback {
            let _ = writeln!(
                out,
                "      playback: {} ch, {}, rates {:?}, 48 kHz {}{}",
                altset.channels,
                altset.format.as_deref().unwrap_or("format?"),
                altset.rates,
                if altset.supports_rate(TRIAL_RATE) {
                    "yes"
                } else {
                    "NO"
                },
                altset
                    .channel_map
                    .as_deref()
                    .map(|map| format!(", map {map}"))
                    .unwrap_or_default()
            );
        }
        for altset in &card.capture {
            let _ = writeln!(
                out,
                "      capture:  {} ch, {}, rates {:?}",
                altset.channels,
                altset.format.as_deref().unwrap_or("format?"),
                altset.rates
            );
        }
    }

    let _ = writeln!(
        out,
        "\n== CPAL enumeration (ALSA hosts report plugin ranges, PipeWire hosts report the node) =="
    );
    let _ = writeln!(
        out,
        "hosts: {} {:?}",
        hosts.len(),
        hosts.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>()
    );
    for (host_id, devices) in hosts {
        let _ = writeln!(out, "[{host_id}] output devices: {}", devices.len());
        for device in devices {
            let _ = writeln!(
                out,
                "  - {}{}",
                device.name,
                if device.default { "  <-- default" } else { "" }
            );
            let _ = writeln!(out, "      plugin channels: {:?}", device.plugin_channels);
            let _ = writeln!(out, "      plugin rates:    {:?}", device.sample_rates);
            let _ = writeln!(out, "      sample formats:  {:?}", device.sample_formats);
            match device.buffer_frames {
                Some((min, max)) => {
                    let _ = writeln!(out, "      buffer frames:   {min}..{max}");
                }
                None => {
                    let _ = writeln!(out, "      buffer frames:   unspecified");
                }
            }
        }
    }

    let _ = writeln!(
        out,
        "\n== channel trials (48 kHz, real stream open, silence) =="
    );
    if trials.is_empty() {
        let _ = writeln!(out, "  skipped or no output device");
    }
    let mut by_device: BTreeMap<&str, Vec<&ChannelTrial>> = BTreeMap::new();
    for trial in trials {
        by_device
            .entry(trial.device.as_str())
            .or_default()
            .push(trial);
    }
    for (device, device_trials) in by_device {
        let summary: Vec<String> = device_trials
            .iter()
            .map(|trial| {
                if trial.ok {
                    format!("{}ch ok", trial.channels)
                } else if trial.timed_out {
                    format!("{}ch TIMEOUT", trial.channels)
                } else {
                    format!("{}ch FAIL", trial.channels)
                }
            })
            .collect();
        let _ = writeln!(out, "  {device}: {}", summary.join(", "));
        for trial in device_trials.iter().filter(|trial| !trial.ok) {
            if let Some(error) = &trial.error {
                let _ = writeln!(out, "      {}ch: {error}", trial.channels);
            }
        }
    }

    if budget_exhausted {
        let _ = writeln!(
            out,
            "  trial budget exhausted: remaining channel counts were not attempted"
        );
    }

    let _ = writeln!(out, "\n== timing trial ==");
    match trial {
        Some(trial) => {
            let _ = writeln!(
                out,
                "  {} @ {} Hz / {} frames / {} ch",
                trial.device, trial.sample_rate, trial.frames, trial.channels
            );
            let _ = writeln!(
                out,
                "  callbacks {}  avg {:.3} ms  p99 {:.3} ms  max {:.3} ms  (period {:.3} ms)",
                trial.callbacks,
                trial.avg_ms,
                trial.p99_ms,
                trial.max_ms,
                trial.period_ms()
            );
            let _ = writeln!(
                out,
                "  effective block: {:.1} ms ≈ {:.0} frames (requested {} frames)",
                trial.effective_period_ms(),
                trial.effective_frames(),
                trial.frames
            );
            if !trial.requested_size_was_honoured() {
                let _ = writeln!(
                    out,
                    "  NOTE: the backend ignored the requested buffer size — these numbers do not \
                     describe a {}-frame block",
                    trial.frames
                );
            }
            if let Some(pair) = trial.tone_pair {
                let _ = writeln!(
                    out,
                    "  tone pair: {} (channels {} and {} only — LISTEN to which output it reaches)",
                    pair,
                    pair * 2,
                    pair * 2 + 1
                );
            }
            let _ = writeln!(out, "  gaps > 1.5x requested period: {}", trial.gaps);
            if let Some(error) = &trial.error {
                let _ = writeln!(out, "  error: {error}");
            }
        }
        None => {
            let _ = writeln!(out, "  skipped (--list-only or no output device)");
        }
    }

    let _ = writeln!(out, "\n== PipeWire nodes (pw-dump) ==");
    if nodes.is_empty() {
        let _ = writeln!(
            out,
            "  unavailable (no running session, or pw-dump missing)"
        );
    }
    for node in nodes
        .iter()
        .filter(|node| node.media_class.starts_with("Audio/"))
    {
        let _ = writeln!(
            out,
            "  [{}] {} — {}, {} ch",
            node.id,
            node.name,
            node.media_class,
            node.channels
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string())
        );
        if !node.out_ports.is_empty() {
            let _ = writeln!(out, "      out: {}", node.out_ports.join(" "));
        }
        if !node.in_ports.is_empty() {
            let _ = writeln!(out, "      in:  {}", node.in_ports.join(" "));
        }
    }
    out
}

fn json_report(
    args: &Args,
    budget_exhausted: bool,
    hosts: &[(String, Vec<OutputDevice>)],
    cards: &[asound::UsbCard],
    trials: &[ChannelTrial],
    trial: Option<&TrialReport>,
    nodes: &[PwNode],
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
                        "plugin_channels": device.plugin_channels,
                        "plugin_sample_rates": device.sample_rates,
                        "buffer_frames": device.buffer_frames.map(|(min, max)| json!({"min": min, "max": max})),
                        "sample_formats": device.sample_formats,
                    })
                })
                .collect();
            json!({"host": host_id, "devices": device_values})
        })
        .collect();

    let card_values: Vec<Value> = cards
        .iter()
        .map(|card| {
            let playback: Vec<Value> = card
                .playback
                .iter()
                .map(|altset| {
                    json!({
                        "interface": altset.interface,
                        "altset": altset.altset,
                        "channels": altset.channels,
                        "format": altset.format,
                        "rates": altset.rates,
                        "channel_map": altset.channel_map,
                        "supports_48k": altset.supports_rate(TRIAL_RATE),
                    })
                })
                .collect();
            let capture: Vec<Value> = card
                .capture
                .iter()
                .map(|altset| {
                    json!({
                        "interface": altset.interface,
                        "altset": altset.altset,
                        "channels": altset.channels,
                        "format": altset.format,
                        "rates": altset.rates,
                        "channel_map": altset.channel_map,
                    })
                })
                .collect();
            json!({
                "id": card.id,
                "name": card.name,
                "playback_max_channels": card.playback_max_channels(),
                "capture_max_channels": card.capture_max_channels(),
                "playback": playback,
                "capture": capture,
            })
        })
        .collect();

    let trial_values: Vec<Value> = trials
        .iter()
        .map(|trial| {
            json!({
                "device": trial.device,
                "channels": trial.channels,
                "ok": trial.ok,
                "callbacks": trial.callbacks,
                "buffer": trial.buffer,
                "error": trial.error,
                "timed_out": trial.timed_out,
            })
        })
        .collect();

    let node_values: Vec<Value> = nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "name": node.name,
                "media_class": node.media_class,
                "channels": node.channels,
                "out_ports": node.out_ports,
                "in_ports": node.in_ports,
            })
        })
        .collect();

    let timing = match trial {
        Some(trial) => json!({
            "device": trial.device,
            "sample_rate": trial.sample_rate,
            "frames": trial.frames,
            "channels": trial.channels,
            "callbacks": trial.callbacks,
            "period_ms": trial.period_ms(),
            "effective_period_ms": trial.effective_period_ms(),
            "effective_frames": trial.effective_frames(),
            "requested_size_honoured": trial.requested_size_was_honoured(),
            "avg_ms": trial.avg_ms,
            "p99_ms": trial.p99_ms,
            "max_ms": trial.max_ms,
            "gaps_over_1_5x_period": trial.gaps,
            "tone_pair": trial.tone_pair,
            "error": trial.error,
        }),
        None => Value::Null,
    };

    let output_count: usize = hosts.iter().map(|(_, devices)| devices.len()).sum();

    json!({
        "schema_version": SCHEMA_VERSION,
        "generated_at_unix": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "usb_cards": card_values,
        "cpal": {"hosts": host_values, "output_devices": output_count},
        "open_timeout_ms": args.open_timeout_ms,
        "trial_budget_s": args.budget_s,
        "budget_exhausted": budget_exhausted,
        "channel_trials": trial_values,
        "timing_trial": timing,
        "pipewire_nodes": node_values,
    })
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
    let cards = asound::read_cards("/proc/asound");
    let output_devices: usize = hosts.iter().map(|(_, devices)| devices.len()).sum();

    let (channel_trial_results, budget_exhausted) = if args.list_only || output_devices == 0 {
        (Vec::new(), false)
    } else {
        channel_trials(
            args.devices.as_deref(),
            Duration::from_millis(args.open_timeout_ms),
            Duration::from_secs(args.budget_s),
        )
    };

    let timing_trial = if args.list_only || output_devices == 0 {
        None
    } else {
        Some(run_trial(
            args.seconds,
            args.silent,
            Duration::from_secs(args.seconds + 5),
            args.tone_pair,
            args.tone_device.clone(),
        ))
    };
    let nodes = pipewire_nodes();

    let rendered = if args.json {
        let value = json_report(
            &args,
            budget_exhausted,
            &hosts,
            &cards,
            &channel_trial_results,
            timing_trial.as_ref(),
            &nodes,
        );
        match serde_json::to_string_pretty(&value) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("dj-audio-probe: cannot render JSON: {err}");
                return ExitCode::from(2);
            }
        }
    } else {
        human_report(
            &hosts,
            &cards,
            &channel_trial_results,
            timing_trial.as_ref(),
            &nodes,
            budget_exhausted,
        )
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
            "dj-audio-probe: no output device is visible — nothing was measured (run this in your \
             desktop session, with PipeWire running)"
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
    fn a_blocked_operation_times_out_instead_of_hanging() {
        let started = Instant::now();
        let result = bounded(Duration::from_millis(80), || {
            std::thread::sleep(Duration::from_millis(800));
            42
        });
        assert!(result.is_none(), "a blocked worker must report a timeout");
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "the caller must not wait for the abandoned worker"
        );
    }

    #[test]
    fn a_fast_operation_still_returns_its_value() {
        assert_eq!(bounded(Duration::from_millis(500), || 7), Some(7));
    }

    #[test]
    fn device_filter_is_case_insensitive() {
        assert!(matches_filter("Traktor Kontrol S4 MK3", Some("traktor")));
        assert!(matches_filter("Traktor Kontrol S4 MK3", Some("s4 mk3")));
        assert!(!matches_filter("HDA Intel PCH", Some("s4")));
        assert!(matches_filter("anything", None));
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
        let ns = |ms: f64| (ms * 1_000_000.0) as u64;
        let stamps = [0.0, 5.33, 10.66, 15.99, 21.32, 26.65, 46.65]
            .iter()
            .map(|ms| ns(*ms))
            .collect::<Vec<_>>();
        let report = summarize("test", &stamps, 256, 48_000, 2);
        assert_eq!(report.callbacks, 7);
        assert_eq!(report.gaps, 1);
        assert!((report.period_ms() - 5.333_333).abs() < 1e-5);
        // The fixture carries one 20 ms stall, so the mean interval is 7.775 ms
        // (~373 frames at 48 kHz) and the requested 256-frame block was not honoured.
        assert!((report.effective_frames() - 373.0).abs() < 2.0);
        assert!(!report.requested_size_was_honoured());

        let clean = [0.0, 5.33, 10.66, 15.99]
            .iter()
            .map(|ms| ns(*ms))
            .collect::<Vec<_>>();
        let clean_report = summarize("test", &clean, 256, 48_000, 2);
        assert!((clean_report.effective_frames() - 256.0).abs() < 1.0);
        assert!(clean_report.requested_size_was_honoured());
        assert!(report.max_ms > 19.0 && report.max_ms < 21.0);
    }

    #[test]
    fn pw_dump_is_summarized_into_nodes_with_ports() {
        let dump: Value = serde_json::from_str(
            r#"[
              {"id": 40, "type": "PipeWire:Interface:Node",
               "info": {"props": {"node.name": "alsa_output.usb-S4", "media.class": "Audio/Sink", "audio.channels": 4}}},
              {"id": 41, "type": "PipeWire:Interface:Port",
               "info": {"props": {"node.id": 40, "port.direction": "in", "audio.channel": "FL"}}},
              {"id": 42, "type": "PipeWire:Interface:Port",
               "info": {"props": {"node.id": 40, "port.direction": "in", "audio.channel": "FR"}}},
              {"id": 43, "type": "PipeWire:Interface:Port",
               "info": {"props": {"node.id": 40, "port.direction": "in", "audio.channel": "AUX0"}}}
            ]"#,
        )
        .unwrap_or(Value::Null);
        let nodes = summarize_pw_dump(&dump);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "alsa_output.usb-S4");
        assert_eq!(nodes[0].channels, Some(4));
        assert_eq!(nodes[0].in_ports, vec!["FL", "FR", "AUX0"]);
        assert!(nodes[0].out_ports.is_empty());
    }

    #[test]
    fn json_report_carries_all_three_sources() {
        let hosts = vec![(
            "Alsa".to_string(),
            vec![OutputDevice {
                name: "default".to_string(),
                default: true,
                plugin_channels: vec![1, 32],
                sample_rates: vec![1, 384_000],
                buffer_frames: Some((1, 4_194_304)),
                sample_formats: vec!["F32".to_string()],
            }],
        )];
        let cards = vec![asound::UsbCard {
            id: "card4".to_string(),
            name: "Traktor Kontrol S4 MK3".to_string(),
            playback: vec![asound::Altset {
                interface: Some(1),
                altset: Some(1),
                channels: 4,
                format: Some("S32_LE".to_string()),
                rates: vec![44_100, 48_000],
                channel_map: Some("FL FR FC LFE".to_string()),
                endpoint: None,
            }],
            capture: Vec::new(),
        }];
        let trials = vec![ChannelTrial {
            device: "default".to_string(),
            channels: 4,
            ok: true,
            callbacks: 12,
            buffer: "256".to_string(),
            error: None,
            timed_out: false,
        }];
        let args = Args {
            seconds: 2,
            open_timeout_ms: 2000,
            budget_s: 25,
            ..Args::default()
        };
        let value = json_report(&args, false, &hosts, &cards, &trials, None, &[]);
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["usb_cards"][0]["playback_max_channels"], 4);
        assert_eq!(
            value["usb_cards"][0]["playback"][0]["channel_map"],
            "FL FR FC LFE"
        );
        assert_eq!(value["channel_trials"][0]["ok"], true);
        assert!(value["timing_trial"].is_null());
    }
}
