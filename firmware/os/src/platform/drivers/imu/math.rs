//! Integer scaling and fixed-point square root used by sensor samples.

pub(super) fn scale_i16(raw: i16, full_scale: i32, divisor: i32) -> i16 {
    ((raw as i32 * full_scale) / divisor) as i16
}

/// `f32::sqrt` is not available in `core`; use a fixed-point integer square
/// root on the squared magnitude.
pub(super) fn sqrt_f32(value: f32) -> f32 {
    if value <= 0.0 {
        return 0.0;
    }
    // sqrt(value * 1e6) / 1e3 == sqrt(value); keep three fractional digits.
    isqrt((value * 1_000_000.0) as u64) as f32 / 1000.0
}

fn isqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}
