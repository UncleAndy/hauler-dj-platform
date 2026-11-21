//! Monitoring on a second device: a ring, an adaptive resampler and the controller that keeps them together.
//!
//! Two sound cards have two crystals. They agree on the nominal rate and disagree about everything else — tens of parts
//! per million, which is a few milliseconds a minute, which is a click or drift by the end of a set. So the cue does not
//! travel as a copy of the other stream: the main callback produces it, the cue device consumes it, and the consumer's
//! read pointer runs at an ever-so-slightly different ratio so that the buffer between them keeps its depth.
//!
//! Nothing here allocates or locks on the audio path: the ring is a slab of atomics, the resampler is four samples of
//! history per channel, and the controller is one multiply.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Stereo frames of cue audio waiting for the second device.
///
/// Every slot is an atomic word holding an `f32`'s bits, so the ring is safe without `unsafe` — which this workspace
/// forbids outright — and still lock-free. The cost is an atomic store per sample; at 48 kHz that is nothing next to the
/// mixing that produced it.
pub struct CueRing {
    slots: Box<[AtomicU32]>,
    /// Frames the ring can hold; one slot stays empty so that a full ring and an empty one cannot look the same.
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    dropped: AtomicU64,
}

impl CueRing {
    /// A ring for `frames` stereo frames, rounded up to a power of two.
    pub fn new(frames: usize) -> Arc<Self> {
        let wanted = frames.max(64).next_power_of_two();
        let slots: Vec<AtomicU32> = (0..wanted * 2).map(|_| AtomicU32::new(0)).collect();
        Arc::new(Self {
            slots: slots.into_boxed_slice(),
            capacity: wanted - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Frames waiting to be played. The producer writes the head, so the consumer reads it after its own tail.
    pub fn fill(&self) -> usize {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        head.wrapping_sub(tail) & self.capacity
    }

    /// Frames the consumer never got because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Push one frame. `false` when the ring is full — the caller counts the loss, it does not wait.
    pub fn push(&self, left: f32, right: f32) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = head.wrapping_add(1) & self.capacity;
        if next == self.tail.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.slots[head * 2].store(left.to_bits(), Ordering::Relaxed);
        self.slots[head * 2 + 1].store(right.to_bits(), Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
        true
    }

    /// Take one frame, or `None` when there is none.
    pub fn pop(&self) -> Option<(f32, f32)> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let left = f32::from_bits(self.slots[tail * 2].load(Ordering::Relaxed));
        let right = f32::from_bits(self.slots[tail * 2 + 1].load(Ordering::Relaxed));
        self.tail
            .store(tail.wrapping_add(1) & self.capacity, Ordering::Release);
        Some((left, right))
    }
}

/// What the cue consumer is doing, for the journal and for the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CueStats {
    /// Frames waiting in the ring.
    pub fill: usize,
    /// How much faster (or slower) than the nominal ratio the consumer reads, as a share: 0.001 is a tenth of a percent.
    pub correction: f64,
    /// Frames the consumer had to hold because the ring was empty.
    pub starved: u64,
    /// Frames the producer could not hand over because the ring was full.
    pub dropped: u64,
}

/// What the cue link is doing, readable from another thread: the journal has to say whether the two clocks agreed.
#[derive(Debug, Default)]
pub struct CueTelemetry {
    fill: AtomicUsize,
    correction_millionths: AtomicU32,
    starved: AtomicU64,
    dropped: AtomicU64,
    produced: AtomicU64,
}

impl CueTelemetry {
    pub fn read(&self) -> CueStats {
        CueStats {
            fill: self.fill.load(Ordering::Relaxed),
            correction: f64::from(self.correction_millionths.load(Ordering::Relaxed)) / 1_000_000.0,
            starved: self.starved.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }

    pub fn produced(&self) -> u64 {
        self.produced.load(Ordering::Relaxed)
    }

    fn publish(&self, stats: CueStats, produced: u64) {
        self.fill.store(stats.fill, Ordering::Relaxed);
        // Millionths of a ratio: a correction is a few hundred per million, and an integer keeps this lock-free.
        let millionths = (stats.correction * 1_000_000.0).round();
        self.correction_millionths
            .store(millionths.clamp(-1.0, 1.0) as u32, Ordering::Relaxed);
        self.starved.store(stats.starved, Ordering::Relaxed);
        self.dropped.store(stats.dropped, Ordering::Relaxed);
        self.produced.store(produced, Ordering::Relaxed);
    }
}

/// Reads the ring at the cue device's rate, correcting for the difference between the two clocks.
pub struct CuePull {
    ring: Arc<CueRing>,
    /// Ratio of the producer's rate to the consumer's: how many source frames one output frame is worth.
    base_step: f64,
    correction: f64,
    /// Fractional read position between the two middle samples of the history.
    position: f64,
    /// Four frames of history per channel, so the interpolation has neighbours on both sides.
    history: [[f32; 4]; 2],
    /// Depth the controller steers towards, in frames.
    target: usize,
    starved: u64,
    /// Samples produced, for the journal.
    produced: u64,
    /// Where those numbers are published, for whoever asks from outside the callback.
    telemetry: Option<Arc<CueTelemetry>>,
}

impl CuePull {
    /// `source_rate` is the main device's rate, `sink_rate` the cue device's.
    pub fn new(ring: Arc<CueRing>, source_rate: u32, sink_rate: u32) -> Self {
        let base_step = f64::from(source_rate.max(1)) / f64::from(sink_rate.max(1));
        // Forty milliseconds of cushion: enough to ride out a scheduling hiccup on either side, short enough that the cue
        // stays in step with the booth's own monitoring.
        let target =
            ((f64::from(sink_rate.max(1)) * 0.040) as usize).clamp(64, ring.capacity() / 2);
        let mut pull = Self {
            ring,
            base_step,
            correction: 1.0,
            position: 0.0,
            history: [[0.0; 4]; 2],
            target,
            starved: 0,
            produced: 0,
            telemetry: None,
        };
        for _ in 0..4 {
            pull.shift_in();
        }
        pull
    }

    pub fn stats(&self) -> CueStats {
        CueStats {
            fill: self.ring.fill(),
            correction: self.correction - 1.0,
            starved: self.starved,
            dropped: self.ring.dropped(),
        }
    }

    pub fn produced(&self) -> u64 {
        self.produced
    }

    /// Publish the numbers somewhere another thread can read them.
    pub fn with_telemetry(mut self, telemetry: Arc<CueTelemetry>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    fn shift_in(&mut self) {
        let frame = self.ring.pop();
        if frame.is_none() {
            self.starved += 1;
        }
        for (channel, slot) in self.history.iter_mut().enumerate() {
            // A starved consumer holds the last sample instead of inventing a zero: a click is louder than a held note,
            // and the ring fills again as soon as the main callback runs.
            let next = match frame {
                Some(pair) => {
                    if channel == 0 {
                        pair.0
                    } else {
                        pair.1
                    }
                }
                None => slot[3],
            };
            slot[0] = slot[1];
            slot[1] = slot[2];
            slot[2] = slot[3];
            slot[3] = next;
        }
    }

    /// One more sample of the cue.
    pub fn next_frame(&mut self) -> (f32, f32) {
        let t = self.position;
        let left = hermite(&self.history[0], t);
        let right = hermite(&self.history[1], t);
        self.produced += 1;

        // The controller: the buffer is late or early by however many frames, and the read pointer leans against it.
        // A proportional term with a slow one-pole; an integrator here would fight the device's own clock recovery.
        let fill = self.ring.fill() as f64;
        let error = fill - self.target as f64;
        let wanted = (1.0 + error * 1.5e-5).clamp(0.98, 1.02);
        self.correction += (wanted - self.correction) * 1e-4;

        // Every sixty-fourth sample is enough for a journal that is read once a second, and keeps the atomics off the
        // hot path.
        if self.produced.is_multiple_of(64)
            && let Some(telemetry) = self.telemetry.as_ref()
        {
            telemetry.publish(self.stats(), self.produced);
        }

        self.position += self.base_step * self.correction;
        while self.position >= 1.0 {
            self.position -= 1.0;
            self.shift_in();
        }
        (left, right)
    }
}

/// Four-point Catmull-Rom: neighbours on both sides, one multiply chain, no state.
fn hermite(points: &[f32; 4], t: f64) -> f32 {
    let p0 = f64::from(points[0]);
    let p1 = f64::from(points[1]);
    let p2 = f64::from(points[2]);
    let p3 = f64::from(points[3]);
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    let value = ((a * t + b) * t + c) * t + p1;
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine the producer hands over, so the tests move real audio rather than a ramp.
    struct Source {
        phase: f64,
        step: f64,
    }

    impl Source {
        fn new(rate: u32, hz: f64) -> Self {
            Self {
                phase: 0.0,
                step: std::f64::consts::TAU * hz / f64::from(rate),
            }
        }

        fn next(&mut self) -> f32 {
            let value = (self.phase.sin() * 0.5) as f32;
            self.phase += self.step;
            value
        }
    }

    /// Run both clocks against each other for `seconds`, the consumer's clock off by `drift_ppm`.
    fn run(
        source_rate: u32,
        sink_rate: u32,
        drift_ppm: f64,
        seconds: f64,
    ) -> (CueStats, Vec<usize>) {
        let ring = CueRing::new(sink_rate as usize);
        let mut pull = CuePull::new(Arc::clone(&ring), source_rate, sink_rate);
        let mut source = Source::new(source_rate, 440.0);
        // The producer starts a cushion ahead, which is what a running engine looks like.
        for _ in 0..sink_rate as usize / 25 {
            let value = source.next();
            ring.push(value, value);
        }
        let mut fill = Vec::new();
        let mut time = 0.0f64;
        let micro = 1.0 / 1_000_000.0;
        let mut next_source = 0.0f64;
        let mut next_sink = 0.0f64;
        let slow = 1.0 + drift_ppm * 1e-6;
        while time < seconds {
            if time >= next_source {
                let value = source.next();
                ring.push(value, value);
                next_source += 1.0 / f64::from(source_rate);
            }
            if time >= next_sink {
                pull.next_frame();
                fill.push(pull.stats().fill);
                next_sink += 1.0 / (f64::from(sink_rate) * slow);
            }
            time += micro;
        }
        (pull.stats(), fill)
    }

    #[test]
    fn the_ring_keeps_what_is_put_in_it_in_order() {
        let ring = CueRing::new(8);
        assert!(ring.push(1.0, -1.0));
        assert!(ring.push(2.0, -2.0));
        assert_eq!(ring.fill(), 2);
        assert_eq!(ring.pop(), Some((1.0, -1.0)));
        assert_eq!(ring.pop(), Some((2.0, -2.0)));
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn a_full_ring_drops_and_says_so() {
        let ring = CueRing::new(4);
        let mut pushed = 0;
        while ring.push(0.5, 0.5) {
            pushed += 1;
            assert!(
                pushed <= ring.capacity(),
                "the ring accepted more than it holds"
            );
        }
        assert_eq!(pushed, ring.capacity());
        assert!(ring.dropped() >= 1, "a dropped frame has to be counted");
    }

    #[test]
    fn a_rate_difference_is_followed_without_starving() {
        // 48 kHz produced, 44.1 kHz consumed: 1.088 source frames per output frame, and no click after the warm-up.
        let (stats, fill) = run(48_000, 44_100, 0.0, 3.0);
        let late = &fill[fill.len() / 3..];
        let lowest = late.iter().copied().min().unwrap_or(0);
        let highest = late.iter().copied().max().unwrap_or(0);
        assert!(
            stats.starved <= 128,
            "the cue starved {} times",
            stats.starved
        );
        assert!(lowest > 0, "the cushion emptied: {lowest}");
        assert!(highest < 40_000, "the cushion ran away: {highest}");
        assert!(
            stats.correction.abs() < 0.02,
            "the correction left its band: {}",
            stats.correction
        );
    }

    #[test]
    fn a_drifting_clock_is_held_by_the_controller() {
        // 200 ppm is a bad pair of crystals, and a minute of it is 12 ms: without the controller the cushion walks away
        // and the cue clicks.
        let (stats, fill) = run(48_000, 48_000, 200.0, 6.0);
        let late = &fill[fill.len() / 3..];
        let lowest = late.iter().copied().min().unwrap_or(0);
        let highest = late.iter().copied().max().unwrap_or(0);
        assert!(
            stats.starved <= 128,
            "the cue starved {} times",
            stats.starved
        );
        assert!(lowest > 0, "the cushion emptied under drift: {lowest}");
        assert!(
            highest < 40_000,
            "the cushion ran away under drift: {highest}"
        );
    }

    #[test]
    fn a_fast_consumer_is_held_back_without_losing_the_cushion() {
        // The consumer's crystal is 300 ppm slow (it consumes fewer frames than the producer makes): without correction
        // the ring would fill up and the producer would start dropping.
        let (stats, fill) = run(44_100, 44_100, -300.0, 6.0);
        let late = &fill[fill.len() / 3..];
        let highest = late.iter().copied().max().unwrap_or(0);
        assert!(highest < 40_000, "the cushion grew to {highest}");
        assert!(
            stats.correction.abs() < 0.02,
            "the correction left its band: {}",
            stats.correction
        );
    }
}
