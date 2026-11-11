use super::filter::HaarFilter;
use super::integral_image::RollingIntegralImage;
use super::quantizer::Quantizer;

/// Gray code lookup table for 2-bit encoding.
const GRAY_CODE: [u32; 4] = [0, 1, 3, 2];

/// A classifier pairs a Haar-like filter with a quantizer.
/// Each classifier contributes 2 bits to the fingerprint.
#[derive(Debug, Clone, Copy)]
pub struct Classifier {
    pub filter: HaarFilter,
    pub quantizer: Quantizer,
}

impl Classifier {
    /// Classify: apply filter, quantize result, return Gray-coded 2-bit value.
    #[inline(always)]
    pub fn classify(&self, image: &RollingIntegralImage, offset: usize) -> u32 {
        let value = self.filter.apply(image, offset);
        let quantized = self.quantizer.quantize(value);
        GRAY_CODE[quantized as usize]
    }
}
