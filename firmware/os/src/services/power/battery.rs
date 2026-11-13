//! Voltage-based reserve protection. SOC is deliberately not a cutoff input:
//! an uncalibrated gauge can report 8% at 4.1 V. Factory EDV0 is 3000 mV;
//! 3300 mV leaves reserve for the warning and the physical power latch.
pub const LOW_MV: u16 = 3300;
pub const RECOVER_MV: u16 = 3500;
pub const HARD_LOW_MV: u16 = 3100;
pub const WARNING_MS: u64 = 2500;
pub const CUT_DEADLINE_MS: u64 = 6000;

#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub voltage_mv: u16,
    pub current_ma: i16,
}
impl Reading {
    pub fn valid(self) -> bool {
        (2000..=4500).contains(&self.voltage_mv)
    }
    fn low(self) -> bool {
        self.voltage_mv <= HARD_LOW_MV || (self.voltage_mv <= LOW_MV && self.current_ma <= 5)
    }
}

pub struct Policy {
    confirm_ms: u64,
    since: Option<u64>,
    last: Option<u64>,
    latched: bool,
}
impl Policy {
    pub const fn new(confirm_ms: u64) -> Self {
        Self {
            confirm_ms,
            since: None,
            last: None,
            latched: false,
        }
    }
    /// Missing/invalid/stale samples break confirmation; a one-way request is
    /// emitted only once, after another low sample confirms the elapsed time.
    pub fn observe(&mut self, now: u64, reading: Option<Reading>) -> bool {
        if self.latched {
            return false;
        }
        let Some(reading) = reading.filter(|r| r.valid()) else {
            self.since = None;
            self.last = None;
            return false;
        };
        if self
            .last
            .is_some_and(|last| now.saturating_sub(last) > 1500)
        {
            self.since = None;
        }
        self.last = Some(now);
        if reading.voltage_mv >= RECOVER_MV
            || reading.current_ma > 5 && reading.voltage_mv > HARD_LOW_MV
        {
            self.since = None;
        }
        if !reading.low() {
            return false;
        }
        let since = *self.since.get_or_insert(now);
        let confirm = if reading.voltage_mv <= HARD_LOW_MV {
            self.confirm_ms.min(200)
        } else {
            self.confirm_ms
        };
        if now.saturating_sub(since) >= confirm {
            self.latched = true;
            return true;
        }
        false
    }
}

/// Critical shutdown is bounded even if display, LED, or radio shutdown fails.
/// A successful warning upload starts the visible interval, not its render call.
pub struct Shutdown {
    requested: u64,
    shown: Option<u64>,
    cut: bool,
}
impl Shutdown {
    pub const fn new(now: u64) -> Self {
        Self {
            requested: now,
            shown: None,
            cut: false,
        }
    }
    pub fn uploaded(&mut self, now: u64, completed: bool) {
        if completed && self.shown.is_none() {
            self.shown = Some(now);
        }
    }
    pub fn poll(&mut self, now: u64) -> bool {
        if !self.cut
            && (now.saturating_sub(self.requested) >= CUT_DEADLINE_MS
                || self
                    .shown
                    .is_some_and(|shown| now.saturating_sub(shown) >= WARNING_MS))
        {
            self.cut = true;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(mv: u16, ma: i16) -> Option<Reading> {
        Some(Reading {
            voltage_mv: mv,
            current_ma: ma,
        })
    }
    #[test]
    fn high_voltage_and_charging_do_not_cut_off_an_uncalibrated_gauge() {
        let mut p = Policy::new(3000);
        for n in 0..30 {
            assert!(!p.observe(n * 1000, r(4100, -120)));
        }
        for n in 30..60 {
            assert!(!p.observe(n * 1000, r(3250, 25)));
        }
    }
    #[test]
    fn sustained_low_voltage_latches_once_and_hard_low_ignores_charging() {
        let mut p = Policy::new(3000);
        for n in 0..3 {
            assert!(!p.observe(n * 1000, r(3250, -120)));
        }
        assert!(p.observe(3000, r(3250, -120)));
        assert!(!p.observe(4000, r(3200, -120)));
        let mut p = Policy::new(3000);
        assert!(!p.observe(0, r(3050, 10)));
        assert!(p.observe(1000, r(3050, 10)));
    }
    #[test]
    fn failed_invalid_or_stale_samples_and_recovery_break_confirmation() {
        for bad in [None, r(0, 0), r(65535, -100), r(3550, -100), r(3290, 20)] {
            let mut p = Policy::new(3000);
            assert!(!p.observe(0, r(3250, -100)));
            assert!(!p.observe(1000, bad));
            assert!(!p.observe(2000, r(3250, -100)));
            assert!(!p.observe(3000, r(3250, -100)));
        }
        let mut p = Policy::new(3000);
        assert!(!p.observe(0, r(3250, -100)));
        assert!(!p.observe(5000, r(3250, -100)));
    }
    #[test]
    fn boot_confirmation_is_short_but_requires_multiple_samples() {
        let mut p = Policy::new(200);
        assert!(!p.observe(0, r(3250, -100)));
        assert!(!p.observe(100, r(3250, -100)));
        assert!(p.observe(200, r(3250, -100)));
        let mut charged_boot = Policy::new(200);
        assert!(!charged_boot.observe(0, r(3900, -100)));
    }
    #[test]
    fn warning_receipt_not_render_time_starts_countdown_and_failure_is_bounded() {
        let mut s = Shutdown::new(1000);
        s.uploaded(1100, false);
        assert!(!s.poll(4000));
        s.uploaded(4100, true);
        s.uploaded(4200, true);
        assert!(!s.poll(6599));
        assert!(s.poll(6600));
        assert!(!s.poll(9000));
        let mut failed = Shutdown::new(1000);
        assert!(!failed.poll(6999));
        assert!(failed.poll(7000));
    }
}
