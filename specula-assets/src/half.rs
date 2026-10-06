//! IEEE half precision, for animation rotations and bake-unit's vertex
//! animation textures.

/// Rounds to nearest even. Unit coordinates are a few metres, where a half
/// keeps about a millimetre.
pub fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xFF) as i32;
    let mantissa = bits & 0x007F_FFFF;
    if exponent == 0xFF {
        let nan = if mantissa != 0 { 0x0200 } else { 0 };
        return sign | 0x7C00 | nan;
    }
    let half_exponent = exponent - 127 + 15;
    if half_exponent >= 0x1F {
        return sign | 0x7C00;
    }
    if half_exponent <= 0 {
        if half_exponent < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x0080_0000;
        let shift = (14 - half_exponent) as u32;
        let half = mantissa >> shift;
        let remainder = mantissa & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let round = remainder > halfway || (remainder == halfway && half & 1 == 1);
        return sign | (half + u32::from(round)) as u16;
    }
    let half = ((half_exponent as u32) << 10) | (mantissa >> 13);
    let remainder = mantissa & 0x1FFF;
    let round = remainder > 0x1000 || (remainder == 0x1000 && half & 1 == 1);
    // A carry out of the mantissa correctly bumps the exponent.
    sign | (half + u32::from(round)) as u16
}

pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x1F);
    let mantissa = u32::from(bits & 0x03FF);
    let magnitude = match exponent {
        0 => {
            // Subnormal: exact in f32, so scale instead of renormalising.
            let value = mantissa as f32 / 1024.0 / 16384.0;
            return if sign != 0 { -value } else { value };
        }
        0x1F => 0x7F80_0000 | (mantissa << 13),
        _ => ((exponent + 127 - 15) << 23) | (mantissa << 13),
    };
    f32::from_bits(sign | magnitude)
}
