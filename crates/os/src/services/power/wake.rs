//! Readiness of a standby restore. Cycle tags reject acknowledgements from an
//! earlier attempt; a panel command alone cannot confirm a fresh visible frame.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WakeCycle {
    pub cycle: u32,
    restoring: bool,
    panel_ready: bool,
    frame_done: bool,
    brightness_done: bool,
}

impl WakeCycle {
    pub const fn new() -> Self {
        Self {
            cycle: 0,
            restoring: false,
            panel_ready: false,
            frame_done: false,
            brightness_done: false,
        }
    }

    pub fn begin(&mut self) {
        *self = Self {
            cycle: self.cycle.wrapping_add(1).max(1),
            ..Self::new()
        };
    }

    pub fn restoring(&mut self) {
        if self.cycle != 0 {
            self.restoring = true;
            self.panel_ready = false;
            self.frame_done = false;
            self.brightness_done = false;
        }
    }

    pub fn panel_ready(&mut self, cycle: u32) {
        if cycle == self.cycle && self.restoring {
            self.panel_ready = true;
        }
    }

    pub fn frame_done(&mut self, cycle: u32) {
        if cycle == self.cycle && self.panel_ready {
            self.frame_done = true;
        }
    }

    pub fn brightness_done(&mut self, cycle: u32) -> bool {
        if cycle == self.cycle && self.frame_done && !self.brightness_done {
            self.brightness_done = true;
            return true;
        }
        false
    }

    pub fn complete(&self) -> bool {
        self.brightness_done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_ack_and_frame_alone_do_not_complete_restore() {
        let mut wake = WakeCycle::new();
        wake.begin();
        wake.restoring();
        assert!(!wake.brightness_done(wake.cycle));
        wake.panel_ready(wake.cycle);
        assert!(!wake.complete());
        wake.frame_done(wake.cycle);
        assert!(!wake.complete());
        assert!(wake.brightness_done(wake.cycle));
        assert!(wake.complete());
        assert!(!wake.brightness_done(wake.cycle));
    }

    #[test]
    fn new_cycle_rejects_old_panel_frame_and_brightness_acknowledgements() {
        let mut wake = WakeCycle::new();
        wake.begin();
        let old = wake.cycle;
        wake.restoring();
        wake.panel_ready(old);
        wake.begin();
        wake.restoring();
        wake.panel_ready(old);
        wake.frame_done(wake.cycle);
        assert!(!wake.brightness_done(wake.cycle));
        wake.panel_ready(wake.cycle);
        wake.frame_done(old);
        assert!(!wake.brightness_done(old));
        assert!(!wake.brightness_done(wake.cycle));
        wake.frame_done(wake.cycle);
        assert!(wake.brightness_done(wake.cycle));
    }

    #[test]
    fn retry_requires_a_new_panel_and_frame_acknowledgement() {
        let mut wake = WakeCycle::new();
        wake.begin();
        wake.restoring();
        wake.panel_ready(wake.cycle);
        wake.frame_done(wake.cycle);
        wake.restoring();
        assert!(!wake.brightness_done(wake.cycle));
        assert!(!wake.complete());
        wake.panel_ready(wake.cycle);
        wake.frame_done(wake.cycle);
        assert!(wake.brightness_done(wake.cycle));
    }

    #[test]
    fn wrapped_cycle_never_reuses_the_initial_boot_generation() {
        let mut wake = WakeCycle {
            cycle: u32::MAX,
            ..WakeCycle::new()
        };
        wake.begin();
        assert_eq!(wake.cycle, 1);
        wake.restoring();
        wake.panel_ready(0);
        wake.frame_done(0);
        assert!(!wake.brightness_done(0));
        assert!(!wake.complete());
    }
}
