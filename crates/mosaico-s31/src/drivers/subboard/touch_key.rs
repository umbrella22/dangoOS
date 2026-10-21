//! Press detection for one capacitive key from the touch block's smooth
//! data. Hardware independent so it runs in host tests.
//!
//! The baseline is tracked here rather than taken from the hardware
//! benchmark: that benchmark settles slowly after a reset and freezes once
//! the data is above its threshold, so an early calibration could leave a
//! key held forever.
//!
//! - The first reading after warmup becomes the baseline.
//! - A finger raises the reading. A rise of [`PRESS_PERMILLE`] of the
//!   baseline presses; falling under [`RELEASE_PERMILLE`] releases. The gap
//!   stops a hovering finger from chattering.
//! - While released the baseline follows slow drift upward (1/32 of the gap
//!   per sample) and drops faster (1/4) when the reading falls below it.
//!   It freezes while pressed.
//! - A press held for [`STUCK_SAMPLES`] is treated as a stale baseline: the
//!   current reading becomes the new baseline.

/// Rise over the baseline that counts as a press, in 1/1000.
pub const PRESS_PERMILLE: u32 = 20;
/// Rise under which a press releases, in 1/1000.
pub const RELEASE_PERMILLE: u32 = 10;
/// Smallest rise ever treated as a press.
pub const MIN_PRESS_DELTA: u32 = 40;
/// Smallest release level.
pub const MIN_RELEASE_DELTA: u32 = 20;
/// Readings outside this range mean no pad is there or the pad is shorted:
/// fall back to plain GPIO keys.
pub const MIN_PLAUSIBLE: u16 = 200;
pub const MAX_PLAUSIBLE: u16 = 0xfff0;
/// Samples to skip after start while the smooth filter fills.
pub const WARMUP_SAMPLES: u8 = 12;
/// Continuous press that resets the baseline (25 Hz -> 20 s).
pub const STUCK_SAMPLES: u16 = 500;

/// Baseline fraction bits.
const FRAC: u32 = 4;
const RISE_SHIFT: u32 = 5;
const FALL_SHIFT: u32 = 2;

/// Rise over `baseline` that counts as a press.
pub fn press_delta(baseline: u32) -> u32 {
    (baseline * PRESS_PERMILLE / 1000).max(MIN_PRESS_DELTA)
}

/// Rise under which a press releases.
pub fn release_delta(baseline: u32) -> u32 {
    (baseline * RELEASE_PERMILLE / 1000).max(MIN_RELEASE_DELTA)
}

pub fn plausible(reading: u16) -> bool {
    (MIN_PLAUSIBLE..=MAX_PLAUSIBLE).contains(&reading)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TouchKey {
    /// Baseline with `FRAC` fraction bits; 0 until the first reading.
    base: u32,
    pressed: bool,
    held_samples: u16,
}

impl TouchKey {
    pub const fn new() -> Self {
        Self {
            base: 0,
            pressed: false,
            held_samples: 0,
        }
    }

    pub fn pressed(&self) -> bool {
        self.pressed
    }

    pub fn baseline(&self) -> u32 {
        self.base >> FRAC
    }

    /// Feed one smooth reading; returns whether the key is held.
    pub fn update(&mut self, smooth: u16) -> bool {
        let reading = u32::from(smooth);
        if self.base == 0 {
            self.base = reading << FRAC;
            return false;
        }
        let baseline = self.baseline();
        let rise = reading.saturating_sub(baseline);
        self.pressed = if self.pressed {
            rise >= release_delta(baseline)
        } else {
            rise >= press_delta(baseline)
        };
        if self.pressed {
            self.held_samples += 1;
            if self.held_samples >= STUCK_SAMPLES {
                self.base = reading << FRAC;
                self.pressed = false;
                self.held_samples = 0;
            }
        } else {
            self.held_samples = 0;
            let target = reading << FRAC;
            if target < self.base {
                self.base -= (self.base - target) >> FALL_SHIFT;
            } else {
                self.base += (target - self.base) >> RISE_SHIFT;
            }
        }
        self.pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(level: u16) -> TouchKey {
        let mut k = TouchKey::new();
        for _ in 0..10 {
            assert!(!k.update(level));
        }
        k
    }

    #[test]
    fn first_reading_is_the_baseline() {
        let mut k = TouchKey::new();
        assert!(!k.update(36_000));
        assert_eq!(k.baseline(), 36_000);
    }

    #[test]
    fn press_needs_the_full_rise() {
        let mut k = settled(10_000);
        assert!(!k.update(10_190));
        let mut k = settled(10_000);
        assert!(k.update(10_200));
    }

    #[test]
    fn release_uses_the_lower_level() {
        let mut k = settled(10_000);
        assert!(k.update(10_300));
        assert!(k.update(10_150), "between the levels stays held");
        assert!(!k.update(10_090));
    }

    #[test]
    fn baseline_freezes_while_pressed() {
        let mut k = settled(10_000);
        for _ in 0..100 {
            assert!(k.update(10_400));
        }
        assert_eq!(k.baseline(), 10_000);
        assert!(!k.update(10_000));
    }

    #[test]
    fn slow_drift_is_followed() {
        let mut k = settled(10_000);
        for step in 0..400u16 {
            assert!(!k.update(10_000 + step), "drift must not press");
        }
        assert!(k.baseline() > 10_300);
    }

    #[test]
    fn baseline_drops_quickly() {
        let mut k = settled(10_000);
        for _ in 0..20 {
            k.update(9_000);
        }
        assert!(k.baseline() < 9_050);
    }

    #[test]
    fn a_stuck_press_rebaselines() {
        let mut k = settled(10_000);
        for _ in 0..STUCK_SAMPLES - 1 {
            assert!(k.update(12_000));
        }
        assert!(!k.update(12_000));
        assert_eq!(k.baseline(), 12_000);
        assert!(!k.update(12_000));
    }

    #[test]
    fn small_levels_use_the_floor() {
        assert_eq!(press_delta(500), MIN_PRESS_DELTA);
        assert_eq!(release_delta(500), MIN_RELEASE_DELTA);
    }

    #[test]
    fn plausibility_rejects_empty_and_saturated() {
        assert!(!plausible(0));
        assert!(!plausible(u16::MAX));
        assert!(plausible(36_000));
    }
}
