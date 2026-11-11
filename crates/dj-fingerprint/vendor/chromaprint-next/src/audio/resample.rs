//! A polyphase Kaiser-windowed sinc resampler, written for this tree.
//!
//! Chromaprint resamples every input to 11025 Hz mono before it looks at the spectrum, and the reference
//! library borrows FFmpeg's `av_resample` for it — which is LGPL. This file replaces that one module so
//! the rest of the crate, which is MIT, can be used whole: the algorithm (windowed-sinc interpolation
//! with a polyphase kernel) is textbook, and none of FFmpeg's code is reproduced here.
//!
//! The interface is the one `audio::processor` calls: build the context once per rate pair, then pull
//! output samples, telling the caller how many input frames it may drop.

/// Zero-th order modified Bessel function of the first kind, by its power series.
///
/// The Kaiser window needs it; the series converges in a few dozen terms for the beta we use and needs
/// no table.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0_f64;
    let mut term = 1.0_f64;
    let half = x * x / 4.0;
    for k in 1..64 {
        term *= half / (k as f64 * k as f64);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// `sin(pi x) / (pi x)`, with the value at zero.
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let pi_x = std::f64::consts::PI * x;
        pi_x.sin() / pi_x
    }
}

/// One polyphase kernel bank: `phases` rows of `taps` weights each.
struct Bank {
    phases: usize,
    taps: usize,
    weights: Vec<f32>,
}

impl Bank {
    /// Build the bank for a rate pair.
    ///
    /// `cutoff` is the fraction of the *input* Nyquist that is kept. Downsampling by a factor `f` needs a
    /// kernel `f` times longer to keep the same number of lobes, which is why the tap count grows as the
    /// ratio falls: filtering 48 kHz to 11 kHz is exactly the case where a short kernel would alias.
    fn build(ratio: f64, filter_size: f64, phase_shift: i32, cutoff: f64) -> Self {
        let fc = (cutoff.clamp(0.0, 1.0) * ratio.min(1.0)).max(1e-6);
        let half = ((filter_size / (2.0 * fc)).ceil() as usize).max(1);
        let taps = 2 * half;
        let phases = 1_usize << phase_shift.max(0);
        let beta = 9.0_f64;
        let norm = bessel_i0(beta);
        let mut weights = vec![0.0_f32; phases * taps];
        for phase in 0..phases {
            let frac = phase as f64 / phases as f64;
            let mut row = 0.0_f64;
            for tap in 0..taps {
                // Distance from the output position to this input frame, in input frames.
                let offset = (tap as f64 - half as f64 + 1.0) - frac;
                let window = if offset.abs() >= half as f64 {
                    0.0
                } else {
                    let t = offset / half as f64;
                    bessel_i0(beta * (1.0 - t * t).max(0.0).sqrt()) / norm
                };
                let weight = fc * sinc(fc * offset) * window;
                weights[phase * taps + tap] = weight as f32;
                row += weight;
            }
            // Unity gain per phase: without it the output level follows the kernel length and the
            // fingerprint sees a recording that is quieter than it is.
            if row.abs() > 1e-12 {
                let scale = 1.0 / row;
                for tap in 0..taps {
                    weights[phase * taps + tap] =
                        (f64::from(weights[phase * taps + tap]) * scale) as f32;
                }
            }
        }
        Self {
            phases,
            taps,
            weights,
        }
    }
}

/// Resamples i16 mono audio from one rate to another, one buffer at a time.
pub struct AvResampleContext {
    bank: Bank,
    /// Input frames per output frame.
    step: f64,
    /// Where the next output sits, in input frames from the start of the buffer handed in.
    pos: f64,
    half: usize,
}

impl AvResampleContext {
    /// Build a context for one rate pair.
    ///
    /// `filter_size` is the kernel length in frames at unity ratio, `phase_shift` the log2 of the number
    /// of fractional positions the kernel is tabulated at. `linear` is part of the interface this
    /// replaces and is ignored: the kernel is a windowed sinc either way.
    pub fn new(
        out_rate: i32,
        in_rate: i32,
        filter_size: i32,
        phase_shift: i32,
        _linear: bool,
        cutoff: f64,
    ) -> Self {
        let out_rate = f64::from(out_rate.max(1));
        let in_rate = f64::from(in_rate.max(1));
        let ratio = out_rate / in_rate;
        let bank = Bank::build(ratio, f64::from(filter_size.max(2)), phase_shift, cutoff);
        Self {
            half: bank.taps / 2,
            bank,
            step: in_rate / out_rate,
            pos: 0.0,
        }
    }

    /// Pull up to `dst_size` output samples out of `src`.
    ///
    /// Returns how many were written and sets `consumed` to the number of input frames the caller may
    /// drop from the front of `src`: the frames the next output still needs stay in the buffer, and the
    /// fractional position is carried into the next call. A frame before the start of the buffer is
    /// silence, which is what the first call of a stream needs.
    pub fn resample(
        &mut self,
        dst: &mut [i16],
        src: &[i16],
        consumed: &mut usize,
        dst_size: usize,
    ) -> usize {
        let half = self.half as isize;
        let taps = self.bank.taps;
        let len = src.len() as isize;
        let mut written = 0_usize;
        let limit = dst_size.min(dst.len());
        while written < limit {
            let position = self.pos;
            let base = position.floor();
            if base + half as f64 >= len as f64 {
                break;
            }
            let frac = position - base;
            let phase = ((frac * self.bank.phases as f64) as usize).min(self.bank.phases - 1);
            let kernel = &self.bank.weights[phase * taps..phase * taps + taps];
            let start = base as isize - half + 1;
            let mut sum = 0.0_f32;
            for (tap, weight) in kernel.iter().enumerate() {
                let index = start + tap as isize;
                if index < 0 || index >= len {
                    continue;
                }
                sum += f32::from(src[index as usize]) * weight;
            }
            let sample = sum.round();
            dst[written] = if sample >= 32767.0 {
                32767
            } else if sample <= -32768.0 {
                -32768
            } else {
                sample as i16
            };
            written += 1;
            self.pos += self.step;
        }
        // Everything before the first frame the next output needs can go.
        let keep_from = (self.pos.floor() as isize - half + 1).max(0);
        let keep_from = keep_from.min(len);
        *consumed = keep_from as usize;
        self.pos -= keep_from as f64;
        written
    }
}
