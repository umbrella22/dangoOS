//! Press detection for one capacitive key, from the touch block's smooth
//! data and benchmark. Hardware independent so it runs in host tests.
//!
//! A finger raises the reading above the benchmark. A press needs the rise
//! to pass [`PRESS_PERMILLE`] of the benchmark; it releases once the rise
//! falls under [`RELEASE_PERMILLE`]. The gap keeps a finger hovering at the
//! edge from chattering. Absolute floors stop noise on a tiny benchmark from
//! counting as a touch.

/// Rise over the benchmark that counts as a press, in 1/1000.
pub const PRESS_PERMILLE: u32 = 20;
/// Rise under which a press releases, in 1/1000.
pub const RELEASE_PERMILLE: u32 = 10;
/// Smallest rise ever treated as a press.
pub const MIN_PRESS_DELTA: u32 = 40;
/// Smallest release level.
pub const MIN_RELEASE_DELTA: u32 = 20;
/// Benchmarks outside this range mean no pad is there or the pad is shorted:
/// fall back to plain GPIO keys.
pub const MIN_PLAUSIBLE_BENCHMARK: u16 = 200;
pub const MAX_PLAUSIBLE_BENCHMARK: u16 = 0xfff0;
/// Samples to wait after start for the benchmark to settle.
pub const WARMUP_SAMPLES: u8 = 12;

/// Rise over `benchmark` that counts as a press.
pub fn press_delta(benchmark: u16) -> u32 {
    (u32::from(benchmark) * PRESS_PERMILLE / 1000).max(MIN_PRESS_DELTA)
}

/// Rise under which a press releases.
pub fn release_delta(benchmark: u16) -> u32 {
    (u32::from(benchmark) * RELEASE_PERMILLE / 1000).max(MIN_RELEASE_DELTA)
}

/// Threshold for the hardware so its benchmark freezes while touched.
pub fn hardware_threshold(benchmark: u16) -> u16 {
    press_delta(benchmark).min(u32::from(u16::MAX)) as u16
}

pub fn plausible(benchmark: u16) -> bool {
    (MIN_PLAUSIBLE_BENCHMARK..=MAX_PLAUSIBLE_BENCHMARK).contains(&benchmark)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TouchKey {
    pressed: bool,
}

impl TouchKey {
    pub const fn new() -> Self {
        Self { pressed: false }
    }

    pub fn pressed(&self) -> bool {
        self.pressed
    }

    /// Feed one reading; returns whether the key is held.
    pub fn update(&mut self, smooth: u16, benchmark: u16) -> bool {
        let rise = u32::from(smooth.saturating_sub(benchmark));
        self.pressed = if self.pressed {
            rise >= release_delta(benchmark)
        } else {
            rise >= press_delta(benchmark)
        };
        self.pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_needs_the_full_rise() {
        let mut k = TouchKey::new();
        assert!(!k.update(10_190, 10_000));
        assert!(k.update(10_200, 10_000));
    }

    #[test]
    fn release_uses_the_lower_level() {
        let mut k = TouchKey::new();
        assert!(k.update(10_300, 10_000));
        assert!(k.update(10_150, 10_000), "between the levels stays held");
        assert!(!k.update(10_090, 10_000));
        assert!(
            !k.update(10_150, 10_000),
            "between the levels stays released"
        );
    }

    #[test]
    fn readings_below_the_benchmark_never_press() {
        let mut k = TouchKey::new();
        assert!(!k.update(5_000, 10_000));
    }

    #[test]
    fn small_benchmarks_use_the_floor() {
        assert_eq!(press_delta(500), MIN_PRESS_DELTA);
        assert_eq!(release_delta(500), MIN_RELEASE_DELTA);
        let mut k = TouchKey::new();
        assert!(!k.update(530, 500));
        assert!(k.update(540, 500));
    }

    #[test]
    fn plausibility_rejects_empty_and_saturated() {
        assert!(!plausible(0));
        assert!(!plausible(u16::MAX));
        assert!(plausible(12_000));
    }

    #[test]
    fn hardware_threshold_matches_press_level() {
        assert_eq!(hardware_threshold(10_000), 200);
        assert_eq!(hardware_threshold(100), MIN_PRESS_DELTA as u16);
    }
}
