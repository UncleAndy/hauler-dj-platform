//! Key lock: the tempo moves, the pitch does not (DJP-DECK-008).
//!
//! The work is done by the vendored [Signalsmith Stretch] library (MIT, `crates/dj-stretch-sys`): a
//! spectral phase-vocoder with per-band peaks, which keeps transients and tonality intact where a
//! single-band time-domain overlap-add smears them into a roughness the ear hears as "жужжание".
//!
//! This module is the policy: which rate the library runs at, how it is reset, and how much delay it
//! owes the deck. The library runs at the **device** rate, because the deck resamples the file to the
//! device rate first — the stretch then only has to answer one question (speed), and its buffers are
//! built once at deck construction instead of on every load.
//!
//! [Signalsmith Stretch]: https://signalsmith-audio.co.uk/code/stretch/

use dj_stretch_sys::Stretch;

/// Block length the library analyses, in seconds.
///
/// The library's default (0,12 s) delays its output by 60 ms: that is a lag on every press and a loop
/// marker running ahead of the sound. 60 ms of block halves the delay to 30 ms and costs 0,0002 of
/// tone purity at 1,08 — measured in `dj-stretch-sys/tests/stretch.rs`.
const BLOCK_SECONDS: f64 = 0.06;
/// Overlap interval, in seconds.
const INTERVAL_SECONDS: f64 = 0.015;

/// The deck's key-lock stage.
pub(crate) struct Stretcher {
    stretch: Stretch,
}

impl Stretcher {
    /// A stretcher for material already at `sample_rate`.
    pub(crate) fn new(sample_rate: f64) -> Self {
        Self {
            stretch: Stretch::with_config(sample_rate, BLOCK_SECONDS, INTERVAL_SECONDS),
        }
    }

    /// Frames the library holds before its output appears.
    ///
    /// The deck subtracts this from its playhead, so the clock, the waveform marker and the loop
    /// region all describe what is heard rather than what has been read.
    pub(crate) fn latency(&self) -> usize {
        self.stretch.latency()
    }

    /// Forget the history: the playhead jumped, and the material before it is not part of the sound.
    pub(crate) fn reset(&mut self) {
        self.stretch.reset();
    }

    /// Stretch `input` (device-rate, interleaved) into `out`, whose frame count sets the tempo.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut [f32]) {
        self.stretch.process(input, out);
    }
}

impl std::fmt::Debug for Stretcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stretcher")
            .field("latency", &self.stretch.latency())
            .finish()
    }
}
