use std::f32::consts::PI;

const SCALE: f32 = 1.0 / 32768.0;

/// Pre-compute Hamming window coefficients with i16 normalization baked in.
pub fn hamming(size: usize) -> Vec<f32> {
    let mut window = Vec::with_capacity(size);
    let size_minus_1 = (size - 1) as f32;
    for i in 0..size {
        let w = SCALE * (0.54 - 0.46 * (i as f32 * 2.0 * PI / size_minus_1).cos());
        window.push(w);
    }
    window
}
