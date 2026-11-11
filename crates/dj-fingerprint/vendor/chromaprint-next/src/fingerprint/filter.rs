use super::integral_image::RollingIntegralImage;

/// Haar-like rectangular filter descriptor.
/// 6 types of filters that compute comparisons on rectangular regions
/// of the chroma integral image.
#[derive(Debug, Clone, Copy)]
pub struct HaarFilter {
    /// Filter type (0-5).
    pub kind: u8,
    /// Y offset (chroma band start).
    pub y: u8,
    /// Height in chroma bands.
    pub height: u8,
    /// Width in time frames.
    pub width: u8,
}

/// SubtractLog comparator: log((1 + a) / (1 + b))
#[inline(always)]
fn subtract_log(a: f64, b: f64) -> f64 {
    ((1.0 + a) / (1.0 + b)).ln()
}

impl HaarFilter {
    /// Apply this filter to the integral image at time offset `x`.
    #[inline]
    pub fn apply(&self, image: &RollingIntegralImage, x: usize) -> f64 {
        let y = self.y as usize;
        let h = self.height as usize;
        let w = self.width as usize;

        match self.kind {
            0 => self.filter0(image, x, y, w, h),
            1 => self.filter1(image, x, y, w, h),
            2 => self.filter2(image, x, y, w, h),
            3 => self.filter3(image, x, y, w, h),
            4 => self.filter4(image, x, y, w, h),
            5 => self.filter5(image, x, y, w, h),
            _ => 0.0,
        }
    }

    #[inline(always)]
    fn filter0(&self, image: &RollingIntegralImage, x: usize, y: usize, w: usize, h: usize) -> f64 {
        let a = image.area(x, y, x + w, y + h);
        subtract_log(a, 0.0)
    }

    #[inline(always)]
    fn filter1(&self, image: &RollingIntegralImage, x: usize, y: usize, w: usize, h: usize) -> f64 {
        let h_2 = h / 2;
        let a = image.area(x, y + h_2, x + w, y + h);
        let b = image.area(x, y, x + w, y + h_2);
        subtract_log(a, b)
    }

    #[inline(always)]
    fn filter2(&self, image: &RollingIntegralImage, x: usize, y: usize, w: usize, h: usize) -> f64 {
        let w_2 = w / 2;
        let a = image.area(x + w_2, y, x + w, y + h);
        let b = image.area(x, y, x + w_2, y + h);
        subtract_log(a, b)
    }

    #[inline(always)]
    fn filter3(&self, image: &RollingIntegralImage, x: usize, y: usize, w: usize, h: usize) -> f64 {
        let w_2 = w / 2;
        let h_2 = h / 2;
        let a = image.area(x, y + h_2, x + w_2, y + h) + image.area(x + w_2, y, x + w, y + h_2);
        let b = image.area(x, y, x + w_2, y + h_2) + image.area(x + w_2, y + h_2, x + w, y + h);
        subtract_log(a, b)
    }

    #[inline(always)]
    fn filter4(&self, image: &RollingIntegralImage, x: usize, y: usize, w: usize, h: usize) -> f64 {
        let h_3 = h / 3;
        let a = image.area(x, y + h_3, x + w, y + 2 * h_3);
        let b = image.area(x, y, x + w, y + h_3) + image.area(x, y + 2 * h_3, x + w, y + h);
        subtract_log(a, b)
    }

    #[inline(always)]
    fn filter5(&self, image: &RollingIntegralImage, x: usize, y: usize, w: usize, h: usize) -> f64 {
        let w_3 = w / 3;
        let a = image.area(x + w_3, y, x + 2 * w_3, y + h);
        let b = image.area(x, y, x + w_3, y + h) + image.area(x + 2 * w_3, y, x + w, y + h);
        subtract_log(a, b)
    }
}
