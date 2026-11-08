//! Signalsmith DSP behind a C ABI: a meter that rises to a peak and falls back at a steady rate.
//!
//! The library is vendored C++11 (`vendor/signalsmith-dsp`, MIT, © Geraint Luff / Signalsmith Audio
//! Ltd.). This crate is the only place that knows it exists: the rest of the tree talks to [`PeakDecay`],
//! which owns the handle and cannot be copied.

use std::ffi::c_void;

mod ffi {
    use std::ffi::c_void;

    unsafe extern "C" {
        pub fn dj_peak_decay_new(length: i32) -> *mut c_void;
        pub fn dj_peak_decay_free(handle: *mut c_void);
        pub fn dj_peak_decay_reset(handle: *mut c_void);
        pub fn dj_peak_decay_block(handle: *mut c_void, peak: f32, frames: i32) -> f32;

        pub fn dj_limiter_new(
            channels: i32,
            max_block: i32,
            sample_rate: f64,
            threshold: f32,
            attack_ms: f32,
            release_ms: f32,
        ) -> *mut c_void;
        pub fn dj_limiter_free(handle: *mut c_void);
        pub fn dj_limiter_reset(handle: *mut c_void);
        pub fn dj_limiter_latency(handle: *mut c_void) -> i32;
        pub fn dj_limiter_process(handle: *mut c_void, interleaved: *mut f32, frames: i32);
    }
}

/// How long a meter takes to fall from a full-scale peak to silence.
///
/// Half a second is the bar's own speed on hardware a DJ knows: slow enough to read at a glance, fast
/// enough that the next hit is not hidden behind the last one.
pub const DEFAULT_FALL_SECONDS: f32 = 0.5;

/// A peak meter with a linear fall back to the signal.
///
/// Feed it the peak of every block; it returns what the bar should show, which stays above the signal
/// and slides down over [`DEFAULT_FALL_SECONDS`] once the signal stops.
pub struct PeakDecay {
    handle: *mut c_void,
}

impl PeakDecay {
    /// A meter for `sample_rate`, falling over `fall_seconds`.
    ///
    /// # Panics
    /// When the length is not positive, which can only come from a nonsense rate.
    pub fn new(sample_rate: u32, fall_seconds: f32) -> Self {
        let frames = (sample_rate as f32 * fall_seconds).round();
        assert!(frames >= 1.0, "a meter needs a positive fall in frames");
        let length = frames as i32;
        // SAFETY: `length` is positive (checked above), so the library allocates a real filter.
        let handle = unsafe { ffi::dj_peak_decay_new(length) };
        assert!(
            !handle.is_null(),
            "the DSP library refused a {length}-frame meter"
        );
        Self { handle }
    }

    /// Advance the meter by one block and return the value to show.
    ///
    /// `peak` is the highest absolute sample of the block; `frames` is how long the block lasts, so a
    /// silent stream still moves the bar down at the same speed in time.
    pub fn block(&mut self, peak: f32, frames: usize) -> f32 {
        let frames = i32::try_from(frames).unwrap_or(i32::MAX);
        // SAFETY: `handle` came from `dj_peak_decay_new` and is freed only in `Drop`; the slice is not
        // shared with the library and every argument is a plain value.
        unsafe { ffi::dj_peak_decay_block(self.handle, peak.max(0.0), frames) }
    }

    /// Forget the history: the meter drops to zero at once, as after a stop or a track change.
    pub fn reset(&mut self) {
        // SAFETY: `handle` is live; the call only clears the filter's own state.
        unsafe { ffi::dj_peak_decay_reset(self.handle) };
    }
}

// SAFETY: the handle owns a plain value filter with no thread-affine state, and the mixer keeps one
// instance per channel inside the render thread. Moving it between threads (a mixer being handed to
// another callback owner) is sound; sharing it is prevented by `&mut self` on every method.
unsafe impl Send for PeakDecay {}

impl Drop for PeakDecay {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `dj_peak_decay_new` and is freed exactly once, here.
        unsafe { ffi::dj_peak_decay_free(self.handle) };
    }
}

impl std::fmt::Debug for PeakDecay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeakDecay").finish_non_exhaustive()
    }
}

/// How much of the peak the limiter sees before it arrives, in milliseconds.
///
/// The attack *is* the lookahead: the gain is already down when the peak reaches the output. Five
/// milliseconds is enough to catch a transient and short enough that the master still feels immediate.
pub const LIMITER_ATTACK_MS: f32 = 5.0;

/// How long the gain takes to come back after a peak, in milliseconds.
pub const LIMITER_RELEASE_MS: f32 = 150.0;

/// A lookahead limiter on an interleaved block, from `signalsmith-basics/limiter.h`.
///
/// Two channels share one gain: an unlinked limiter moves the stereo image whenever one side peaks.
/// The signal comes out delayed by [`LookaheadLimiter::latency_frames`] — that delay is what buys the
/// lookahead, and it is the caller's job to account for it.
pub struct LookaheadLimiter {
    handle: *mut c_void,
}

impl LookaheadLimiter {
    /// A limiter for `channels` at `sample_rate`, holding the output at `threshold`.
    ///
    /// # Panics
    /// When the geometry is empty, which can only come from a nonsense configuration.
    pub fn new(channels: usize, max_block_frames: usize, sample_rate: u32, threshold: f32) -> Self {
        let channels = channels.max(1);
        let max_block_frames = max_block_frames.max(1);
        let (Ok(channels_arg), Ok(block_arg)) =
            (i32::try_from(channels), i32::try_from(max_block_frames))
        else {
            panic!(
                "a limiter cannot be built for {channels} channels of {max_block_frames} frames"
            );
        };
        // SAFETY: every argument is a plain value; the library copies what it needs and returns a live
        // handle (or null, which is checked).
        let handle = unsafe {
            ffi::dj_limiter_new(
                channels_arg,
                block_arg,
                f64::from(sample_rate.max(1)),
                threshold,
                LIMITER_ATTACK_MS,
                LIMITER_RELEASE_MS,
            )
        };
        assert!(!handle.is_null(), "the DSP library refused a limiter");
        Self { handle }
    }

    /// Frames the signal is delayed by: the attack time, which is the lookahead.
    pub fn latency_frames(&self) -> u32 {
        // SAFETY: `handle` came from `dj_limiter_new` and lives until `Drop`.
        let latency = unsafe { ffi::dj_limiter_latency(self.handle) };
        u32::try_from(latency.max(0)).unwrap_or(0)
    }

    /// Limit one interleaved block in place.
    pub fn process(&mut self, interleaved: &mut [f32]) {
        let frames = i32::try_from(interleaved.len() / 2).unwrap_or(0);
        if frames <= 0 {
            return;
        }
        // SAFETY: the slice holds at least `frames * 2` samples, which is what the wrapper reads and
        // writes, and the handle is live.
        unsafe { ffi::dj_limiter_process(self.handle, interleaved.as_mut_ptr(), frames) };
    }

    /// Forget the history: the delay line empties, as after a stop or a device change.
    pub fn reset(&mut self) {
        // SAFETY: `handle` is live; the call only clears the effect's own state.
        unsafe { ffi::dj_limiter_reset(self.handle) };
    }
}

// SAFETY: the handle owns the effect and its scratch buffers, and every method takes `&mut self`, so
// it moves between threads without being shared.
unsafe impl Send for LookaheadLimiter {}

impl Drop for LookaheadLimiter {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `dj_limiter_new` and is freed exactly once, here.
        unsafe { ffi::dj_limiter_free(self.handle) };
    }
}

impl std::fmt::Debug for LookaheadLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LookaheadLimiter")
            .field("latency_frames", &self.latency_frames())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_of_peak_shows_the_peak() {
        let mut meter = PeakDecay::new(48_000, DEFAULT_FALL_SECONDS);
        let shown = meter.block(0.5, 128);
        assert!(
            (shown - 0.5).abs() < 1e-6,
            "the bar must reach the peak: {shown}"
        );
    }

    #[test]
    fn silence_falls_back_over_the_fall_time() {
        let mut meter = PeakDecay::new(48_000, DEFAULT_FALL_SECONDS);
        let _ = meter.block(1.0, 128);
        // Half of the half-second fall, in blocks of 128 frames: 12 000 frames.
        let blocks = (48_000 / 4) / 128;
        let mut shown = 1.0;
        for _ in 0..blocks {
            shown = meter.block(0.0, 128);
        }
        assert!(
            (0.35..0.65).contains(&shown),
            "half the fall time should be about half the peak, got {shown}"
        );
    }

    #[test]
    fn the_limiter_delays_the_signal_and_holds_the_peak() {
        let mut limiter = LookaheadLimiter::new(2, 256, 48_000, 0.999);
        assert!(
            limiter.latency_frames() > 0,
            "a lookahead limiter has a delay"
        );
        let mut block = vec![0.0f32; 256 * 2];
        // A quiet opening and then a peak well above full scale.
        for frame in 0..128 {
            block[frame * 2] = 0.25;
            block[frame * 2 + 1] = 0.25;
        }
        for frame in 128..256 {
            block[frame * 2] = 1.5;
            block[frame * 2 + 1] = 1.5;
        }
        let before = block.clone();
        limiter.process(&mut block);
        let peak = block
            .iter()
            .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
        assert!(peak <= 1.0, "the limiter let {peak} out");
        // The first frames are the delay line, not the input: that is what the lookahead costs.
        let latency = limiter.latency_frames() as usize;
        assert!(
            block[..latency * 2]
                .iter()
                .all(|sample| sample.abs() < 1e-6),
            "the delay line must start empty"
        );
        assert!(
            block[latency * 2..latency * 2 + 8]
                .iter()
                .any(|sample| sample.abs() > 1e-6),
            "the material must arrive after the delay"
        );
        assert_ne!(block, before, "the peak must be turned down");
    }

    #[test]
    fn a_louder_block_raises_the_bar_again() {
        let mut meter = PeakDecay::new(48_000, DEFAULT_FALL_SECONDS);
        let _ = meter.block(0.2, 128);
        let _ = meter.block(0.0, 128);
        let shown = meter.block(0.9, 128);
        assert!(
            (shown - 0.9).abs() < 1e-6,
            "a new peak must show at once: {shown}"
        );
    }
}
