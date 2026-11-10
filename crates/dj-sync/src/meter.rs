//! How far two decks drift apart, measured the way the acceptance criterion measures it (AC-SYNC).

/// The worst and the average distance between two beats, kept while the music runs.
///
/// A single reading says nothing: two decks are in sync at the moment they are started and drift apart
/// afterwards, so what a check has to look at is the whole run. The meter keeps the worst error it was
/// shown and the average of the absolute errors, which is enough to tell "in sync" from "wandered".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DriftMeter {
    samples: u64,
    worst_ms: f64,
    sum_abs_ms: f64,
    worst_at: Option<u64>,
}

impl DriftMeter {
    /// An empty meter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one reading of the signed phase error, in milliseconds, taken at `frame` of the run.
    pub fn record(&mut self, error_ms: f64, frame: u64) {
        if !error_ms.is_finite() {
            return;
        }
        self.samples += 1;
        self.sum_abs_ms += error_ms.abs();
        if error_ms.abs() > self.worst_ms {
            self.worst_ms = error_ms.abs();
            self.worst_at = Some(frame);
        }
    }

    /// Readings taken.
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// The largest absolute error seen, in milliseconds.
    pub fn worst_ms(&self) -> f64 {
        self.worst_ms
    }

    /// Where the largest error was seen, in frames of the run.
    pub fn worst_at(&self) -> Option<u64> {
        self.worst_at
    }

    /// The average of the absolute errors, in milliseconds.
    pub fn mean_abs_ms(&self) -> f64 {
        if self.samples == 0 {
            return 0.0;
        }
        self.sum_abs_ms / self.samples as f64
    }

    /// Whether every reading stayed inside `tolerance_ms`.
    pub fn within(&self, tolerance_ms: f64) -> bool {
        self.worst_ms <= tolerance_ms
    }
}
