/// 4-level scalar quantizer with 3 thresholds.
#[derive(Debug, Clone, Copy)]
pub struct Quantizer {
    pub t0: f64,
    pub t1: f64,
    pub t2: f64,
}

impl Quantizer {
    /// Quantize a value to one of 4 levels (0-3).
    #[inline(always)]
    pub fn quantize(&self, value: f64) -> u32 {
        if value < self.t1 {
            if value < self.t0 {
                0
            } else {
                1
            }
        } else {
            if value < self.t2 {
                2
            } else {
                3
            }
        }
    }
}
