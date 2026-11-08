//! Key lock: changing how fast a track plays without changing its pitch.
//!
//! This crate is a thin, safe wrapper over [Signalsmith Stretch] — a header-only MIT library that
//! does the actual work (a spectral phase-vocoder with per-band peaks and correlation, which keeps
//! transients and tonality where a single-band time-domain overlap-add smears them). The vendored
//! copy and its licence live in `vendor/`.
//!
//! The deck speaks interleaved stereo; the library speaks per-channel pointers, so the buffers are
//! converted here. Nothing in [`Stretch::process`] allocates once the scratch buffers have grown to
//! the sizes the caller uses, which is what lets the deck call it from the audio callback.
//!
//! [Signalsmith Stretch]: https://signalsmith-audio.co.uk/code/stretch/
// The C ABI in this crate cannot be declared without `unsafe`; the workspace denies it everywhere
// else. Every block carries a SAFETY comment.
#![allow(unsafe_code)]

use std::ffi::c_void;

mod ffi {
    use std::ffi::c_void;

    unsafe extern "C" {
        pub fn dj_stretch_new(channels: i32, sample_rate: f64) -> *mut c_void;
        pub fn dj_stretch_new_custom(
            channels: i32,
            sample_rate: f64,
            block_seconds: f64,
            interval_seconds: f64,
        ) -> *mut c_void;
        pub fn dj_stretch_free(handle: *mut c_void);
        pub fn dj_stretch_reset(handle: *mut c_void);
        pub fn dj_stretch_latency(handle: *mut c_void) -> i32;
        pub fn dj_stretch_block(handle: *mut c_void) -> i32;
        pub fn dj_stretch_process(
            handle: *mut c_void,
            input: *const f32,
            input_frames: i32,
            output: *mut f32,
            output_frames: i32,
        );
        pub fn dj_stretch_seek(
            handle: *mut c_void,
            input: *const f32,
            input_frames: i32,
            playback_rate: f64,
        );
    }
}

/// Stereo interleaved time-stretcher.
pub struct Stretch {
    handle: *mut c_void,
    latency: usize,
    block: usize,
}

// SAFETY: the handle owns nothing shared — it is a C++ object with its own buffers, and every method
// takes `&mut self`, so no two threads can drive it at once.
unsafe impl Send for Stretch {}

impl Stretch {
    /// A stretcher for `sample_rate`, with the library's default block: the best sound, the largest
    /// delay.
    pub fn new(sample_rate: f64) -> Self {
        // SAFETY: the constructor validates its arguments and returns null only for an unsupported
        // channel count, which never happens here (two channels).
        let handle = unsafe { ffi::dj_stretch_new(2, sample_rate) };
        assert!(
            !handle.is_null(),
            "signalsmith stretch refused two channels"
        );
        // SAFETY: `handle` is a live stretcher created above.
        let latency = unsafe { ffi::dj_stretch_latency(handle) }.max(0) as usize;
        // SAFETY: same handle.
        let block = unsafe { ffi::dj_stretch_block(handle) }.max(1) as usize;
        Self {
            handle,
            latency,
            block,
        }
    }

    /// A stretcher with a block and an overlap interval of the caller's choosing, in seconds.
    ///
    /// The deck uses this to buy alignment: the library's default latency is heard as a lag and puts
    /// a loop marker ahead of the sound. Purity at 1.08 measures 0,9998 for the default block and
    /// 0,9996 for the short one, so the shorter block costs almost nothing.
    pub fn with_config(sample_rate: f64, block_seconds: f64, interval_seconds: f64) -> Self {
        // SAFETY: same contract as `new`; the sizes are clamped by the library to sane values.
        let handle =
            unsafe { ffi::dj_stretch_new_custom(2, sample_rate, block_seconds, interval_seconds) };
        assert!(
            !handle.is_null(),
            "signalsmith stretch refused two channels"
        );
        // SAFETY: `handle` is live.
        let latency = unsafe { ffi::dj_stretch_latency(handle) }.max(0) as usize;
        // SAFETY: same handle.
        let block = unsafe { ffi::dj_stretch_block(handle) }.max(1) as usize;
        Self {
            handle,
            latency,
            block,
        }
    }

    /// Frames the library delays its output by: the deck's clock subtracts this, the deck's priming
    /// feeds it before the first sample is heard.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// The library's own analysis block, in frames. Exposed for tests and for the deck's buffer sizes.
    pub fn block(&self) -> usize {
        self.block
    }

    /// Forget the history: the playhead jumped, and material from before it is not part of the sound.
    pub fn reset(&mut self) {
        // SAFETY: `handle` lives as long as `self`.
        unsafe { ffi::dj_stretch_reset(self.handle) };
    }

    /// Play `input` (interleaved stereo) into `out`, which may have a different frame count.
    ///
    /// The ratio is not passed: the library infers it from the two lengths and keeps the remainder
    /// between calls, so a caller that keeps the average right gets a steady tempo.
    pub fn process(&mut self, input: &[f32], out: &mut [f32]) {
        let in_frames = i32::try_from(input.len() / 2).unwrap_or(0);
        let out_frames = i32::try_from(out.len() / 2).unwrap_or(0);
        if in_frames == 0 || out_frames == 0 {
            out.fill(0.0);
            return;
        }
        // SAFETY: both slices are at least `frames * 2` longs, their lengths are what is passed, and
        // the handle is live. The library writes exactly `out_frames` frames.
        unsafe {
            ffi::dj_stretch_process(
                self.handle,
                input.as_ptr(),
                in_frames,
                out.as_mut_ptr(),
                out_frames,
            );
        }
    }

    /// Tell the library the material before this point belongs to the old position, and hand it the
    /// pre-roll it needs to keep the output continuous across a jump.
    pub fn seek(&mut self, input: &[f32], playback_rate: f64) {
        let frames = i32::try_from(input.len() / 2).unwrap_or(0);
        if frames <= 0 {
            return;
        }
        // SAFETY: `input` holds at least `frames * 2` samples and the handle is live.
        unsafe { ffi::dj_stretch_seek(self.handle, input.as_ptr(), frames, playback_rate) };
    }
}

impl Drop for Stretch {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `dj_stretch_new` and is freed exactly once, here.
        unsafe { ffi::dj_stretch_free(self.handle) };
    }
}

impl std::fmt::Debug for Stretch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stretch")
            .field("latency", &self.latency)
            .field("block", &self.block)
            .finish()
    }
}
