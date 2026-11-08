//! Signalsmith DSP behind a C ABI: a meter that rises to a peak and falls back at a steady rate.
//!
//! The library is vendored C++11 (`vendor/signalsmith-dsp`, MIT, © Geraint Luff / Signalsmith Audio
//! Ltd.). This crate is the only place that knows it exists: the rest of the tree talks to [`PeakDecay`],
//! which owns the handle and cannot be copied.

use std::ffi::c_void;

mod ffi {
    use std::ffi::c_void;

    unsafe extern "C" {
        pub fn dj_fft_new(size: i32) -> *mut c_void;
        pub fn dj_fft_free(handle: *mut c_void);
        pub fn dj_fft_magnitudes(handle: *mut c_void, input: *const f32, out: *mut f32);

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

        pub fn dj_resampler_new(mode: i32) -> *mut c_void;
        pub fn dj_resampler_free(handle: *mut c_void);
        pub fn dj_isolator_new(
            channels: i32,
            sample_rate: f64,
            low_hz: f64,
            high_hz: f64,
        ) -> *mut c_void;
        pub fn dj_isolator_free(handle: *mut c_void);
        pub fn dj_isolator_reset(handle: *mut c_void);
        pub fn dj_isolator_set_gains(handle: *mut c_void, low: f32, mid: f32, high: f32);
        pub fn dj_isolator_process(handle: *mut c_void, interleaved: *mut f32, frames: i32);
        pub fn dj_isolator_response_db(
            handle: *mut c_void,
            band: i32,
            hertz: f64,
            sample_rate: f64,
        ) -> f64;

        pub fn dj_resample(
            handle: *mut c_void,
            input: *const f32,
            in_frames: i32,
            output: *mut f32,
            out_frames: i32,
            base: i32,
            phase: f64,
            step: f64,
        ) -> f64;
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
/// A windowed real-input spectrum, for analysis that reads flux rather than level (B02).
///
/// Owns the plan and its scratch buffers: [`Spectrum::magnitudes`] allocates nothing, so it can run on
/// the analysis worker without a surprise in the middle of a measurement.
pub struct Spectrum {
    handle: *mut c_void,
    size: usize,
}

// The handle is a plan plus its buffers, and every call takes `&mut self`: sending it to another
// thread cannot make two calls share the scratch space.
unsafe impl Send for Spectrum {}

impl Spectrum {
    /// Build a plan for `size` samples per block; `None` when the size is too small to be a spectrum.
    pub fn new(size: usize) -> Option<Self> {
        if size < 16 {
            return None;
        }
        let handle = unsafe { ffi::dj_fft_new(i32::try_from(size).ok()?) };
        if handle.is_null() {
            return None;
        }
        Some(Self { handle, size })
    }

    /// Magnitudes of `size` samples, in the order of the bins; the buffer must be `size / 2` long.
    ///
    /// The block is windowed inside the call, so the caller hands over raw samples.
    pub fn magnitudes(&mut self, input: &[f32], out: &mut [f32]) {
        let wanted = self.size / 2;
        if input.len() < self.size || out.len() < wanted {
            return;
        }
        unsafe { ffi::dj_fft_magnitudes(self.handle, input.as_ptr(), out.as_mut_ptr()) }
    }
}

impl Drop for Spectrum {
    fn drop(&mut self) {
        unsafe { ffi::dj_fft_free(self.handle) };
    }
}

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

/// Which fractional-delay kernel the resampler uses.
///
/// A file at its own rate needs nothing; a 44.1 kHz file on a 48 kHz device, or a tempo the hand
/// asked for, is a resampling job, and the kernel decides how much of the signal folds back into the
/// audible band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    /// Two taps: what the deck did before, kept so the difference can be measured.
    Linear,
    /// Four taps: cheap and much cleaner than linear, good enough for a tempo move.
    Cubic,
    /// Sixteen taps of Kaiser-windowed sinc: the quality the rate conversion needs, at a cost.
    KaiserSinc16,
}

impl Interpolation {
    fn code(self) -> i32 {
        match self {
            Interpolation::Linear => 0,
            Interpolation::Cubic => 1,
            Interpolation::KaiserSinc16 => 2,
        }
    }

    /// Taps the kernel needs, and how many frames before the integer position.
    ///
    /// A caller has to keep that much history at the front of the buffer it interpolates from.
    pub fn geometry(self) -> (usize, usize) {
        match self {
            Interpolation::Linear => (2, 0),
            Interpolation::Cubic => (4, 1),
            Interpolation::KaiserSinc16 => (16, 7),
        }
    }
}

/// A fractional-delay resampler: interpolates an interleaved stereo buffer at any step.
///
/// The state is the kernel's (the windowed-sinc coefficients); the position lives with the caller,
/// who passes `phase` and gets the position reached back.
pub struct Resampler {
    handle: *mut c_void,
    interpolation: Interpolation,
}

impl Resampler {
    /// A resampler with the given kernel.
    ///
    /// # Panics
    /// Only if the library refuses a mode this crate itself defines.
    pub fn new(interpolation: Interpolation) -> Self {
        // SAFETY: the mode code is one of the crate's own values, and the handle is checked below.
        let handle = unsafe { ffi::dj_resampler_new(interpolation.code()) };
        assert!(
            !handle.is_null(),
            "the DSP library refused {interpolation:?}"
        );
        Self {
            handle,
            interpolation,
        }
    }

    /// Which kernel this resampler uses.
    pub fn interpolation(&self) -> Interpolation {
        self.interpolation
    }

    /// Interpolate `out` from `input`, starting `phase` frames past frame `base`.
    ///
    /// Every output frame sits `step` frames further than the one before. Returns the position
    /// reached, in frames from `base`: the caller keeps the fractional part as its next phase and
    /// discards the whole frames it has consumed. Frames the kernel cannot reach — the tail of the
    /// buffer, or material before `base` — come back as silence rather than a guess.
    pub fn process(
        &mut self,
        input: &[f32],
        out: &mut [f32],
        base: usize,
        phase: f64,
        step: f64,
    ) -> f64 {
        let in_frames = i32::try_from(input.len() / 2).unwrap_or(0);
        let out_frames = i32::try_from(out.len() / 2).unwrap_or(0);
        let base = i32::try_from(base).unwrap_or(i32::MAX);
        if in_frames <= 0 || out_frames <= 0 {
            out.fill(0.0);
            return phase;
        }
        // SAFETY: both slices hold at least `frames * 2` samples, `input` is only read, the handle is
        // live, and every other argument is a plain value.
        unsafe {
            ffi::dj_resample(
                self.handle,
                input.as_ptr(),
                in_frames,
                out.as_mut_ptr(),
                out_frames,
                base,
                phase,
                step,
            )
        }
    }
}

// SAFETY: the handle owns the kernel and its coefficients, and every method takes `&mut self`, so it
// moves between threads without being shared.
unsafe impl Send for Resampler {}

impl Drop for Resampler {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `dj_resampler_new` and is freed exactly once, here.
        unsafe { ffi::dj_resampler_free(self.handle) };
    }
}

impl std::fmt::Debug for Resampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resampler")
            .field("interpolation", &self.interpolation)
            .finish_non_exhaustive()
    }
}

/// A three-way crossover with a linear gain on each band.
///
/// The splits are Linkwitz-Riley fourth order, built from the library's biquads: a lowpass and a
/// highpass of that family sum to one in phase, so the three bands add up to the dry signal exactly.
/// A gain of `1.0` leaves its band alone, `0.0` removes it, above one boosts it, and with every gain
/// at one the signal comes back bit for bit.
pub struct Isolator {
    handle: *mut c_void,
}

impl Isolator {
    /// An isolator for stereo `sample_rate`, splitting at `low_hz` and `high_hz`.
    ///
    /// # Panics
    /// When the crossover points are not a pair of positive, ordered frequencies.
    pub fn new(sample_rate: f64, low_hz: f64, high_hz: f64) -> Self {
        assert!(
            low_hz > 0.0 && high_hz > low_hz,
            "an isolator needs an ordered pair of crossover points, got {low_hz} and {high_hz}"
        );
        // SAFETY: every argument is a plain value, and the handle is checked below.
        let handle = unsafe { ffi::dj_isolator_new(2, sample_rate, low_hz, high_hz) };
        assert!(!handle.is_null(), "the DSP library refused an isolator");
        Self { handle }
    }

    /// Linear gain of each band: `(low, mid, high)`. `1.0` is unity, `0.0` removes the band.
    pub fn set_gains(&mut self, low: f32, mid: f32, high: f32) {
        // SAFETY: `handle` came from `dj_isolator_new` and lives until `Drop`.
        unsafe { ffi::dj_isolator_set_gains(self.handle, low, mid, high) };
    }

    /// Filter one interleaved stereo block in place.
    pub fn process(&mut self, interleaved: &mut [f32]) {
        let frames = i32::try_from(interleaved.len() / 2).unwrap_or(0);
        if frames <= 0 {
            return;
        }
        // SAFETY: the slice holds at least `frames * 2` samples, which is what the wrapper reads and
        // writes, and the handle is live.
        unsafe { ffi::dj_isolator_process(self.handle, interleaved.as_mut_ptr(), frames) };
    }

    /// Response of one band (`0`, `1`, `2`) or of the whole split (`3`) at `hertz`, in decibels.
    ///
    /// Read from the sections' own coefficients, so the curve a strip draws is the response the filter
    /// has, not the one a formula predicts.
    pub fn response_db(&self, band: i32, hertz: f64, sample_rate: f64) -> f64 {
        // SAFETY: the handle is live for the whole call and the function only reads the coefficients.
        unsafe { ffi::dj_isolator_response_db(self.handle, band, hertz, sample_rate) }
    }

    /// Drop the filter history, for a deck that starts a new track.
    pub fn reset(&mut self) {
        // SAFETY: `handle` is live; the call only clears the filters' own state.
        unsafe { ffi::dj_isolator_reset(self.handle) };
    }
}

// SAFETY: the handle owns the filters, and every method takes `&mut self`, so it moves between threads
// without being shared.
unsafe impl Send for Isolator {}

impl Drop for Isolator {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `dj_isolator_new` and is freed exactly once, here.
        unsafe { ffi::dj_isolator_free(self.handle) };
    }
}

impl std::fmt::Debug for Isolator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Isolator").finish_non_exhaustive()
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
    fn a_kernel_that_folds_less_keeps_the_image_out_of_the_band() {
        // A 15 kHz tone read at 0.919 (44.1 kHz into a 48 kHz device). Linear interpolation leaves an
        // image at 44100 - 15000 = 29.1 kHz, which folds to 18.9 kHz. A windowed-sinc kernel does not.
        let step = 44_100.0 / 48_000.0;
        let frames = 4096;
        let mut input = vec![0.0f32; frames * 2];
        for frame in 0..frames {
            let value =
                (std::f64::consts::TAU * 15_000.0 * frame as f64 / 44_100.0).sin() as f32 * 0.5;
            input[frame * 2] = value;
            input[frame * 2 + 1] = value;
        }
        let mut energy_at_image = Vec::new();
        for interpolation in [
            Interpolation::Linear,
            Interpolation::Cubic,
            Interpolation::KaiserSinc16,
        ] {
            let (_, lookback) = interpolation.geometry();
            let mut resampler = Resampler::new(interpolation);
            let mut out = vec![0.0f32; 2048 * 2];
            let reached = resampler.process(&input, &mut out, lookback + 16, 0.0, step);
            assert!(reached > 0.0, "{interpolation:?} produced nothing");
            energy_at_image.push(share_at(&out, 18_900.0));
        }
        assert!(
            energy_at_image[0] > 0.01,
            "the linear kernel is the reason for this test: {:?}",
            energy_at_image
        );
        assert!(
            energy_at_image[2] < energy_at_image[0] / 100.0,
            "the sinc kernel must fold at least 20 dB less: {:?}",
            energy_at_image
        );
        assert!(
            energy_at_image[1] < energy_at_image[0],
            "cubic must be better than linear: {:?}",
            energy_at_image
        );
    }

    /// Share of the energy that sits at `hertz`, measured at 48 kHz.
    fn share_at(block: &[f32], hertz: f64) -> f64 {
        let frames = block.len() / 2;
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        let mut energy = 0.0f64;
        for index in 0..frames {
            let value = f64::from(block[index * 2]);
            let phase = std::f64::consts::TAU * hertz * index as f64 / 48_000.0;
            re += value * phase.cos();
            im += value * phase.sin();
            energy += value * value;
        }
        if energy <= 0.0 {
            return 0.0;
        }
        2.0 * (re * re + im * im) / frames as f64 / energy
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
