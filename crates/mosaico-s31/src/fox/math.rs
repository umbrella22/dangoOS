//! Float helpers for a target without libm.
//!
//! `core` has no transcendental functions on `riscv32imafc-unknown-none-elf`,
//! so the animation uses these approximations. They are shared by the
//! per-expression animator and the entrance.

pub const PI: f32 = core::f32::consts::PI;
pub const TAU: f32 = core::f32::consts::TAU;

/// Round half away from zero.
pub fn round(v: f32) -> i32 {
    if v >= 0.0 {
        (v + 0.5) as i32
    } else {
        (v - 0.5) as i32
    }
}

/// Sine, 7th-order Taylor after folding into `-pi/2..=pi/2` (error below
/// 2e-4).
pub fn sin(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let mut x = x % TAU;
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }
    if x > PI * 0.5 {
        x = PI - x;
    } else if x < -PI * 0.5 {
        x = -PI - x;
    }
    let x2 = x * x;
    x * (1.0 + x2 * (-1.0 / 6.0 + x2 * (1.0 / 120.0 - x2 / 5040.0)))
}

pub fn cos(x: f32) -> f32 {
    sin(x + PI / 2.0)
}

/// `exp(-x)` for `x >= 0`: `(1 + x/16)^-16`.
pub fn exp_neg(x: f32) -> f32 {
    let t = 1.0 + x.max(0.0) / 16.0;
    let t2 = t * t;
    let t4 = t2 * t2;
    let t8 = t4 * t4;
    1.0 / (t8 * t8)
}

/// Clamp, mapping NaN and infinity to `1.0` (the neutral scale).
pub fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if !v.is_finite() {
        1.0
    } else if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `1 / sqrt(x)` for `x > 0`, two Newton steps.
pub fn inv_sqrt(x: f32) -> f32 {
    let i = 0x5f37_59df - (x.to_bits() >> 1);
    let mut y = f32::from_bits(i);
    y *= 1.5 - 0.5 * x * y * y;
    y *= 1.5 - 0.5 * x * y * y;
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_are_close_to_std() {
        let mut x = -10.0f32;
        while x < 10.0 {
            assert!((sin(x) - std::primitive::f32::sin(x)).abs() < 2e-3, "sin {x}");
            assert!((cos(x) - std::primitive::f32::cos(x)).abs() < 2e-3, "cos {x}");
            x += 0.01;
        }
        let mut x = 0.0f32;
        while x < 8.0 {
            assert!((exp_neg(x) - std::primitive::f32::exp(-x)).abs() < 0.02, "exp {x}");
            x += 0.05;
        }
        for v in [0.25f32, 1.0, 2.0, 100.0] {
            assert!((inv_sqrt(v) - 1.0 / std::primitive::f32::sqrt(v)).abs() < 1e-3);
        }
        assert_eq!((round(1.5), round(-1.5), round(0.49)), (2, -2, 0));
    }
}
