//! Block-timing measurement for the render loop.
//!
//! This measures how long `Engine::process` itself takes per block — the quantity
//! DJP-NFR-003 talks about, as opposed to the interval between callbacks, which the device
//! drives. It runs offline on whatever host calls it: useful from the first day, but it is
//! **not** the reference-rig measurement (that needs a 2-hour live run, A11).

use std::time::Instant;

use crate::engine::Engine;

/// Per-block processing times of one offline render.
#[derive(Debug, Clone, Default)]
pub struct BlockTimings {
    /// Duration of each `process` call, in microseconds, in call order.
    pub per_block_us: Vec<f64>,
    /// Block size that was driven through the callback.
    pub block_frames: usize,
    /// Device sample rate of the render.
    pub sample_rate: u32,
}

/// Percentiles of a [`BlockTimings`] run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingSummary {
    /// Number of measured blocks.
    pub blocks: usize,
    /// Median block duration, microseconds.
    pub p50_us: f64,
    /// 99th percentile block duration, microseconds.
    pub p99_us: f64,
    /// Slowest block, microseconds.
    pub max_us: f64,
    /// Budget one block has at this rate and size, microseconds.
    pub period_us: f64,
    /// `p99_us` as a fraction of `period_us`.
    pub p99_of_period: f64,
    /// `max_us` as a fraction of `period_us`.
    pub max_of_period: f64,
}

impl BlockTimings {
    /// Percentile summary, or `None` when nothing was measured.
    pub fn summary(&self) -> Option<TimingSummary> {
        if self.per_block_us.is_empty() || self.sample_rate == 0 || self.block_frames == 0 {
            return None;
        }
        let mut sorted = self.per_block_us.clone();
        sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let at = |fraction: f64| -> f64 {
            let last = sorted.len().saturating_sub(1);
            let index = ((last as f64) * fraction).round() as usize;
            sorted[index.min(last)]
        };
        let block_frames = self.block_frames as f64;
        let period_us = block_frames / f64::from(self.sample_rate) * 1_000_000.0;
        Some(TimingSummary {
            blocks: sorted.len(),
            p50_us: at(0.5),
            p99_us: at(0.99),
            max_us: sorted.last().copied().unwrap_or(0.0),
            period_us,
            p99_of_period: at(0.99) / period_us,
            max_of_period: sorted.last().copied().unwrap_or(0.0) / period_us,
        })
    }
}

/// Render offline while timing every `process` call.
///
/// Allocation of the output buffer happens before the first timed call, so the
/// measurement covers the callback only.
pub fn render_measured(
    engine: &mut Engine,
    frames: usize,
    block_frames: usize,
) -> (Vec<f32>, BlockTimings) {
    let block = block_frames.max(1);
    let mut timings = BlockTimings {
        per_block_us: Vec::with_capacity(frames / block + 1),
        block_frames: block,
        sample_rate: engine.sample_rate(),
    };
    let mut out = vec![0.0f32; frames];
    let mut written = 0usize;
    while written < frames {
        let end = (written + block).min(frames);
        let (head, _) = out.split_at_mut(end);
        let started = Instant::now();
        engine.process(&mut head[written..end]);
        timings
            .per_block_us
            .push(started.elapsed().as_secs_f64() * 1_000_000.0);
        written = end;
    }
    (out, timings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, scheduled};
    use dj_domain::{DeckId, DjCommand};

    #[test]
    fn every_block_is_timed_and_the_summary_is_sane() {
        let mut engine = Engine::new(48_000, 440.0);
        engine.submit(scheduled(
            1,
            0,
            DjCommand::Play {
                deck: DeckId::A,
                enabled: true,
            },
        ));
        let (samples, timings) = render_measured(&mut engine, 4_800, 256);
        assert_eq!(samples.len(), 4_800);
        assert_eq!(timings.per_block_us.len(), 19, "4800 / 256 rounded up");
        let summary = timings.summary().unwrap_or_else(|| panic!("summary"));
        assert_eq!(summary.blocks, 19);
        assert!((summary.period_us - 5_333.333).abs() < 1.0);
        assert!(summary.p50_us > 0.0);
        assert!(summary.p99_us >= summary.p50_us);
        assert!(summary.max_us >= summary.p99_us);
    }

    #[test]
    fn an_empty_render_has_no_summary() {
        let timings = BlockTimings::default();
        assert!(timings.summary().is_none());
    }
}
