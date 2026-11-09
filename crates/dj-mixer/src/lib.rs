//! Mixer: channel strips, crossfader, master limiter, metering, cue bus (A04).
//!
//! Signal order follows the graph contract (DJP-ARCH-006): a deck arrives already gain-staged and
//! EQ'd, the mixer applies channel trim and fader, the crossfader, then master gain and the
//! limiter, and only then are the buses routed (DJP-ARCH-007).
//!
//! Everything here is preallocated by [`Mixer::prepare`] and runs without allocation, locks or
//! panics on the data path; non-finite input is replaced with silence and counted instead of
//! propagating (DJP-NFR-012, A04 acceptance).
//!
//! Ownership: dsp (`docs/architecture/crate-map.md`).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

/// Which side of the crossfader a channel follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAssign {
    /// Left side: full level at crossfader 0.
    A,
    /// Right side: full level at crossfader 1.
    B,
    /// Bypasses the crossfader entirely (used for aux/record later).
    Thru,
}

/// Crossfader law, defined by the protocol so a client can name the one in force.
pub use dj_domain::CrossfaderCurve;

use dj_signalsmith_sys::{DEFAULT_FALL_SECONDS, LookaheadLimiter, PeakDecay};

/// Level the limiter holds, slightly below full scale to leave room for inter-sample peaks.
const LIMITER_THRESHOLD: f32 = 0.999;

/// One channel strip's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelSettings {
    /// Trim in decibels, applied before the fader.
    pub trim_db: f32,
    /// Channel fader, 0.0 … 1.0 linear.
    pub fader: f32,
    /// Crossfader side.
    pub assign: CrossAssign,
    /// True when the channel feeds the cue (headphone) bus, pre-crossfader.
    pub cue: bool,
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            trim_db: 0.0,
            fader: 1.0,
            assign: CrossAssign::Thru,
            cue: false,
        }
    }
}

/// Peak and RMS metering of one signal.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meter {
    /// Highest absolute sample in the measured block.
    pub peak: f32,
    /// Root-mean-square of the block.
    pub rms: f32,
    /// What the bar shows: the block peak held and falling back over half a second.
    ///
    /// A meter that jumps to zero between blocks is a counter, not a meter — the eye cannot read a
    /// value that lives for four milliseconds. `peak` stays per block for tests and for clipping;
    /// `held` is the value that belongs on screen.
    pub held: f32,
}

/// Metering and clip accounting for one rendered block.
#[derive(Debug, Clone, PartialEq)]
pub struct Meters {
    /// Per-channel meters, post trim/fader/crossfader.
    pub channels: Vec<Meter>,
    /// Master bus after gain and limiter.
    pub master: Meter,
    /// Cue bus.
    pub cue: Meter,
    /// Samples that had to be replaced because they were not finite.
    pub non_finite_samples: u64,
    /// Samples that reached full scale on the master bus before the limiter.
    pub clipped_samples: u64,
    /// Peak gain reduction applied by the limiter (1.0 = none).
    pub limiter_gain: f32,
}

impl Meters {
    /// True when anything reached full scale in this block.
    pub fn clipped(&self) -> bool {
        self.clipped_samples > 0
    }
}

/// Mixer state.
///
/// Usage per block: [`Mixer::begin_block`], one [`Mixer::mix_channel`] per deck, then
/// [`Mixer::finish_block`]; read the results with [`Mixer::master`], [`Mixer::cue`] and
/// [`Mixer::meters`].
pub struct Mixer {
    sample_rate: u32,
    channels: Vec<ChannelSettings>,
    block_frames: usize,
    master: Vec<f32>,
    cue: Vec<f32>,
    reference: Vec<f32>,
    crossfader: f32,
    curve: CrossfaderCurve,
    master_gain: f32,
    cue_gain: f32,
    /// The master limiter: lookahead, so the gain is down before the peak arrives.
    limiter: LookaheadLimiter,
    /// The cue bus runs through this to stay level with the master, which the limiter delays: a DJ
    /// mixing by ear must not hear the two buses a few milliseconds apart.
    cue_delay: Vec<f32>,
    cue_delay_pos: usize,
    /// One fall-back filter per metered signal: the channels, the master and the cue bus.
    channel_decay: Vec<PeakDecay>,
    master_decay: PeakDecay,
    cue_decay: PeakDecay,
    meters: Meters,
}

impl std::fmt::Debug for Mixer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mixer")
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels.len())
            .field("crossfader", &self.crossfader)
            .field("master_gain", &self.master_gain)
            .field("meters", &self.meters)
            .finish()
    }
}

impl Mixer {
    /// Build a mixer for `channels` channel strips.
    pub fn new(channels: usize) -> Self {
        Self {
            sample_rate: 48_000,
            channels: vec![ChannelSettings::default(); channels.max(1)],
            block_frames: 0,
            master: Vec::new(),
            cue: Vec::new(),
            reference: Vec::new(),
            crossfader: 0.5,
            curve: CrossfaderCurve::ConstantPower,
            master_gain: 1.0,
            cue_gain: 1.0,
            limiter: LookaheadLimiter::new(2, 256, 48_000, LIMITER_THRESHOLD),
            cue_delay: Vec::new(),
            cue_delay_pos: 0,
            channel_decay: (0..channels.max(1))
                .map(|_| PeakDecay::new(48_000, DEFAULT_FALL_SECONDS))
                .collect(),
            master_decay: PeakDecay::new(48_000, DEFAULT_FALL_SECONDS),
            cue_decay: PeakDecay::new(48_000, DEFAULT_FALL_SECONDS),
            meters: Meters {
                channels: vec![Meter::default(); channels.max(1)],
                master: Meter::default(),
                cue: Meter::default(),
                non_finite_samples: 0,
                clipped_samples: 0,
                limiter_gain: 1.0,
            },
        }
    }

    /// DJP-ARCH-009 node contract: allocate for this rate and block size, reset state.
    pub fn prepare(&mut self, sample_rate: u32, max_block_frames: usize) {
        self.sample_rate = sample_rate.max(1);
        self.block_frames = 0;
        self.master = vec![0.0; max_block_frames * 2];
        self.cue = vec![0.0; max_block_frames * 2];
        self.reference = vec![0.0; max_block_frames * 2];
        self.limiter = LookaheadLimiter::new(
            2,
            max_block_frames.max(1),
            self.sample_rate,
            LIMITER_THRESHOLD,
        );
        self.channel_decay = (0..self.channels.len())
            .map(|_| PeakDecay::new(self.sample_rate, DEFAULT_FALL_SECONDS))
            .collect();
        self.master_decay = PeakDecay::new(self.sample_rate, DEFAULT_FALL_SECONDS);
        self.cue_decay = PeakDecay::new(self.sample_rate, DEFAULT_FALL_SECONDS);
        // The cue bus gets the same delay the limiter gives the master, so the two stay aligned.
        self.cue_delay = vec![0.0; self.latency_frames() as usize * 2];
        self.cue_delay_pos = 0;
        self.meters.channels = vec![Meter::default(); self.channels.len()];
        self.meters.non_finite_samples = 0;
        self.meters.clipped_samples = 0;
        self.meters.limiter_gain = 1.0;
    }

    /// Latency the mixer adds to the graph, in frames.
    ///
    /// The limiter's lookahead: the master comes out that much later than the material that made it,
    /// which is the price of the gain being down before a peak arrives.
    pub fn latency_frames(&self) -> u32 {
        self.limiter.latency_frames()
    }

    /// Tail length after the input stops.
    pub fn tail_frames(&self) -> u32 {
        0
    }

    /// Sample rate the mixer was prepared for.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channel count.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Settings of one channel.
    pub fn channel(&self, index: usize) -> Option<&ChannelSettings> {
        self.channels.get(index)
    }

    /// Change one channel's settings. Values are sanitised here: a non-finite number keeps the
    /// previous value instead of poisoning the mix.
    pub fn set_channel(&mut self, index: usize, settings: ChannelSettings) {
        let Some(slot) = self.channels.get_mut(index) else {
            return;
        };
        if settings.trim_db.is_finite() {
            slot.trim_db = settings.trim_db.clamp(-60.0, 12.0);
        }
        if settings.fader.is_finite() {
            slot.fader = settings.fader.clamp(0.0, 1.0);
        }
        slot.assign = settings.assign;
        slot.cue = settings.cue;
    }

    /// Crossfader position, 0.0 = A, 1.0 = B. Non-finite values are ignored.
    pub fn set_crossfader(&mut self, position: f32) {
        if position.is_finite() {
            self.crossfader = position.clamp(0.0, 1.0);
        }
    }

    /// Current crossfader position.
    pub fn crossfader(&self) -> f32 {
        self.crossfader
    }

    /// Crossfader law.
    pub fn set_curve(&mut self, curve: CrossfaderCurve) {
        self.curve = curve;
    }

    /// Master gain in force, linear.
    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }

    /// Master gain, linear.
    pub fn set_master_gain(&mut self, gain: f32) {
        if gain.is_finite() {
            self.master_gain = gain.clamp(0.0, 4.0);
        }
    }

    /// Cue bus gain, linear.
    pub fn set_cue_gain(&mut self, gain: f32) {
        if gain.is_finite() {
            self.cue_gain = gain.clamp(0.0, 4.0);
        }
    }

    /// Clear the buses and the per-block counters.
    pub fn begin_block(&mut self, frames: usize) {
        let frames = frames.min(self.master.len() / 2);
        self.block_frames = frames;
        self.master[..frames * 2].fill(0.0);
        self.cue[..frames * 2].fill(0.0);
        self.meters.clipped_samples = 0;
        for meter in &mut self.meters.channels {
            // The bar keeps its value across the block boundary: only the per-block numbers start over.
            meter.peak = 0.0;
            meter.rms = 0.0;
        }
    }

    /// Mix one channel into the buses.
    ///
    /// `input` is that channel's stereo block for this callback. The trim, fader and crossfader
    /// are applied here; the cued copy goes to the cue bus pre-crossfader (PFL).
    pub fn mix_channel(&mut self, index: usize, input: &[f32]) {
        let Some(settings) = self.channels.get(index).copied() else {
            return;
        };
        let frames = self.block_frames.min(input.len() / 2);
        if frames == 0 {
            return;
        }

        let trim = db_to_gain(settings.trim_db);
        let cross = self.crossfader_gain(settings.assign);
        let channel_gain = trim * settings.fader;
        let master_gain = channel_gain * cross;

        let mut meter = Meter::default();
        let mut sum_squares = 0.0f64;

        for frame in 0..frames {
            for channel in 0..2 {
                let index = frame * 2 + channel;
                let sample = input[index];
                let sample = if sample.is_finite() {
                    sample
                } else {
                    self.meters.non_finite_samples += 1;
                    0.0
                };
                let scaled = sample * channel_gain;
                let to_master = sample * master_gain;

                let m = &mut self.master[index];
                *m += to_master;
                if to_master.abs() >= 1.0 {
                    self.meters.clipped_samples += 1;
                }
                if settings.cue {
                    self.cue[index] += scaled * self.cue_gain;
                }
                meter.peak = meter.peak.max(scaled.abs());
                sum_squares += f64::from(scaled) * f64::from(scaled);
            }
        }

        meter.rms = (sum_squares / (frames * 2) as f64).sqrt() as f32;
        if let Some(decay) = self.channel_decay.get_mut(index) {
            meter.held = decay.block(meter.peak, frames);
        }
        if let Some(slot) = self.meters.channels.get_mut(index) {
            *slot = meter;
        }
    }

    /// Apply master gain and the limiter, then measure the buses.
    pub fn finish_block(&mut self) {
        let frames = self.block_frames;
        let gain = self.master_gain;

        // Reference copy keeps the pre-limiter peak for the clip counter.
        self.reference[..frames * 2].copy_from_slice(&self.master[..frames * 2]);
        let pre_peak = self
            .reference
            .iter()
            .take(frames * 2)
            .fold(0.0f32, |acc, sample| acc.max(sample.abs() * gain));

        for sample in &mut self.master[..frames * 2] {
            *sample *= gain;
        }
        self.limiter.process(&mut self.master[..frames * 2]);
        // The library does not report its gain; the peak of the block says the same thing at the
        // resolution the meter is read at.
        let reduction = if pre_peak > LIMITER_THRESHOLD {
            (LIMITER_THRESHOLD / pre_peak).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let (peak, rms) = analyse(&self.master[..frames * 2]);
        self.meters.master = Meter {
            peak,
            rms,
            held: self.master_decay.block(peak, frames),
        };
        self.delay_cue(frames);
        let (peak, rms) = analyse(&self.cue[..frames * 2]);
        self.meters.cue = Meter {
            peak,
            rms,
            held: self.cue_decay.block(peak, frames),
        };
        self.meters.limiter_gain = reduction;
        if pre_peak > 1.0 && self.meters.clipped_samples == 0 {
            // Gain can push an otherwise legal channel over full scale.
            self.meters.clipped_samples = 1;
        }
    }

    /// Push the cue bus through the delay line that matches the limiter's lookahead.
    fn delay_cue(&mut self, frames: usize) {
        if self.cue_delay.is_empty() {
            return;
        }
        let line = self.cue_delay.len();
        for sample in 0..frames * 2 {
            let slot = self.cue_delay_pos;
            // What comes out of the line is the sample written a lookahead ago; what goes in is now.
            std::mem::swap(&mut self.cue_delay[slot], &mut self.cue[sample]);
            self.cue_delay_pos = if slot + 1 == line { 0 } else { slot + 1 };
        }
    }

    /// Master bus of the last finished block.
    pub fn master(&self) -> &[f32] {
        &self.master[..self.block_frames * 2]
    }

    /// The master bus before the master gain and the limiter.
    ///
    /// A monitor signal — the metronome — is mixed here: it is heard where the music is heard, and the
    /// limiter protects the sum instead of the DJ's ears having to protect the mix. Nothing else may
    /// write here: the bus is the mixer's own sum.
    pub fn master_bus(&mut self) -> &mut [f32] {
        &mut self.master[..self.block_frames * 2]
    }

    /// Cue bus of the last finished block.
    pub fn cue(&self) -> &[f32] {
        &self.cue[..self.block_frames * 2]
    }

    /// Metering of the last finished block.
    pub fn meters(&self) -> &Meters {
        &self.meters
    }

    /// Crossfader gain for one side, following the configured law.
    pub fn crossfader_gain(&self, assign: CrossAssign) -> f32 {
        match assign {
            CrossAssign::Thru => 1.0,
            CrossAssign::A => self.side_gain(1.0 - self.crossfader),
            CrossAssign::B => self.side_gain(self.crossfader),
        }
    }

    fn side_gain(&self, position: f32) -> f32 {
        let p = position.clamp(0.0, 1.0);
        match self.curve {
            CrossfaderCurve::Linear => p,
            CrossfaderCurve::ConstantPower => {
                // Quarter sine: at 0.5 each side sits at -3 dB, so the sum stays at unity.
                (p * std::f32::consts::FRAC_PI_2).sin()
            }
            // Smoothstep: flat at both ends, steepest in the middle. The handle does most of its
            // work over the central half of the travel.
            CrossfaderCurve::Smooth => p * p * (3.0 - 2.0 * p),
            // Cut: full until a quarter from the edge, then a hard cut. Both channels stay open
            // across the middle, which is what scratching wants.
            CrossfaderCurve::Sharp => (p / 0.25).clamp(0.0, 1.0),
        }
    }

    /// The law currently in force.
    pub fn curve(&self) -> CrossfaderCurve {
        self.curve
    }
}

/// Converts decibels to a linear gain, clamping the silent end.
pub fn db_to_gain(db: f32) -> f32 {
    if !db.is_finite() {
        return 1.0;
    }
    if db <= -60.0 {
        return 0.0;
    }
    10f32.powf(db / 20.0)
}

/// Peak and RMS of one block, without the bar's fall back.
fn analyse(block: &[f32]) -> (f32, f32) {
    let mut peak = 0.0f32;
    let mut sum_squares = 0.0f64;
    for sample in block {
        peak = peak.max(sample.abs());
        sum_squares += f64::from(*sample) * f64::from(*sample);
    }
    let rms = if block.is_empty() {
        0.0
    } else {
        (sum_squares / block.len() as f64).sqrt() as f32
    };
    (peak, rms)
}
