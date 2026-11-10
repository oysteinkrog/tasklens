//! Packs a path vertex's `st` coordinates as two f16 values in one u32
//! (tasklens-1ibm6). The vertex shader unpacks them with `unpack_st` in
//! `shaders.wgsl`.
//!
//! GPUI and tasklens only use 0, 0.5 and 1 for `st`, which f16 holds
//! exactly. Other values round to the nearest f16 (11 significant bits).

use gpui::Point;

/// `st` of a triangle that is fully inside its path, (0, 1): what every
/// `line_to` and every chart triangle uses.
const INSIDE: Point<f32> = Point { x: 0.0, y: 1.0 };

/// `pack_st(INSIDE)`: s = 0 in the low half, t = 1.0 (f16 `0x3c00`) in the
/// high half.
const INSIDE_PACKED: u32 = 0x3c00_0000;

/// Packs `st` as two f16 values, s in bits 0 to 15 and t in bits 16 to 31,
/// the order `unpack_st` reads them in.
#[inline]
pub(crate) fn pack_st(st: Point<f32>) -> u32 {
    if st == INSIDE {
        return INSIDE_PACKED;
    }
    u32::from(f16_bits(st.x)) | (u32::from(f16_bits(st.y)) << 16)
}

/// The IEEE 754 binary16 bits of `value`, rounded to nearest, ties to even.
/// Too large values become infinity, too small ones zero or a subnormal, and
/// NaN stays NaN.
pub(crate) fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;

    if exponent == 0xff {
        let nan = if mantissa != 0 { 0x0200 } else { 0 };
        return sign | 0x7c00 | nan;
    }
    // The exponent rebased from the f32 bias (127) to the f16 bias (15).
    let half_exponent = exponent - 112;
    if half_exponent >= 0x1f {
        return sign | 0x7c00;
    }
    if half_exponent <= 0 {
        // A subnormal f16, or zero: shift the 24-bit significand (with its
        // implicit bit) down to 10 bits.
        if half_exponent < -10 {
            return sign;
        }
        let significand = mantissa | 0x0080_0000;
        let shift = (14 - half_exponent) as u32;
        let half = significand >> shift;
        let rest = significand & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let round_up = rest > halfway || (rest == halfway && half & 1 == 1);
        return sign | (half + u32::from(round_up)) as u16;
    }
    let half = ((half_exponent as u32) << 10) | (mantissa >> 13);
    let rest = mantissa & 0x1fff;
    let round_up = rest > 0x1000 || (rest == 0x1000 && half & 1 == 1);
    // A carry out of the mantissa raises the exponent, which is the correct
    // rounding, up to infinity.
    sign | (half + u32::from(round_up)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes f16 bits, as `f16_to_f32` in `shaders.wgsl` does.
    fn f16_value(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exponent = i32::from((bits >> 10) & 0x1f);
        let mantissa = f32::from(bits & 0x03ff);
        match exponent {
            0 => sign * mantissa * 2f32.powi(-24),
            0x1f if mantissa == 0.0 => sign * f32::INFINITY,
            0x1f => f32::NAN,
            _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
        }
    }

    #[test]
    fn gpui_st_values_are_exact() {
        for (value, bits) in [(0.0, 0x0000), (0.5, 0x3800), (1.0, 0x3c00), (-0.0, 0x8000)] {
            assert_eq!(f16_bits(value), bits, "{value}");
            assert_eq!(f16_value(bits).to_bits(), f32::to_bits(value), "{value}");
        }
    }

    #[test]
    fn pack_st_puts_s_low_and_t_high() {
        assert_eq!(pack_st(INSIDE), INSIDE_PACKED);
        assert_eq!(pack_st(Point { x: 0.0, y: 1.0 }), 0x3c00_0000);
        assert_eq!(pack_st(Point { x: 0.5, y: 0.0 }), 0x0000_3800);
        assert_eq!(pack_st(Point { x: 1.0, y: 1.0 }), 0x3c00_3c00);
        assert_eq!(pack_st(Point { x: 0.0, y: 0.0 }), 0);
    }

    #[test]
    fn rounds_to_nearest_even() {
        // 1 + 2^-11 is halfway between 1 and the next f16: ties to even (1).
        assert_eq!(f16_bits(1.0 + 2f32.powi(-11)), 0x3c00);
        // 1 + 3 * 2^-11 is halfway between odd 0x3c01 and even 0x3c02.
        assert_eq!(f16_bits(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        // Just above halfway rounds up.
        assert_eq!(f16_bits(1.0 + 2f32.powi(-11) + 2f32.powi(-20)), 0x3c01);
        // Rounding up out of the mantissa carries into the exponent.
        assert_eq!(f16_bits(2.0 - 2f32.powi(-12)), 0x4000);
    }

    #[test]
    fn edges() {
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(65520.0), 0x7c00);
        assert_eq!(f16_bits(1.0e9), 0x7c00);
        assert_eq!(f16_bits(-1.0e9), 0xfc00);
        assert_eq!(f16_bits(f32::INFINITY), 0x7c00);
        assert_eq!(f16_bits(f32::NEG_INFINITY), 0xfc00);
        assert!(f16_value(f16_bits(f32::NAN)).is_nan());
        // The smallest subnormal, 2^-24, and half of it (a tie, to even 0).
        assert_eq!(f16_bits(2f32.powi(-24)), 0x0001);
        assert_eq!(f16_bits(2f32.powi(-25)), 0x0000);
        assert_eq!(f16_bits(1.5 * 2f32.powi(-25)), 0x0001);
        assert_eq!(f16_bits(1.0e-10), 0x0000);
        // The largest subnormal and the smallest normal.
        assert_eq!(f16_bits(1023.0 * 2f32.powi(-24)), 0x03ff);
        assert_eq!(f16_bits(2f32.powi(-14)), 0x0400);
    }

    #[test]
    fn values_in_0_to_1_round_within_half_a_step() {
        for i in 0..=10_000u32 {
            let value = i as f32 / 10_000.0;
            let decoded = f16_value(f16_bits(value));
            let step = if value < 2f32.powi(-14) {
                2f32.powi(-24)
            } else {
                2f32.powi(value.log2().floor() as i32 - 10)
            };
            assert!(
                (decoded - value).abs() <= step / 2.0,
                "{value} -> {decoded}"
            );
        }
    }
}
