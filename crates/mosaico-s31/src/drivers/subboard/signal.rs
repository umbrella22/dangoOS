//! Debounce and light filtering for slot modules.
//!
//! Pure and dependency-free so `scripts/test-drivers.sh` can test it on the
//! host.

/// Consecutive samples needed to accept a change (official module manager).
pub const DEBOUNCE_SAMPLES: u8 = 3;

/// Boolean edge state with three-sample debounce for raw levels.
#[derive(Clone, Copy, Debug)]
pub struct Debounce {
    stable: Option<bool>,
    candidate: bool,
    count: u8,
}

impl Debounce {
    /// Adopt the first sample silently. Used for keys and PIR: a key held
    /// while the board is plugged in is not a press.
    pub const fn new() -> Self {
        Self {
            stable: None,
            candidate: false,
            count: 0,
        }
    }

    /// Start from a known level. Used for EEPROM presence: a module already
    /// plugged in at boot is reported as an insertion.
    pub const fn starting_at(level: bool) -> Self {
        Self {
            stable: Some(level),
            candidate: level,
            count: 0,
        }
    }

    /// Accept an already-filtered level without filtering it a second time.
    /// Shares the silent first sample and accepted state with raw GPIO input.
    pub fn accept_filtered(&mut self, level: bool) -> Option<bool> {
        let previous = self.stable.replace(level);
        self.candidate = level;
        self.count = 0;
        previous
            .filter(|previous| *previous != level)
            .map(|_| level)
    }

    /// Feed one raw sample. Returns `Some(level)` once
    /// [`DEBOUNCE_SAMPLES`] consecutive samples disagree with the accepted
    /// level.
    pub fn update(&mut self, raw: bool) -> Option<bool> {
        let Some(stable) = self.stable else {
            return self.accept_filtered(raw);
        };
        if raw == stable {
            self.count = 0;
            return None;
        }
        if raw == self.candidate && self.count > 0 {
            self.count += 1;
        } else {
            self.candidate = raw;
            self.count = 1;
        }
        if self.count >= DEBOUNCE_SAMPLES {
            self.accept_filtered(raw)
        } else {
            None
        }
    }
}

impl Default for Debounce {
    fn default() -> Self {
        Self::new()
    }
}

/// S31 ADC code for 0 V (BSP `LDR_S31_ZERO_CODE`).
const LDR_ZERO_CODE: i32 = 2196;
/// S31 ADC code for full scale on a P-side channel (BSP `LDR_S31_HIGH_CODE`).
const LDR_HIGH_CODE: i32 = 4393;

/// LDR level `0..=100` with the official 3:1 IIR filter
/// (`mosaico_interact_read_inputs`).
///
/// The S31 SAR ADC is differential: odd channels are P-side and their code
/// rises with voltage from [`LDR_ZERO_CODE`]; even channels are N-side and
/// their code falls from it. The left slot LDR (`GPIO53`, ADC2 channel 3) and
/// the right slot LDR (`GPIO46`, ADC1 channel 4) therefore need opposite
/// signs.
#[derive(Clone, Copy, Debug)]
pub struct LdrFilter {
    inverted: bool,
    filtered_q8: Option<i32>,
}

impl LdrFilter {
    pub const fn new(adc_channel: u8) -> Self {
        Self {
            inverted: adc_channel & 1 == 0,
            filtered_q8: None,
        }
    }

    /// Forget the filter state (module removed).
    pub fn reset(&mut self) {
        self.filtered_q8 = None;
    }

    /// Feed one raw ADC code and return the filtered level `0..=100`.
    pub fn update(&mut self, raw: u16) -> u8 {
        let raw = raw as i32;
        let (level, span) = if self.inverted {
            (LDR_ZERO_CODE - raw, LDR_ZERO_CODE)
        } else {
            (raw - LDR_ZERO_CODE, LDR_HIGH_CODE - LDR_ZERO_CODE)
        };
        let level = level.clamp(0, span);
        let target_q8 = (level * 100 * 256 + span / 2) / span;
        let filtered = match self.filtered_q8 {
            None => target_q8,
            Some(previous) => (previous * 3 + target_q8 + 2) / 4,
        };
        self.filtered_q8 = Some(filtered);
        ((filtered + 128) / 256).clamp(0, 100) as u8
    }
}

#[cfg(test)]
#[path = "touch_key.rs"]
mod touch_key;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_is_adopted_without_an_edge() {
        let mut key = Debounce::new();
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), None);
        let release: Vec<_> = (0..3).map(|_| key.update(false)).collect();
        assert_eq!(release, [None, None, Some(false)]);
    }

    #[test]
    fn change_needs_three_consecutive_samples() {
        let mut key = Debounce::starting_at(false);
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), Some(true));
        assert_eq!(key.update(true), None);
    }

    #[test]
    fn a_bounce_restarts_the_count() {
        let mut key = Debounce::starting_at(false);
        key.update(true);
        key.update(true);
        assert_eq!(key.update(false), None);
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), Some(true));
    }

    #[test]
    fn filtered_levels_keep_initial_silence_and_clear_raw_candidates() {
        let mut key = Debounce::new();
        assert_eq!(key.accept_filtered(true), None);
        assert_eq!(key.accept_filtered(true), None);
        assert_eq!(key.accept_filtered(false), Some(false));
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), None);
        assert_eq!(key.accept_filtered(false), None);
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), None);
        assert_eq!(key.update(true), Some(true));
    }

    #[test]
    fn recorded_touch_taps_with_five_quiet_samples_keep_both_releases() {
        // Continuous smooth sequences from 24-fast-keys-trace.log. Use the
        // actual TouchKey implementation, not a second filter model.
        let traces: &[(u16, &[u16])] = &[
            (
                15139,
                &[
                    31612, 38837, 32372, 15240, 15098, 15198, 15204, 15232, 31799, 41431, 16594,
                    15203, 15187, 15187, 15192, 15191, 15186,
                ],
            ),
            (
                15151,
                &[
                    34481, 38245, 15255, 14724, 15181, 15179, 15195, 28010, 37855, 15223, 14645,
                    15188, 15180, 15185, 15180,
                ],
            ),
            (
                14600,
                &[
                    22279, 37463, 14721, 14622, 14615, 14604, 14607, 23243, 44334, 42065, 14664,
                    14625, 14613, 14609, 14612,
                ],
            ),
        ];
        for &(baseline, readings) in traces {
            let mut touch = super::touch_key::TouchKey::new();
            let mut key = Debounce::new();
            let mut old_second_filter = Debounce::new();
            let idle = touch.update(baseline);
            assert_eq!(key.accept_filtered(idle), None);
            old_second_filter.update(idle);
            let mut edges = Vec::new();
            let mut old_edges = Vec::new();
            for &smooth in readings {
                let held = touch.update(smooth);
                edges.extend(key.accept_filtered(held));
                old_edges.extend(old_second_filter.update(held));
            }
            assert_eq!(edges, [true, false, true, false]);
            assert_eq!(old_edges.iter().filter(|&&pressed| pressed).count(), 1);
        }
    }

    #[test]
    fn filtered_touch_stays_held_through_single_baseline_crossing_and_long_hold() {
        let mut touch = super::touch_key::TouchKey::new();
        let mut key = Debounce::new();
        key.accept_filtered(touch.update(15000));
        assert_eq!(key.accept_filtered(touch.update(35000)), Some(true));
        // Four seconds held, one baseline crossing, then another four seconds
        // swinging below baseline: neither may invent a release or press.
        for smooth in core::iter::repeat_n(35000, 100)
            .chain(core::iter::once(15000))
            .chain(core::iter::repeat_n(2000, 100))
        {
            assert!(touch.update(smooth));
            assert_eq!(key.accept_filtered(touch.pressed()), None);
        }
        for _ in 0..3 {
            assert_eq!(key.accept_filtered(touch.update(15000)), None);
        }
        assert_eq!(key.accept_filtered(touch.update(15000)), Some(false));
    }

    #[test]
    fn presence_at_boot_is_reported() {
        let mut slot = Debounce::starting_at(false);
        let edges: Vec<_> = (0..3).map(|_| slot.update(true)).collect();
        assert_eq!(edges, [None, None, Some(true)]);
    }

    #[test]
    fn even_channel_code_falls_with_light() {
        let mut right = LdrFilter::new(4);
        assert_eq!(right.update(2196), 0);
        right.reset();
        assert_eq!(right.update(0), 100);
        right.reset();
        assert_eq!(right.update(1098), 50);
    }

    #[test]
    fn odd_channel_code_rises_with_light() {
        let mut left = LdrFilter::new(3);
        assert_eq!(left.update(2196), 0);
        left.reset();
        assert_eq!(left.update(4393), 100);
        left.reset();
        assert_eq!(left.update(3295), 50);
    }

    #[test]
    fn out_of_range_codes_are_clamped() {
        let mut left = LdrFilter::new(3);
        assert_eq!(left.update(100), 0);
        let mut right = LdrFilter::new(4);
        assert_eq!(right.update(4000), 0);
    }

    #[test]
    fn filter_moves_a_quarter_of_the_way_each_sample() {
        let mut right = LdrFilter::new(4);
        assert_eq!(right.update(2196), 0);
        assert_eq!(right.update(0), 25);
        assert_eq!(right.update(0), 44);
    }
}
