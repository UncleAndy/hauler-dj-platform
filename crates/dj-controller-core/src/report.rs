//! Bits out of a report, the way HID states them.
//!
//! A control is a run of bits somewhere in a report: `byte 4, bit 2, width 1` is the third button of a group. These
//! helpers are the only arithmetic a driver should need, and they refuse to read past the end of what arrived.

/// An unsigned field, low bits first and little-endian across byte boundaries.
///
/// Returns `None` when the field would reach past `payload` — a short report is a short answer, not a wrong one.
#[must_use]
pub fn unsigned(payload: &[u8], byte: usize, bit: usize, width: usize) -> Option<u16> {
    if width == 0 || width > 16 || bit >= 8 {
        return None;
    }
    let end = bit.checked_add(width)?;
    let needed = end.div_ceil(8);
    let slice = payload.get(byte..byte.checked_add(needed)?)?;
    let mut bits: u32 = 0;
    for (index, value) in slice.iter().enumerate() {
        bits |= u32::from(*value) << (8 * index);
    }
    let mask = (1_u32 << width) - 1;
    u16::try_from((bits >> bit) & mask).ok()
}

/// A 32-bit little-endian word, for the counters a jog wheel sends.
#[must_use]
pub fn u32_le(payload: &[u8], byte: usize) -> Option<u32> {
    let slice = payload.get(byte..byte.checked_add(4)?)?;
    let mut bytes = [0_u8; 4];
    bytes.copy_from_slice(slice);
    Some(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bit_inside_one_byte() {
        // 0b0000_1000: bit 3 is set.
        assert_eq!(unsigned(&[0b0000_1000], 0, 3, 1), Some(1));
        assert_eq!(unsigned(&[0b0000_1000], 0, 2, 1), Some(0));
    }

    #[test]
    fn a_field_that_crosses_a_byte_boundary() {
        // Bits 0..8 come from both bytes, low bits first: 0b1 from the second byte is bit 8.
        assert_eq!(unsigned(&[0xff, 0x01], 0, 4, 8), Some(0x1f));
    }

    #[test]
    fn a_field_that_does_not_fit_is_not_guessed() {
        assert_eq!(unsigned(&[0x01], 0, 4, 8), None);
        assert_eq!(unsigned(&[0x01], 4, 0, 1), None);
        assert_eq!(unsigned(&[0x01], 0, 8, 1), None);
        assert_eq!(unsigned(&[0x01], 0, 0, 0), None);
    }

    #[test]
    fn a_counter_is_little_endian() {
        assert_eq!(u32_le(&[0x01, 0x02, 0x03, 0x04], 0), Some(0x0403_0201));
        assert_eq!(u32_le(&[0, 0, 0, 0x80], 0), Some(0x8000_0000));
        assert_eq!(u32_le(&[1, 2], 0), None);
    }
}
