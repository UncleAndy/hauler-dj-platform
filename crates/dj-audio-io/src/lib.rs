//! Device output (A05): route plans, a bounded callback and honest counters.
//!
//! The engine hands this crate a [`Renderer`]; the callback asks it for one block and writes the
//! result into the device buffer according to a [`RoutePlan`]. Everything the callback touches is
//! preallocated, and every counter it updates is an atomic — no locks, no allocation (ADR-0002,
//! DJP-NFR-001).
//!
//! Ownership: architect / audio-platform (`docs/architecture/crate-map.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rustix::time::{ClockId, clock_gettime};

/// Which physical channels carry which bus (ADR-0005).
///
/// Channels are addressed by position, never by name: the S4 descriptor says `FL FR FC LFE` while
/// the PipeWire profile says `FL FR RL RR`, and only listening settles which pair is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePlan {
    /// Left and right channel of the main output.
    pub master: (usize, usize),
    /// Left and right channel of the headphones, when the device has four or more outputs.
    pub cue: Option<(usize, usize)>,
    /// Total channels to open on the device.
    pub channels: usize,
}

impl RoutePlan {
    /// The plan for a device with `channels` outputs.
    ///
    /// Two channels: master only, the cue unusable without split-cue (ADR-0005). Four or more:
    /// master on 0/1, cue on 2/3 — the arrangement verified by ear on the S4 MK3.
    pub fn for_channels(channels: usize) -> Self {
        match channels {
            0 | 1 => Self {
                master: (0, 0),
                cue: None,
                channels: channels.max(1),
            },
            2 | 3 => Self {
                master: (0, 1),
                cue: None,
                channels: 2,
            },
            _ => Self {
                master: (0, 1),
                cue: Some((2, 3)),
                channels: 4,
            },
        }
    }

    /// Number of channels the plan writes to.
    pub fn channels(&self) -> usize {
        self.channels
    }
}

/// Something that produces one block of master and cue audio.
///
/// The callback calls it with preallocated buffers: `master` and `cue` hold `frames` stereo
/// frames each. Implementations must not allocate, lock or block — the deck's blocking render is
/// for offline work, and this is not that path.
pub trait Renderer: Send {
    /// Fill `master` and `cue` for one callback.
    fn render(&mut self, master: &mut [f32], cue: &mut [f32], frames: usize);
}

/// Counters collected inside the callback.
#[derive(Debug, Default)]
struct Counters {
    callbacks: AtomicU64,
    frames: AtomicU64,
    /// Callbacks that arrived later than 1.5 periods after the previous one.
    late: AtomicU64,
    /// Callbacks that arrived earlier than half a period after the previous one.
    early: AtomicU64,
    /// Total microseconds spent inside the renderer.
    render_sum_us: AtomicU64,
    /// Longest single render, microseconds.
    render_max_us: AtomicU32,
    /// Histogram of render durations, 1 µs buckets.
    render_histogram: Vec<AtomicU32>,
    /// Backend-reported stream errors.
    errors: AtomicU64,
    /// Blocks during which the thread was off the CPU for at least `STALL_THRESHOLD_US`.
    stalled: AtomicU64,
    /// Longest stretch off the CPU, microseconds.
    off_cpu_max_us: AtomicU32,
    /// Shortest and longest callback interval, microseconds.
    min_us: AtomicU32,
    max_us: AtomicU32,
    /// Peak per output channel, as f32 bits.
    peaks: Vec<AtomicU32>,
    /// Histogram of intervals, 1 µs buckets up to 32 ms.
    histogram: Vec<AtomicU32>,
}

impl Counters {
    fn new(channels: usize) -> Self {
        Self {
            peaks: (0..channels).map(|_| AtomicU32::new(0)).collect(),
            histogram: (0..32_000).map(|_| AtomicU32::new(0)).collect(),
            render_histogram: (0..32_000).map(|_| AtomicU32::new(0)).collect(),
            min_us: AtomicU32::new(u32::MAX),
            ..Self::default()
        }
    }

    fn note_interval(&self, micros: u32, period_us: f64, thresholds: Thresholds) {
        self.min_us.fetch_min(micros, Ordering::Relaxed);
        self.max_us.fetch_max(micros, Ordering::Relaxed);
        if let Some(bucket) = self.histogram.get(micros as usize) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
        match classify_interval(micros, period_us, thresholds) {
            IntervalKind::Late => self.late.fetch_add(1, Ordering::Relaxed),
            IntervalKind::Early => self.early.fetch_add(1, Ordering::Relaxed),
            IntervalKind::OnTime => 0,
        };
    }

    fn note_peak(&self, channel: usize, value: f32) {
        if let Some(slot) = self.peaks.get(channel) {
            slot.fetch_max(value.to_bits(), Ordering::Relaxed);
        }
    }
}

/// Timing and level summary of one run.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveReport {
    /// Device name as the backend spells it.
    pub device: String,
    /// Channels opened.
    pub channels: usize,
    /// Sample rate requested and used.
    pub sample_rate: u32,
    /// Block size requested (the backend may hand a different one).
    pub block_frames_requested: usize,
    /// Frames per callback actually observed, on average.
    pub frames_per_callback: f64,
    /// Callbacks seen.
    pub callbacks: u64,
    /// Frames rendered.
    pub frames: u64,
    /// Seconds of audio produced.
    pub audio_seconds: f64,
    /// Callbacks that arrived more than 1.5 periods late.
    pub late_callbacks: u64,
    /// Callbacks that arrived less than half a period after the previous one.
    pub early_callbacks: u64,
    /// Backend-reported stream errors.
    pub errors: u64,
    /// Shortest and longest callback interval, milliseconds.
    pub interval_min_ms: f64,
    /// Longest callback interval, milliseconds.
    pub interval_max_ms: f64,
    /// Median and 99th percentile callback interval, milliseconds.
    pub interval_p50_ms: f64,
    /// 99th percentile callback interval, milliseconds.
    pub interval_p99_ms: f64,
    /// Peak of every output channel, in plan order.
    pub peaks: Vec<f32>,
    /// Median time spent inside the renderer per block, microseconds.
    pub render_p50_us: f64,
    /// 99th percentile render time, microseconds.
    pub render_p99_us: f64,
    /// Longest single render, microseconds.
    pub render_max_us: f64,
    /// Share of the period the p99 render took: the figure DJP-NFR-003 judges.
    pub render_budget: f64,
    /// Blocks during which the thread was off the CPU for 500 us or more.
    pub stalled_blocks: u64,
    /// Longest stretch spent off the CPU inside a block, microseconds.
    pub off_cpu_max_us: u32,
    /// The thresholds this run was judged against.
    pub thresholds: Thresholds,
    /// Blocks that were late, early, or expensive, with the moment they happened.
    pub outliers: Vec<Outlier>,
    /// Outlier records dropped because the log filled up.
    pub outliers_dropped: u64,
}

impl LiveReport {
    /// True when the backend honoured the requested block size within 1 %.
    pub fn honoured_block(&self) -> bool {
        (self.frames_per_callback - self.block_frames_requested as f64).abs() < 0.01
    }
}

/// What the backend says about a device before a stream is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceShape {
    /// Name as the backend spells it.
    pub name: String,
    /// Channels the plan should open: 2 for stereo devices, 4 for wider ones (ADR-0005).
    pub channels: usize,
    /// Rate the device prefers.
    pub sample_rate: u32,
}

/// Resolve a device by name substring and report its shape.
///
/// The plan has to match what the device offers, so this runs before [`LiveOutput::start`]: a
/// four-channel interface will not accept a two-channel stream, and a stereo sink has no cue pair
/// to write to.
pub fn resolve_device(device: Option<&str>) -> Result<DeviceShape, IoError> {
    let host = cpal::default_host();
    let device = match device {
        Some(needle) => {
            let needle_lower = needle.to_ascii_lowercase();
            let devices = host
                .output_devices()
                .map_err(|err| IoError::Unsupported(err.to_string()))?;
            let mut found = None;
            for candidate in devices {
                if candidate
                    .to_string()
                    .to_ascii_lowercase()
                    .contains(&needle_lower)
                {
                    found = Some(candidate);
                    break;
                }
            }
            found.ok_or_else(|| IoError::DeviceNotFound(needle.to_string()))?
        }
        None => host
            .default_output_device()
            .ok_or_else(|| IoError::DeviceNotFound("default".to_string()))?,
    };

    let name = device.to_string();
    let (channels, sample_rate) = match device.default_output_config() {
        Ok(config) => {
            let native = usize::from(config.channels());
            // Only the widths the route plan knows how to use: stereo, or master + cue.
            let width = if native >= 4 {
                4
            } else if native >= 2 {
                2
            } else {
                1
            };
            (width, config.sample_rate())
        }
        Err(_) => (2, 48_000),
    };

    Ok(DeviceShape {
        name,
        channels,
        sample_rate,
    })
}

/// Percentiles of a microsecond histogram.
fn percentiles(histogram: &[AtomicU32], low: f64, high: f64) -> (f64, f64) {
    let total: u64 = histogram
        .iter()
        .map(|bucket| u64::from(bucket.load(Ordering::Relaxed)))
        .sum();
    if total == 0 {
        return (0.0, 0.0);
    }
    let mut seen = 0u64;
    let mut low_value = 0.0;
    let mut high_value = 0.0;
    let mut low_done = false;
    let high_done = false;
    for (micros, bucket) in histogram.iter().enumerate() {
        let count = u64::from(bucket.load(Ordering::Relaxed));
        if count == 0 {
            continue;
        }
        seen += count;
        let fraction = seen as f64 / total as f64;
        if !low_done && fraction >= low {
            low_value = micros as f64;
            low_done = true;
        }
        if !high_done && fraction >= high {
            high_value = micros as f64;
            break;
        }
    }
    (low_value, high_value)
}

/// CPU time consumed by the calling thread, in nanoseconds.
///
/// The difference against wall time is the whole answer to "was the block slow, or was the thread
/// gone?": a block that takes 2.7 ms of wall time but 20 us of CPU was not executing our code.
fn thread_cpu_ns() -> u64 {
    let now = clock_gettime(ClockId::ThreadCPUTime);
    u64::try_from(now.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::try_from(now.tv_nsec).unwrap_or(0))
}

/// What the kernel says about the thread the callback runs on.
///
/// A block that costs 3 ms may be slow code or a thread that was taken off the CPU. The two look
/// identical from inside the callback, because it cannot observe its own absence — so the kernel is
/// asked instead: `nonvoluntary` counts times it was descheduled, `run_delay_ns` is how long it
/// waited to be scheduled again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThreadPressure {
    /// Involuntary context switches.
    pub nonvoluntary: u64,
    /// Nanoseconds spent waiting on a run queue.
    pub run_delay_ns: u64,
    /// Nanoseconds spent running on a CPU.
    pub cpu_ns: u64,
}

/// Parse `nonvoluntary_ctxt_switches` out of `/proc/<pid>/task/<tid>/status`.
fn parse_nonvoluntary(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("nonvoluntary_ctxt_switches:"))
        .and_then(|value| value.trim().parse().ok())
}

/// Parse `/proc/<pid>/task/<tid>/schedstat`: cpu nanoseconds, run-queue delay, timeslices.
fn parse_schedstat(schedstat: &str) -> Option<(u64, u64, u64)> {
    let mut fields = schedstat.split_whitespace();
    let cpu = fields.next()?.parse().ok()?;
    let delay = fields.next()?.parse().ok()?;
    let slices = fields.next()?.parse().ok()?;
    Some((cpu, delay, slices))
}

/// How the kernel schedules the thread the callback runs on.
///
/// Preemptions are inevitable for `SCHED_OTHER` and rare for `SCHED_FIFO`; which one we got decides
/// whether the fix is ours or the deployment's, so it is measured rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadScheduling {
    /// `other`, `fifo`, `round-robin`, `batch`, `idle` or `unknown`.
    pub policy: &'static str,
    /// Real-time priority, 0 when the thread is not a real-time one.
    pub rt_priority: u64,
    /// Nice value, from -20 to 19.
    pub nice: i64,
}

/// Parse `/proc/<pid>/task/<tid>/stat`: fields 19 (nice), 40 (rt_priority) and 41 (policy).
///
/// The command name in field 2 may contain spaces and parentheses, so parsing starts after the
/// last `)` — the classic trap in this file.
fn parse_scheduling(stat: &str) -> Option<ThreadScheduling> {
    let tail = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // After the command name field, `nice` is the 17th token and `rt_priority` the 38th.
    let nice: i64 = fields.get(16)?.parse().ok()?;
    let rt_priority: u64 = fields.get(37)?.parse().ok()?;
    let policy: u64 = fields.get(38)?.parse().ok()?;
    let policy = match policy {
        0 => "other",
        1 => "fifo",
        2 => "round-robin",
        3 => "batch",
        5 => "idle",
        6 => "deadline",
        _ => "unknown",
    };
    Some(ThreadScheduling {
        policy,
        rt_priority,
        nice,
    })
}

/// The lines a run is judged against.
///
/// Defaults come from the A05 measurements, but they are settings: if a later test disagrees — a
/// hungrier desktop, a bigger buffer, a slower machine — the line moves without a rebuild, and the
/// report carries the values it was judged by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// A callback interval above this many periods counts as late.
    pub late_periods: f64,
    /// Below this many periods counts as early.
    pub early_periods: f64,
    /// A block with at least this long off the CPU counts as stalled.
    pub stall_us: u32,
    /// A block costing more than this share of the period is recorded as expensive.
    pub expensive_percent: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            late_periods: 1.5,
            early_periods: 0.5,
            stall_us: 500,
            expensive_percent: 1.0,
        }
    }
}

/// The verdict the application is allowed to draw about real-time scheduling (ADR-0010).
///
/// The application never grants itself priority: the packaging does (`CPUSchedulingPolicy=fifo`
/// on a systemd unit, MMCSS on Windows, a time-constraint policy on macOS). What the application
/// must do is notice when it did not get it and say so, because a silent `SCHED_OTHER` audio thread
/// is exactly how a working graph produces dropouts on a busy desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeStatus {
    /// True when the thread actually runs under a real-time policy.
    pub realtime: bool,
    /// Policy as the kernel names it, or `unknown` when procfs could not be read.
    pub policy: &'static str,
    /// What to tell the user or the log when the answer is no.
    pub advice: Option<&'static str>,
}

/// Judge a thread's scheduling against the RT requirement.
pub fn realtime_status(scheduling: Option<ThreadScheduling>) -> RealtimeStatus {
    match scheduling {
        Some(seen) if seen.policy == "fifo" || seen.policy == "round-robin" => RealtimeStatus {
            realtime: true,
            policy: seen.policy,
            advice: None,
        },
        Some(seen) => RealtimeStatus {
            realtime: false,
            policy: seen.policy,
            advice: Some(
                "the audio thread runs without real-time priority; expect dropouts on a busy machine. \
                 Run the engine under a unit with CPUSchedulingPolicy=fifo (see docs/operations/realtime-audio.md)",
            ),
        },
        None => RealtimeStatus {
            realtime: false,
            policy: "unknown",
            advice: Some(
                "the scheduling policy of the audio thread could not be read; \
                 this platform reports it through procfs only",
            ),
        },
    }
}

/// Scheduling policy and priority of one thread.
pub fn thread_scheduling(tid: u64) -> Option<ThreadScheduling> {
    if tid == 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat")).ok()?;
    parse_scheduling(&stat)
}

/// Kernel counters for one thread. `None` when the platform has no procfs.
pub fn thread_pressure(tid: u64) -> Option<ThreadPressure> {
    if tid == 0 {
        return None;
    }
    let status = std::fs::read_to_string(format!("/proc/self/task/{tid}/status")).ok()?;
    let schedstat = std::fs::read_to_string(format!("/proc/self/task/{tid}/schedstat")).ok()?;
    let (cpu_ns, run_delay_ns, _) = parse_schedstat(&schedstat)?;
    Some(ThreadPressure {
        nonvoluntary: parse_nonvoluntary(&status)?,
        run_delay_ns,
        cpu_ns,
    })
}

/// Number of outlier records kept for diagnosis.
const OUTLIER_SLOTS: usize = 32;

/// One block that misbehaved, with the time it happened.
///
/// A count alone cannot be attributed after the fact: "22 late callbacks" over two hours says
/// nothing about whether they clustered at loop wraps, at a device event, or at random. The time
/// makes the difference visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outlier {
    /// Milliseconds since the stream started.
    pub elapsed_ms: u64,
    /// Interval since the previous callback, microseconds.
    pub interval_us: u32,
    /// Time spent inside the renderer for this block, microseconds.
    pub render_us: u32,
    /// Of that, time the thread was actually running on a CPU.
    pub cpu_us: u32,
}

/// Fixed-size log of misbehaving blocks, written with atomics only.
#[derive(Debug)]
struct Outliers {
    index: AtomicU32,
    elapsed_us: Vec<AtomicU32>,
    interval_us: Vec<AtomicU32>,
    render_us: Vec<AtomicU32>,
    cpu_us: Vec<AtomicU32>,
}

impl Default for Outliers {
    fn default() -> Self {
        Self {
            index: AtomicU32::new(0),
            elapsed_us: (0..OUTLIER_SLOTS).map(|_| AtomicU32::new(0)).collect(),
            interval_us: (0..OUTLIER_SLOTS).map(|_| AtomicU32::new(0)).collect(),
            render_us: (0..OUTLIER_SLOTS).map(|_| AtomicU32::new(0)).collect(),
            cpu_us: (0..OUTLIER_SLOTS).map(|_| AtomicU32::new(0)).collect(),
        }
    }
}

impl Outliers {
    fn note(&self, elapsed_us: u32, interval_us: u32, render_us: u32, cpu_us: u32) {
        let slot = self.index.fetch_add(1, Ordering::Relaxed) as usize % OUTLIER_SLOTS;
        self.elapsed_us[slot].store(elapsed_us, Ordering::Relaxed);
        self.interval_us[slot].store(interval_us, Ordering::Relaxed);
        self.render_us[slot].store(render_us, Ordering::Relaxed);
        self.cpu_us[slot].store(cpu_us, Ordering::Relaxed);
    }

    fn collected(&self) -> (Vec<Outlier>, u64) {
        let total = u64::from(self.index.load(Ordering::Relaxed));
        let kept = (total as usize).min(OUTLIER_SLOTS);
        let outliers = (0..kept)
            .map(|slot| Outlier {
                elapsed_ms: u64::from(self.elapsed_us[slot].load(Ordering::Relaxed)) / 1000,
                interval_us: self.interval_us[slot].load(Ordering::Relaxed),
                render_us: self.render_us[slot].load(Ordering::Relaxed),
                cpu_us: self.cpu_us[slot].load(Ordering::Relaxed),
            })
            .collect();
        (outliers, total.saturating_sub(kept as u64))
    }
}

/// How a callback interval compares to the expected period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalKind {
    /// Within half a period of the expectation.
    OnTime,
    /// Later than 1.5 periods — the buffer lost ground.
    Late,
    /// Earlier than half a period — a spurious or catch-up callback.
    Early,
}

/// Classify one callback interval.
///
/// Separated out and tested because it is the number the report is read for: a run that says
/// "0 late" must actually mean no callback lost ground, not "the backend printed no errors".
pub fn classify_interval(micros: u32, period_us: f64, thresholds: Thresholds) -> IntervalKind {
    if period_us <= 0.0 {
        return IntervalKind::OnTime;
    }
    let ratio = f64::from(micros) / period_us;
    if ratio > thresholds.late_periods {
        IntervalKind::Late
    } else if ratio < thresholds.early_periods {
        IntervalKind::Early
    } else {
        IntervalKind::OnTime
    }
}

/// What can go wrong opening or running a device.
#[derive(Debug)]
pub enum IoError {
    /// No output device matched the requested name.
    DeviceNotFound(String),
    /// The backend refused the configuration.
    Unsupported(String),
    /// The stream was built but failed to start.
    Start(String),
    /// The backend reported an error while running.
    Stream(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::DeviceNotFound(name) => write!(f, "no output device matching {name:?}"),
            IoError::Unsupported(detail) => write!(f, "unsupported configuration: {detail}"),
            IoError::Start(detail) => write!(f, "stream did not start: {detail}"),
            IoError::Stream(detail) => write!(f, "stream error: {detail}"),
        }
    }
}

impl std::error::Error for IoError {}

/// Configuration for one live run.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveConfig {
    /// Device name substring; `None` takes the default output.
    pub device: Option<String>,
    /// Sample rate to request.
    pub sample_rate: u32,
    /// Block size to request.
    pub block_frames: usize,
    /// Seconds to run.
    pub seconds: f64,
    /// Thresholds to judge the run by.
    pub thresholds: Thresholds,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: 48_000,
            block_frames: 256,
            seconds: 10.0,
            thresholds: Thresholds::default(),
        }
    }
}

/// A running output stream. Dropping it stops playback.
pub struct LiveOutput {
    _stream: cpal::Stream,
    counters: Arc<Counters>,
    outliers: Arc<Outliers>,
    /// Thresholds this run is judged by.
    thresholds: Thresholds,
    /// Thread the callback runs on, filled from inside it on the first block.
    ///
    /// Kept as the shared atomic rather than as a copy: the value does not exist yet when the
    /// stream is built, and a copy taken then would read zero for the whole run.
    audio_tid: Arc<std::sync::atomic::AtomicU64>,
    device: String,
    channels: usize,
    sample_rate: u32,
    block_frames: usize,
}

impl LiveOutput {
    /// Open a device and start rendering.
    ///
    /// `plan` decides which channels carry master and cue; `renderer` is moved into the callback.
    pub fn start(
        config: &LiveConfig,
        plan: RoutePlan,
        renderer: Box<dyn Renderer>,
    ) -> Result<Self, IoError> {
        let host = cpal::default_host();
        let device = match &config.device {
            Some(needle) => {
                let needle_lower = needle.to_ascii_lowercase();
                let devices = host
                    .output_devices()
                    .map_err(|err| IoError::Unsupported(err.to_string()))?;
                let mut found = None;
                for candidate in devices {
                    let name = candidate.to_string();
                    if name.to_ascii_lowercase().contains(&needle_lower) {
                        found = Some(candidate);
                        break;
                    }
                }
                found.ok_or_else(|| IoError::DeviceNotFound(needle.clone()))?
            }
            None => host
                .default_output_device()
                .ok_or_else(|| IoError::DeviceNotFound("default".to_string()))?,
        };

        let name = device.to_string();
        let channels = u16::try_from(plan.channels()).unwrap_or(2);
        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate: config.sample_rate,
            buffer_size: cpal::BufferSize::Fixed(u32::try_from(config.block_frames).unwrap_or(256)),
        };

        let counters = Arc::new(Counters::new(usize::from(channels)));
        let callback_counters = Arc::clone(&counters);
        let mut renderer = renderer;
        let mut master = vec![0.0f32; config.block_frames.max(1) * 2];
        let mut cue = vec![0.0f32; config.block_frames.max(1) * 2];
        let started = Instant::now();
        let mut last: Option<Instant> = None;
        let outliers = Arc::new(Outliers::default());
        let callback_outliers = Arc::clone(&outliers);
        let tid = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let callback_tid = Arc::clone(&tid);
        let mut frames_total = 0u64;
        let mut callbacks = 0u64;
        let period_us =
            config.block_frames.max(1) as f64 / f64::from(config.sample_rate) * 1_000_000.0;
        let thresholds = config.thresholds;

        let data_fn = move |buffer: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // The thread's identity is unambiguous only from inside it, and only worth reading once.
            if callback_tid.load(Ordering::Relaxed) == 0 {
                callback_tid.store(current_thread_id(), Ordering::Relaxed);
            }
            let now = Instant::now();
            // The interval before the first callback measures stream start-up, not the callback
            // cadence: counting it would report a fake stall at the beginning of every run.
            let mut interval_us = 0u32;
            if let Some(previous) = last {
                let micros = now.duration_since(previous).as_micros();
                interval_us = u32::try_from(micros).unwrap_or(u32::MAX);
                callback_counters.note_interval(interval_us, period_us, thresholds);
            }
            last = Some(now);

            let frames = buffer.len() / usize::from(channels);
            if master.len() < frames * 2 {
                // Never grow inside the callback: a larger block than prepared is rendered as the
                // part we have and the rest stays silent, which is visible in the counters.
                master.resize(frames * 2, 0.0);
                cue.resize(frames * 2, 0.0);
            }
            master[..frames * 2].fill(0.0);
            cue[..frames * 2].fill(0.0);
            let cpu_before = thread_cpu_ns();
            let render_started = Instant::now();
            renderer.render(&mut master[..frames * 2], &mut cue[..frames * 2], frames);
            // What the graph actually cost, as opposed to when the callback arrived: this is the
            // number DJP-NFR-003 is about ("p99 block processing under 60 % of the period").
            let spent = u32::try_from(render_started.elapsed().as_micros()).unwrap_or(u32::MAX);
            // The split that identifies the fault: wall time without CPU time is time the thread
            // was not running, and no amount of optimisation removes it.
            let cpu_us = u32::try_from(thread_cpu_ns().saturating_sub(cpu_before) / 1000)
                .unwrap_or(u32::MAX)
                .min(spent);
            let off_cpu_us = spent.saturating_sub(cpu_us);
            if off_cpu_us >= thresholds.stall_us {
                callback_counters.stalled.fetch_add(1, Ordering::Relaxed);
                callback_counters
                    .off_cpu_max_us
                    .fetch_max(off_cpu_us, Ordering::Relaxed);
            }
            callback_counters
                .render_sum_us
                .fetch_add(u64::from(spent), Ordering::Relaxed);
            callback_counters
                .render_max_us
                .fetch_max(spent, Ordering::Relaxed);
            if let Some(bucket) = callback_counters.render_histogram.get(spent as usize) {
                bucket.fetch_add(1, Ordering::Relaxed);
            }

            let (master_l, master_r) = plan.master;
            let cue_channels = plan.cue;
            for frame in 0..frames {
                let base = frame * 2;
                for channel in 0..usize::from(channels) {
                    let value = if channel == master_l {
                        master[base]
                    } else if channel == master_r {
                        master[base + 1]
                    } else if let Some((cue_l, cue_r)) = cue_channels {
                        if channel == cue_l {
                            cue[base]
                        } else if channel == cue_r {
                            cue[base + 1]
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    buffer[frame * usize::from(channels) + channel] = value;
                    callback_counters.note_peak(channel, value.abs());
                }
            }

            // Record what would otherwise be an unexplained count: a late arrival and an
            // expensive block are different faults and the timestamp tells them apart.
            let interval_kind = classify_interval(interval_us, period_us, thresholds);
            let expensive = period_us > 0.0
                && f64::from(spent) * 100.0 >= period_us * thresholds.expensive_percent;
            if interval_us != 0 && (interval_kind != IntervalKind::OnTime || expensive) {
                callback_outliers.note(
                    u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX),
                    interval_us,
                    spent,
                    cpu_us,
                );
            }

            callbacks += 1;
            frames_total += frames as u64;
            callback_counters
                .callbacks
                .store(callbacks, Ordering::Relaxed);
            callback_counters
                .frames
                .store(frames_total, Ordering::Relaxed);
        };

        let error_counters = Arc::clone(&counters);
        let stream = device
            .build_output_stream(
                stream_config,
                data_fn,
                move |err| {
                    error_counters.errors.fetch_add(1, Ordering::Relaxed);
                    eprintln!("dj-audio-io: stream error: {err}");
                },
                None,
            )
            .map_err(|err| IoError::Unsupported(err.to_string()))?;

        stream
            .play()
            .map_err(|err| IoError::Start(err.to_string()))?;

        Ok(Self {
            _stream: stream,
            counters,
            outliers,
            thresholds,
            audio_tid: tid,
            device: name,
            channels: usize::from(channels),
            sample_rate: config.sample_rate,
            block_frames: config.block_frames,
        })
    }

    /// Wait for the run to finish and produce the report.
    pub fn finish(self, seconds: f64) -> LiveReport {
        std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
        self.snapshot()
    }

    /// Thread the audio callback runs on, or 0 when it has not run yet.
    ///
    /// Zero is the honest answer before the first block: the id is read from inside the callback,
    /// and a copy taken while the stream is being built would be zero for the whole run.
    pub fn audio_thread_id(&self) -> u64 {
        self.audio_tid.load(Ordering::Relaxed)
    }

    /// Kernel view of that thread: preemptions and run-queue delay.
    pub fn audio_thread_pressure(&self) -> Option<ThreadPressure> {
        thread_pressure(self.audio_thread_id())
    }

    /// Scheduling policy the audio thread ended up with.
    pub fn audio_thread_scheduling(&self) -> Option<ThreadScheduling> {
        thread_scheduling(self.audio_thread_id())
    }

    /// Report on the run so far, without stopping it.
    ///
    /// A two-hour run has to be observable while it happens: if it ends badly, the numbers up to
    /// that moment are the evidence.
    pub fn snapshot(&self) -> LiveReport {
        let callbacks = self.counters.callbacks.load(Ordering::Relaxed);
        let frames = self.counters.frames.load(Ordering::Relaxed);
        let frames_per_callback = if callbacks == 0 {
            0.0
        } else {
            frames as f64 / callbacks as f64
        };
        let (p50_us, p99_us) = percentiles(&self.counters.histogram, 0.5, 0.99);
        let p50 = p50_us / 1000.0;
        let p99 = p99_us / 1000.0;

        let (render_p50, render_p99) = percentiles(&self.counters.render_histogram, 0.5, 0.99);
        let (outliers, outliers_dropped) = self.outliers.collected();
        let render_max = f64::from(self.counters.render_max_us.load(Ordering::Relaxed));
        let period_us = if frames_per_callback > 0.0 {
            frames_per_callback / f64::from(self.sample_rate) * 1_000_000.0
        } else {
            0.0
        };

        LiveReport {
            device: self.device.clone(),
            channels: self.channels,
            sample_rate: self.sample_rate,
            block_frames_requested: self.block_frames,
            frames_per_callback,
            callbacks,
            frames,
            audio_seconds: frames as f64 / f64::from(self.sample_rate),
            late_callbacks: self.counters.late.load(Ordering::Relaxed),
            early_callbacks: self.counters.early.load(Ordering::Relaxed),
            errors: self.counters.errors.load(Ordering::Relaxed),
            interval_min_ms: f64::from(self.counters.min_us.load(Ordering::Relaxed)) / 1000.0,
            interval_max_ms: f64::from(self.counters.max_us.load(Ordering::Relaxed)) / 1000.0,
            interval_p50_ms: p50,
            interval_p99_ms: p99,
            peaks: self
                .counters
                .peaks
                .iter()
                .map(|slot| f32::from_bits(slot.load(Ordering::Relaxed)))
                .collect(),
            render_p50_us: render_p50,
            render_p99_us: render_p99,
            render_max_us: render_max,
            render_budget: if period_us > 0.0 {
                render_p99 / period_us
            } else {
                0.0
            },
            thresholds: self.thresholds,
            stalled_blocks: self.counters.stalled.load(Ordering::Relaxed),
            off_cpu_max_us: self.counters.off_cpu_max_us.load(Ordering::Relaxed),
            outliers,
            outliers_dropped,
        }
    }
}

/// This thread's id as the kernel numbers it.
fn current_thread_id() -> u64 {
    std::fs::read_link("/proc/thread-self")
        .ok()
        .and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse().ok())
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_route_plan_follows_the_device_width() {
        let two = RoutePlan::for_channels(2);
        assert_eq!(two.master, (0, 1));
        assert_eq!(two.cue, None);
        assert_eq!(two.channels(), 2);

        let four = RoutePlan::for_channels(4);
        assert_eq!(four.master, (0, 1));
        assert_eq!(four.cue, Some((2, 3)));
        assert_eq!(four.channels(), 4);

        let many = RoutePlan::for_channels(8);
        assert_eq!(many.channels(), 4, "R0 opens at most master + cue");
    }

    /// Build a `/proc/<pid>/task/<tid>/stat` line with the fields this parser reads.
    ///
    /// The layout is the point: `nice` is field 19, `rt_priority` 40, `policy` 41, and the command
    /// name in field 2 may contain spaces and parentheses — which is why parsing starts at the last
    /// `)` rather than by splitting the whole line.
    fn stat_line(nice: i64, rt_priority: u64, policy: u64) -> String {
        let mut fields = vec!["0".to_string(); 40];
        fields[0] = "S".to_string();
        fields[16] = nice.to_string();
        fields[37] = rt_priority.to_string();
        fields[38] = policy.to_string();
        format!("1234 (cpal audio thread) {}", fields.join(" "))
    }

    #[test]
    fn realtime_scheduling_is_judged_not_assumed() {
        let fifo = RealtimeStatus {
            realtime: true,
            policy: "fifo",
            advice: None,
        };
        assert_eq!(
            realtime_status(Some(ThreadScheduling {
                policy: "fifo",
                rt_priority: 20,
                nice: 0,
            })),
            fifo
        );

        let ordinary = realtime_status(Some(ThreadScheduling {
            policy: "other",
            rt_priority: 0,
            nice: 0,
        }));
        assert!(!ordinary.realtime);
        assert_eq!(ordinary.policy, "other");
        assert!(
            ordinary
                .advice
                .is_some_and(|text| text.contains("CPUSchedulingPolicy=fifo")),
            "the advice must name the fix, not just the fault"
        );

        let unreadable = realtime_status(None);
        assert!(!unreadable.realtime, "silence must not pass for success");
        assert_eq!(unreadable.policy, "unknown");
        assert!(unreadable.advice.is_some());
    }

    #[test]
    fn the_scheduling_fields_are_parsed() {
        let Some(ordinary) = parse_scheduling(&stat_line(0, 0, 0)) else {
            panic!("the fields must parse");
        };
        assert_eq!(ordinary.policy, "other");
        assert_eq!(ordinary.rt_priority, 0);
        assert_eq!(ordinary.nice, 0);

        let Some(realtime) = parse_scheduling(&stat_line(-10, 20, 1)) else {
            panic!("the fields must parse");
        };
        assert_eq!(realtime.policy, "fifo");
        assert_eq!(realtime.rt_priority, 20);
        assert_eq!(realtime.nice, -10);

        assert_eq!(parse_scheduling("garbage"), None);
    }

    #[test]
    fn our_own_thread_scheduling_is_readable() {
        let tid = current_thread_id();
        let Some(scheduling) = thread_scheduling(tid) else {
            panic!("procfs describes this thread");
        };
        assert!(
            ["other", "fifo", "round-robin", "batch", "idle", "deadline"]
                .contains(&scheduling.policy),
            "unexpected policy name {:?}",
            scheduling.policy
        );
    }

    #[test]
    fn the_kernel_counters_are_parsed() {
        let status = "Name:\tcpal\nState:\tS (sleeping)\nvoluntary_ctxt_switches:\t12\nnonvoluntary_ctxt_switches:\t7\n";
        assert_eq!(parse_nonvoluntary(status), Some(7));
        assert_eq!(parse_nonvoluntary("no counters here"), None);

        assert_eq!(parse_schedstat("1000 250 3\n"), Some((1000, 250, 3)));
        assert_eq!(parse_schedstat("1000 250"), None);
    }

    #[test]
    fn this_thread_has_a_kernel_counters_entry() {
        let tid = current_thread_id();
        assert!(
            tid > 0,
            "procfs must be readable on the platform we support"
        );
        // The counters exist and parse. `cpu_ns` is deliberately not asserted positive: the kernel
        // charges a timeslice only once the thread has used one, so a short test thread reads zero.
        let Some(pressure) = thread_pressure(tid) else {
            panic!("counters for our own thread must exist");
        };
        assert!(
            pressure.run_delay_ns < 1_000_000_000,
            "a sane run-queue delay"
        );
    }

    #[test]
    fn intervals_are_judged_against_the_period() {
        let period = 5_333.33;
        let thresholds = Thresholds::default();
        assert_eq!(
            classify_interval(5_333, period, thresholds),
            IntervalKind::OnTime
        );
        assert_eq!(
            classify_interval(5_352, period, thresholds),
            IntervalKind::OnTime
        );
        // 1.5 periods is 8 000.0 µs exactly, and the boundary counts as late.
        assert_eq!(
            classify_interval(7_900, period, thresholds),
            IntervalKind::OnTime
        );
        assert_eq!(
            classify_interval(8_000, period, thresholds),
            IntervalKind::Late
        );
        assert_eq!(
            classify_interval(12_836, period, thresholds),
            IntervalKind::Late
        );
        assert_eq!(
            classify_interval(2_600, period, thresholds),
            IntervalKind::Early
        );
        assert_eq!(
            classify_interval(5_333, 0.0, thresholds),
            IntervalKind::OnTime
        );

        // The line is a setting: a run may be judged against a stricter one.
        let strict = Thresholds {
            late_periods: 1.1,
            ..Thresholds::default()
        };
        assert_eq!(
            classify_interval(6_500, period, strict),
            IntervalKind::Late,
            "6 500 µs is 1.22 periods: late under 1.1, fine under 1.5"
        );
        assert_eq!(
            classify_interval(6_500, period, thresholds),
            IntervalKind::OnTime
        );
    }

    #[test]
    fn a_single_channel_device_still_gets_a_plan() {
        let mono = RoutePlan::for_channels(1);
        assert_eq!(mono.master, (0, 0));
        assert_eq!(mono.cue, None);
        assert_eq!(mono.channels(), 1);
    }
}
