#![recursion_limit = "512"]

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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
                [--out FILE] [--progress-seconds N] [--loop] [--loop-seconds N]
                [--config FILE] [--write-config FILE]

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
    block: Option<usize>,
    rate: Option<u32>,
    config: Option<PathBuf>,
    write_config: Option<PathBuf>,
    tone_hz: f32,
    gain: f32,
    cache_dir: Option<PathBuf>,
    out: Option<PathBuf>,
    progress_seconds: f64,
    loop_track: bool,
    loop_seconds: Option<f64>,
    buffered: Arc<AtomicU64>,
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
        block: None,
        rate: None,
        config: None,
        write_config: None,
        tone_hz: 440.0,
        gain: 0.25,
        cache_dir: None,
        out: None,
        progress_seconds: 60.0,
        loop_track: false,
        loop_seconds: None,
        buffered: Arc::new(AtomicU64::new(0)),
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
                options.block = Some(number("--block")? as usize);
                index += 2;
            }
            "--config" => {
                options.config = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--write-config" => {
                options.write_config = Some(PathBuf::from(value()?));
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
            "--out" => {
                options.out = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--progress-seconds" => {
                options.progress_seconds = number("--progress-seconds")?;
                index += 2;
            }
            "--loop" => {
                options.loop_track = true;
                index += 1;
            }
            "--loop-seconds" => {
                options.loop_seconds = Some(number("--loop-seconds")?);
                options.loop_track = true;
                index += 2;
            }
            other => return Err(Error::Usage(format!("unknown option {other:?}"))),
        }
        if options.block == Some(0) {
            return Err(Error::Usage("--block must be positive".to_string()));
        }
    }

    // The device decides the width, and its preferred rate is the one to ask for: the point of the
    // default path is "play through whatever the system uses", not "play through the S4".
    let shape = dj_audio_io::resolve_device(options.device.as_deref())
        .map_err(|err| Error::Device(err.to_string()))?;
    // Settings first, then flags: a value given on the command line wins, but the file is the
    // place where a machine-specific number belongs (block size, thresholds, loop budget).
    if let Some(path) = &options.write_config {
        let text = dj_config::Config::default()
            .to_toml()
            .map_err(|err| Error::Usage(err.to_string()))?;
        std::fs::write(path, text)
            .map_err(|err| Error::Usage(format!("cannot write {}: {err}", path.display())))?;
        println!("{}", path.display());
        std::process::exit(0);
    }
    let settings = match &options.config {
        Some(path) => dj_config::Config::load(path).map_err(|err| Error::Usage(err.to_string()))?,
        None => dj_config::Config::default(),
    };

    let rate = options.rate.unwrap_or(if settings.audio.sample_rate > 0 {
        settings.audio.sample_rate
    } else {
        shape.sample_rate
    });
    let block_frames = options.block.unwrap_or(settings.audio.block_frames);
    let thresholds = dj_audio_io::Thresholds {
        late_periods: settings.diagnostics.late_threshold_periods,
        early_periods: settings.diagnostics.early_threshold_periods,
        stall_us: settings.diagnostics.stall_threshold_us,
        expensive_percent: settings.diagnostics.expensive_threshold_percent,
    };
    let plan = RoutePlan::for_channels(shape.channels);

    let underruns = Arc::new(AtomicU64::new(0));
    let loop_wraps = Arc::new(AtomicU64::new(0));
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
                    first_beat_frame: None,
                    key: None,
                }),
            );
            // The audio callback must never wait for the prefetcher.
            deck.set_blocking(false);
            deck.set_gain(1.0);
            deck.set_loop_config(dj_deck::LoopConfig {
                buffer_max_seconds: settings.loop_playback.buffer_max_seconds,
            });
            // A whole-track loop keeps the real path busy for the whole run instead of rendering
            // silence once the file ends.
            if options.loop_track
                && let Some(total) = info.frames
            {
                // A whole-track loop by default; --loop-seconds narrows it to a musical loop, which
                // is where the seam is audible and where the capture buffer has to prove itself.
                let length = match options.loop_seconds {
                    Some(seconds) => (seconds.max(0.0) * f64::from(info.sample_rate)) as u64,
                    None => total,
                };
                let end = length.min(total);
                if let Some(region) = dj_domain::FrameRange::new(0, end) {
                    deck.set_loop(Some(region));
                }
            }
            deck.play();

            Box::new(DeckRenderer::new(
                deck,
                rate,
                block_frames,
                options.gain,
                Arc::clone(&underruns),
                Arc::clone(&loop_wraps),
                Arc::clone(&options.buffered),
            ))
        }
        None => Box::new(ToneRenderer::new(options.tone_hz, rate, options.gain)),
    };

    let config = LiveConfig {
        device: options
            .device
            .clone()
            .or_else(|| settings.audio.device.clone()),
        sample_rate: rate,
        block_frames,
        seconds: options.seconds,
        thresholds,
    };
    let output =
        LiveOutput::start(&config, plan, renderer).map_err(|err| Error::Device(err.to_string()))?;

    // A two-hour run must be observable while it happens: the report is rewritten at every tick,
    // so a crash or Ctrl-C still leaves the numbers up to that moment on disk.
    let started = Instant::now();
    // Baseline for the leak question: without a starting value a final RSS says nothing.
    let rss_start = rss_kb();
    let load_start = load_average();
    let tick = Duration::from_secs_f64(options.progress_seconds.max(1.0));
    let total = options.seconds.max(0.0);
    let mut final_report = output.snapshot();
    // Where in time the outliers happened: a total and a ring of the last 32 cannot answer "did it
    // get worse", and that question comes after every long run.
    let mut timeline: Vec<serde_json::Value> = Vec::new();
    // Kernel view of the audio thread: a slow block and a preempted thread look identical from
    // inside the callback, so the counters come from procfs instead of from guesswork.
    let mut previous_pressure: Option<dj_audio_io::ThreadPressure> = None;

    loop {
        let elapsed = started.elapsed().as_secs_f64();
        if elapsed >= total {
            break;
        }
        std::thread::sleep(tick.min(Duration::from_secs_f64(total - elapsed)));
        let report = output.snapshot();
        eprintln!(
            "dj-live-probe: {:.0}s of {:.0}s | audio {:.1}s | callbacks {} | late {} | errors {} | underruns {} | max {:.3} ms | rss {} kB",
            started.elapsed().as_secs_f64().min(total),
            total,
            report.audio_seconds,
            report.callbacks,
            report.late_callbacks,
            report.errors,
            underruns.load(Ordering::Relaxed),
            report.interval_max_ms,
            rss_kb().map_or_else(|| "?".to_string(), |kb| kb.to_string()),
        );
        let pressure = output.audio_thread_pressure();
        let (preemptions_delta, run_delay_delta_ms) = match (pressure, previous_pressure) {
            (Some(now), Some(before)) => (
                now.nonvoluntary.saturating_sub(before.nonvoluntary),
                (now.run_delay_ns.saturating_sub(before.run_delay_ns)) as f64 / 1_000_000.0,
            ),
            _ => (0, 0.0),
        };
        if pressure.is_some() {
            previous_pressure = pressure;
        }
        timeline.push(json!({
            "at_s": started.elapsed().as_secs(),
            "callbacks": report.callbacks,
            "late": report.late_callbacks,
            "early": report.early_callbacks,
            "errors": report.errors,
            "underruns": underruns.load(Ordering::Relaxed),
            "loop_wraps": loop_wraps.load(Ordering::Relaxed),
            "interval_max_ms": report.interval_max_ms,
            "render_max_us": report.render_max_us,
            "rss_kb": rss_kb(),
            "load1": load_average().map(|load| load[0]),
            "preemptions": preemptions_delta,
            "run_delay_ms": run_delay_delta_ms,
        }));
        if let Some(path) = &options.out {
            let _ = std::fs::write(
                path,
                report_json(
                    &report,
                    &options,
                    plan,
                    &RunFacts {
                        underruns: &underruns,
                        loop_wraps: &loop_wraps,
                        rss_start_kb: rss_start,
                        timeline: &timeline,
                        load_start,
                        audio_tid: output.audio_thread_id(),
                        pressure: output.audio_thread_pressure(),
                        scheduling: output.audio_thread_scheduling(),
                    },
                ),
            );
        }
        final_report = report;
    }

    let json = report_json(
        &final_report,
        &options,
        plan,
        &RunFacts {
            underruns: &underruns,
            loop_wraps: &loop_wraps,
            rss_start_kb: rss_start,
            timeline: &timeline,
            load_start,
            audio_tid: output.audio_thread_id(),
            pressure: output.audio_thread_pressure(),
            scheduling: output.audio_thread_scheduling(),
        },
    );
    if let Some(path) = &options.out {
        let _ = std::fs::write(path, &json);
    }
    // A silent SCHED_OTHER audio thread is how a working graph still produces dropouts; say it out
    // loud rather than letting the report be the only place the fact is recorded (ADR-0010).
    let realtime = dj_audio_io::realtime_status(output.audio_thread_scheduling());
    if let Some(advice) = realtime.advice {
        eprintln!("dj-live-probe: warning: {advice}");
    }
    Ok(json)
}

/// The JSON the run is judged by, printed and optionally written to `--out`.
/// Everything the report says that does not live inside the audio library.
struct RunFacts<'a> {
    underruns: &'a Arc<AtomicU64>,
    loop_wraps: &'a Arc<AtomicU64>,
    rss_start_kb: Option<u64>,
    timeline: &'a [serde_json::Value],
    load_start: Option<[f64; 3]>,
    audio_tid: u64,
    pressure: Option<dj_audio_io::ThreadPressure>,
    scheduling: Option<dj_audio_io::ThreadScheduling>,
}

fn report_json(
    report: &dj_audio_io::LiveReport,
    options: &Options,
    plan: RoutePlan,
    facts: &RunFacts<'_>,
) -> String {
    let underruns = facts.underruns;
    let loop_wraps = facts.loop_wraps;
    let rss_start_kb = facts.rss_start_kb;
    let load_start = facts.load_start;
    let timeline = facts.timeline;
    let audio_tid = facts.audio_tid;
    let pressure = facts.pressure;
    let scheduling = facts.scheduling;
    let realtime = dj_audio_io::realtime_status(scheduling);
    json!({
        "device": report.device,
        "channels": report.channels,
        "sample_rate": report.sample_rate,
        "block_frames_requested": report.block_frames_requested,
        "thresholds": {
            "late_periods": report.thresholds.late_periods,
            "early_periods": report.thresholds.early_periods,
            "stall_us": report.thresholds.stall_us,
            "expensive_percent": report.thresholds.expensive_percent,
        },
        "settings_file": options.config.as_ref().map(|path| path.display().to_string()),
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
        "stalled_blocks": report.stalled_blocks,
        "off_cpu_max_us": report.off_cpu_max_us,
        "rss_kb": rss_kb(),
        "rss_start_kb": rss_start_kb,
        "load_average_at_start": load_start,
        "outliers": report
            .outliers
            .iter()
            .map(|outlier| json!({
                "at_s": outlier.elapsed_ms / 1000,
                "interval_us": outlier.interval_us,
                "render_us": outlier.render_us,
                "cpu_us": outlier.cpu_us,
                "off_cpu_us": outlier.render_us.saturating_sub(outlier.cpu_us),
            }))
            .collect::<Vec<_>>(),
        "outliers_dropped": report.outliers_dropped,
        "timeline": timeline,
        "audio_thread_id": audio_tid,
        // Silence must not look like success: if the callback never reported its thread, say so
        // instead of emitting `null` as if the counters were merely optional.
        "thread_pressure_unavailable": if pressure.is_none() {
            Some(if audio_tid == 0 {
                "the callback never reported its thread id"
            } else {
                "procfs counters for the callback thread are not readable"
            })
        } else {
            None
        },
        "realtime": {
            "ok": realtime.realtime,
            "policy": realtime.policy,
            "advice": realtime.advice,
        },
        "thread_scheduling": scheduling.map(|seen| json!({
            "policy": seen.policy,
            "rt_priority": seen.rt_priority,
            "nice": seen.nice,
        })),
        "thread_pressure": pressure.map(|seen| json!({
            "nonvoluntary_switches": seen.nonvoluntary,
            "run_delay_ms": seen.run_delay_ns as f64 / 1_000_000.0,
            "cpu_ms": seen.cpu_ns as f64 / 1_000_000.0,
        })),
        "deck_underruns": underruns.load(Ordering::Relaxed),
        "loop_wraps": loop_wraps.load(Ordering::Relaxed),
        "route": {
            "master": [plan.master.0, plan.master.1],
            "cue": plan.cue.map(|(left, right)| [left, right]),
        },
        "source": options.file.as_ref().map(|path| path.display().to_string()),
        "looped": options.loop_track,
        "loop_seconds": options.loop_seconds,
        "loop_buffered": options.buffered.load(Ordering::Relaxed) == 1,
        "note": "live run through the system output; not the reference rig (DJP-NFR-003 needs A11)",
    })
    .to_string()
}

/// Load average of the machine, because a measurement without its load is not comparable.
///
/// Today's lesson: every run so far happened on a machine that was also being used, and two of
/// them competed with a cargo build. Without the load in the report nobody can tell a clean
/// baseline from a busy one afterwards.
fn load_average() -> Option<[f64; 3]> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    let mut fields = text.split_whitespace();
    let one = fields.next()?.parse().ok()?;
    let five = fields.next()?.parse().ok()?;
    let fifteen = fields.next()?.parse().ok()?;
    Some([one, five, fifteen])
}

/// Resident set size in kilobytes, so a long run can show whether memory grows.
fn rss_kb() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4)
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
    /// Blocked-audio counters, shared with the report: a silent block is not a late callback,
    /// so the callback timing would look perfect while the deck was starved.
    underruns: Arc<AtomicU64>,
    /// Loop wraps, which is what a whole-track loop does every few minutes.
    loop_wraps: Arc<AtomicU64>,
    /// 1 once the loop is played from the capture buffer (no seam, ADR-0009).
    buffered: Arc<AtomicU64>,
}

impl DeckRenderer {
    fn new(
        deck: Deck,
        sample_rate: u32,
        block: usize,
        gain: f32,
        underruns: Arc<AtomicU64>,
        loop_wraps: Arc<AtomicU64>,
        buffered: Arc<AtomicU64>,
    ) -> Self {
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
            underruns,
            loop_wraps,
            buffered,
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

        let stats = self.deck.stats();
        self.underruns.store(stats.underruns, Ordering::Relaxed);
        self.loop_wraps.store(stats.loop_wraps, Ordering::Relaxed);
        self.buffered
            .store(u64::from(self.deck.loop_is_buffered()), Ordering::Relaxed);
    }
}
