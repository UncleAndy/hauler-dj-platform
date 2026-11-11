/// Removes leading silence from an audio stream.
/// Uses a moving average of absolute sample values to detect audio onset.
pub struct SilenceRemover {
    threshold: i16,
    window_size: usize,
    rolling_sum: i64,
    ring_buffer: Vec<i16>,
    ring_pos: usize,
    ring_count: usize,
    started: bool,
}

impl SilenceRemover {
    pub fn new(threshold: i16) -> Self {
        let window_size = 55; // ~5ms at 11025 Hz
        Self {
            threshold,
            window_size,
            rolling_sum: 0,
            ring_buffer: vec![0i16; window_size],
            ring_pos: 0,
            ring_count: 0,
            started: false,
        }
    }

    pub fn reset(&mut self) {
        self.rolling_sum = 0;
        self.ring_buffer.fill(0);
        self.ring_pos = 0;
        self.ring_count = 0;
        self.started = false;
    }

    /// Process samples, removing leading silence.
    /// Calls callback with non-silent samples.
    pub fn consume<F>(&mut self, samples: &[i16], mut callback: F)
    where
        F: FnMut(&[i16]),
    {
        if self.started {
            callback(samples);
            return;
        }

        let mut start_idx = None;
        for (i, &sample) in samples.iter().enumerate() {
            let abs_val = sample.unsigned_abs() as i64;

            // Subtract oldest value from rolling sum
            if self.ring_count >= self.window_size {
                self.rolling_sum -= self.ring_buffer[self.ring_pos].unsigned_abs() as i64;
            }

            // Add new value
            self.ring_buffer[self.ring_pos] = sample;
            self.rolling_sum += abs_val;
            self.ring_pos = (self.ring_pos + 1) % self.window_size;
            if self.ring_count < self.window_size {
                self.ring_count += 1;
            }

            // Check if average exceeds threshold
            let avg = self.rolling_sum / self.ring_count as i64;
            if avg > self.threshold as i64 {
                self.started = true;
                start_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = start_idx {
            callback(&samples[idx..]);
        }
    }
}
