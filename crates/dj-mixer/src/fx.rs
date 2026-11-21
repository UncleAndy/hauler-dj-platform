//! The deck's effect slot: seven units, one of them audible, and a switch that fades rather than steps.
//!
//! The slot sits at the head of the channel strip, before the trim, so what the DJ hears in the headphones is what
//! the effect does (monitoring is taken after the trim, which is after this), and the fader still decides how much of
//! it reaches the room (DJP-FX-003: insert).
//!
//! All seven units exist from the moment the mixer is prepared: switching between them moves an index, it does not
//! allocate, and it cannot fail on the audio path. Switching fades the dry signal out and the new one in over 20 ms —
//! the same reason the crossfader ramps: a step in gain is a click, and a click on the master is the one thing a DJ
//! never forgives.

use dj_domain::{FxKind, FxPatch};
use dj_fx::{Bitcrusher, DjFilter, Echo, Flanger, FxUnit, Noise, Phaser, Roll};

/// How long the switch takes to fade one way or the other.
const SWITCH_SECONDS: f32 = 0.020;

/// The seven units, indexable by the kind the protocol names.
///
/// The variants differ in size on purpose: the echo and the roll own seconds of delay line, the crusher owns nothing.
/// They are allocated once per strip and live in the rack, so the enum is big and never moved on the render path —
/// boxing them would trade that for a pointer hop per block, which is the wrong trade here.
#[allow(clippy::large_enum_variant)]
enum Unit {
    Filter(DjFilter),
    Echo(Echo),
    Roll(Roll),
    Flanger(Flanger),
    Phaser(Phaser),
    Crusher(Bitcrusher),
    Noise(Noise),
}

fn index_of(kind: FxKind) -> usize {
    match kind {
        FxKind::Filter => 0,
        FxKind::Echo => 1,
        FxKind::Roll => 2,
        FxKind::Flanger => 3,
        FxKind::Phaser => 4,
        FxKind::Bitcrusher => 5,
        FxKind::Noise => 6,
    }
}

impl Unit {
    fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        match self {
            Unit::Filter(unit) => unit.prepare(sample_rate, max_frames),
            Unit::Echo(unit) => unit.prepare(sample_rate, max_frames),
            Unit::Roll(unit) => unit.prepare(sample_rate, max_frames),
            Unit::Flanger(unit) => unit.prepare(sample_rate, max_frames),
            Unit::Phaser(unit) => unit.prepare(sample_rate, max_frames),
            Unit::Crusher(unit) => unit.prepare(sample_rate, max_frames),
            Unit::Noise(unit) => unit.prepare(sample_rate, max_frames),
        }
    }

    fn reset(&mut self) {
        match self {
            Unit::Filter(unit) => unit.reset(),
            Unit::Echo(unit) => unit.reset(),
            Unit::Roll(unit) => unit.reset(),
            Unit::Flanger(unit) => unit.reset(),
            Unit::Phaser(unit) => unit.reset(),
            Unit::Crusher(unit) => unit.reset(),
            Unit::Noise(unit) => unit.reset(),
        }
    }

    fn engage(&mut self, on: bool) {
        match self {
            Unit::Filter(unit) => unit.engage(on),
            Unit::Echo(unit) => unit.engage(on),
            Unit::Roll(unit) => unit.engage(on),
            Unit::Flanger(unit) => unit.engage(on),
            Unit::Phaser(unit) => unit.engage(on),
            Unit::Crusher(unit) => unit.engage(on),
            Unit::Noise(unit) => unit.engage(on),
        }
    }

    fn set_amount(&mut self, amount: f32) {
        match self {
            Unit::Filter(unit) => unit.set_amount(amount),
            Unit::Echo(unit) => unit.set_amount(amount),
            Unit::Roll(unit) => unit.set_amount(amount),
            Unit::Flanger(unit) => unit.set_amount(amount),
            Unit::Phaser(unit) => unit.set_amount(amount),
            Unit::Crusher(unit) => unit.set_amount(amount),
            Unit::Noise(unit) => unit.set_amount(amount),
        }
    }

    fn process(&mut self, input: &[f32], out: &mut [f32], frames: usize) {
        match self {
            Unit::Filter(unit) => unit.process(input, out, frames),
            Unit::Echo(unit) => unit.process(input, out, frames),
            Unit::Roll(unit) => unit.process(input, out, frames),
            Unit::Flanger(unit) => unit.process(input, out, frames),
            Unit::Phaser(unit) => unit.process(input, out, frames),
            Unit::Crusher(unit) => unit.process(input, out, frames),
            Unit::Noise(unit) => unit.process(input, out, frames),
        }
    }
}

/// One deck's effect slot.
pub(crate) struct FxRack {
    units: [Unit; 7],
    patch: FxPatch,
    /// The kind the switch is moving towards, while the dry signal fades out.
    pending_kind: Option<FxKind>,
    /// How much of the effect is in the signal, 0.0 … 1.0.
    wet: f32,
    rate: f32,
    scratch: Vec<f32>,
}

impl FxRack {
    pub(crate) fn new() -> Self {
        Self {
            units: [
                Unit::Filter(DjFilter::new()),
                Unit::Echo(Echo::new()),
                Unit::Roll(Roll::new()),
                Unit::Flanger(Flanger::new()),
                Unit::Phaser(Phaser::new()),
                Unit::Crusher(Bitcrusher::new()),
                Unit::Noise(Noise::new()),
            ],
            patch: FxPatch::default(),
            pending_kind: None,
            wet: 0.0,
            rate: 48_000.0,
            scratch: Vec::new(),
        }
    }

    pub(crate) fn prepare(&mut self, sample_rate: f32, max_frames: usize) {
        self.rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.scratch = vec![0.0; max_frames.max(1) * 2];
        for unit in &mut self.units {
            unit.prepare(self.rate, max_frames);
        }
        self.wet = 0.0;
        self.pending_kind = None;
    }

    /// What the slot is set to, as the engine publishes it.
    pub(crate) fn patch(&self) -> FxPatch {
        self.patch
    }

    pub(crate) fn set_patch(&mut self, patch: FxPatch) {
        let patch = patch.clamped();
        if patch.kind != self.patch.kind {
            // The new unit fades in only after the old one has faded out: switching mid-signal is a crossfade, not a
            // jump between two algorithms.
            self.pending_kind = Some(patch.kind);
        }
        self.patch = patch;
        // The knob reaches every unit, not only the selected one: a unit chosen later must not start from a stale
        // amount.
        for unit in &mut self.units {
            unit.set_amount(self.patch.amount);
        }
    }

    /// The deck's beat in seconds of output, for the effects that live on it.
    pub(crate) fn set_beat(&mut self, seconds_per_beat: f32) {
        for unit in &mut self.units {
            match unit {
                Unit::Echo(echo) => echo.set_beat(seconds_per_beat),
                Unit::Roll(roll) => roll.set_beat(seconds_per_beat),
                _ => {}
            }
        }
    }

    /// Process a channel's block in place.
    ///
    /// The block is what the deck rendered (post-EQ); after this it is what the strip will trim, fade and crossfade.
    pub(crate) fn process(&mut self, block: &mut [f32], frames: usize) {
        let samples = (frames * 2).min(block.len()).min(self.scratch.len());
        if samples == 0 {
            return;
        }
        let step = frames as f32 / (SWITCH_SECONDS * self.rate).max(1.0);

        // A unit change waits for the dry signal: `wet` reaching zero is the moment the old unit is silent.
        if self.wet <= 0.0
            && let Some(kind) = self.pending_kind.take()
        {
            self.units[index_of(self.patch.kind)].reset();
            self.units[index_of(kind)].reset();
            self.patch.kind = kind;
        }

        let target = if self.patch.enabled { 1.0 } else { 0.0 };
        let delta = (target - self.wet).clamp(-step, step);
        self.wet = (self.wet + delta).clamp(0.0, 1.0);
        if self.wet <= 0.0 {
            // Nothing of the effect is in the signal: the block stays exactly as the deck rendered it.
            self.units[index_of(self.patch.kind)].engage(false);
            return;
        }

        let index = index_of(self.patch.kind);
        // Roll captures the moment the switch goes down; every other unit ignores this.
        self.units[index].engage(self.patch.enabled);

        {
            let unit = &mut self.units[index];
            unit.process(block, &mut self.scratch[..samples], frames);
        }
        let wet = self.wet;
        for (dry, processed) in block.iter_mut().zip(self.scratch.iter()).take(samples) {
            *dry += (processed - *dry) * wet;
        }
    }
}
