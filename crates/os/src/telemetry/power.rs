//! Software wake milestones. Optical first-light measurement remains external.
//! A cycle tag prevents a late frame from completing a later standby attempt.

use crate::power::State;
use alloc::{format, string::String};

/// The console transports 160-byte records including CRLF.
pub const STATUS_LINE_BYTES: usize = 158;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WakeTiming {
    pub cycle: u32,
    pub sleep_request_us: Option<u64>,
    pub wake_us: Option<u64>,
    pub restore_start_us: Option<u64>,
    pub panel_ready_us: Option<u64>,
    pub frame_done_us: Option<u64>,
    pub brightness_done_us: Option<u64>,
}

impl WakeTiming {
    pub const fn new() -> Self {
        Self {
            cycle: 0,
            sleep_request_us: None,
            wake_us: None,
            restore_start_us: None,
            panel_ready_us: None,
            frame_done_us: None,
            brightness_done_us: None,
        }
    }

    pub fn begin(&mut self, now_us: u64) {
        let cycle = self.cycle.wrapping_add(1).max(1);
        *self = Self {
            cycle,
            sleep_request_us: Some(now_us),
            ..Self::new()
        };
    }

    /// Keep the first observed wake, including an event during Sleep In.
    pub fn wake(&mut self, now_us: u64) {
        if self.cycle != 0 && self.wake_us.is_none() {
            self.wake_us = Some(now_us);
        }
    }

    pub fn restoring(&mut self, now_us: u64) {
        self.wake(now_us); // Deadline/internal recovery may have no input event.
        if self.cycle != 0 {
            self.restore_start_us = Some(now_us);
            self.panel_ready_us = None;
            self.frame_done_us = None;
            self.brightness_done_us = None;
        }
    }

    pub fn panel_ready(&mut self, cycle: u32, now_us: u64) {
        if cycle == self.cycle && self.restore_start_us.is_some() {
            self.panel_ready_us = Some(now_us);
        }
    }

    /// Called only after a complete, fresh full-frame transfer succeeds.
    pub fn frame_done(&mut self, cycle: u32, now_us: u64) {
        if cycle == self.cycle && self.panel_ready_us.is_some() && self.frame_done_us.is_none() {
            self.frame_done_us = Some(now_us);
        }
    }

    pub fn brightness_done(&mut self, cycle: u32, now_us: u64) -> bool {
        if cycle == self.cycle && self.frame_done_us.is_some() && self.brightness_done_us.is_none()
        {
            self.brightness_done_us = Some(now_us);
            return true;
        }
        false
    }

    pub fn format_status(&self, state: State, frame_ready: bool) -> String {
        let value = |v: Option<u64>| {
            v.map(|v| format!("{v}"))
                .unwrap_or_else(|| String::from("na"))
        };
        format!(
            "@power-wake cycle={} wake_us={} restore_us={}\n@power-panel cycle={} panel_us={} frame_us={} brightness_us={}\n@ok power-status state={} mcu=running mode=manual cycle={} frame_ready={} wake_to_ready_us={}",
            self.cycle, value(self.wake_us), value(self.restore_start_us),
            self.cycle, value(self.panel_ready_us), value(self.frame_done_us), value(self.brightness_done_us),
            state.name(), self.cycle, frame_ready as u8, value(self.wake_to_ready_us())
        )
    }

    pub fn complete(&self) -> bool {
        self.brightness_done_us.is_some()
    }

    pub fn wake_to_ready_us(&self) -> Option<u64> {
        self.brightness_done_us?.checked_sub(self.wake_us?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_ack_and_frame_alone_do_not_claim_complete_restore() {
        let mut t = WakeTiming::new();
        t.begin(100);
        t.wake(200);
        t.restoring(220);
        assert!(!t.brightness_done(t.cycle, 230));
        t.panel_ready(t.cycle, 1_200_220);
        assert!(!t.complete());
        t.frame_done(t.cycle, 1_250_000);
        assert!(!t.complete());
        assert!(t.brightness_done(t.cycle, 1_250_100));
        assert!(t.complete());
        assert_eq!(t.wake_to_ready_us(), Some(1_249_900));
    }

    #[test]
    fn repeated_input_preserves_first_wake_and_first_completed_frame() {
        let mut t = WakeTiming::new();
        t.begin(1);
        t.wake(2);
        t.wake(3);
        t.restoring(4);
        t.panel_ready(t.cycle, 5);
        t.frame_done(t.cycle, 6);
        t.frame_done(t.cycle, 7);
        t.brightness_done(t.cycle, 8);
        assert!(!t.brightness_done(t.cycle, 9));
        t.wake(10);
        assert_eq!(
            (t.wake_us, t.frame_done_us, t.brightness_done_us),
            (Some(2), Some(6), Some(8))
        );
    }

    #[test]
    fn new_cycle_rejects_old_frame_and_brightness_receipts() {
        let mut t = WakeTiming::new();
        t.begin(1);
        let old = t.cycle;
        t.restoring(2);
        t.panel_ready(old, 3);
        t.begin(4);
        t.restoring(5);
        t.panel_ready(old, 6);
        t.frame_done(old, 7);
        assert!(!t.brightness_done(old, 8));
        assert_eq!(t.panel_ready_us, None);
        t.panel_ready(t.cycle, 9);
        t.frame_done(old, 10);
        assert_eq!(t.frame_done_us, None);
        t.frame_done(t.cycle, 11);
        assert!(t.brightness_done(t.cycle, 12));
        assert_eq!(t.wake_to_ready_us(), Some(7));
    }

    #[test]
    fn retry_resets_completion_without_losing_initial_wake() {
        let mut t = WakeTiming::new();
        t.begin(1);
        t.restoring(2);
        t.panel_ready(t.cycle, 3);
        t.frame_done(t.cycle, 4);
        t.restoring(5);
        assert!(!t.brightness_done(t.cycle, 6));
        assert_eq!(t.wake_us, Some(2));
        assert_eq!(t.panel_ready_us, None);
        assert_eq!(t.frame_done_us, None);
    }
    #[test]
    fn status_records_fit_transport_even_with_full_width_timestamps() {
        for state in [
            State::Active,
            State::Entering,
            State::ScreenOff,
            State::Restoring,
            State::Fault,
        ] {
            for trace in [
                WakeTiming::new(),
                WakeTiming {
                    cycle: u32::MAX,
                    sleep_request_us: Some(u64::MAX),
                    wake_us: Some(0),
                    restore_start_us: Some(u64::MAX),
                    panel_ready_us: Some(u64::MAX),
                    frame_done_us: Some(u64::MAX),
                    brightness_done_us: Some(u64::MAX),
                },
            ] {
                let text = trace.format_status(state, trace.complete());
                assert_eq!(text.lines().count(), 3);
                assert!(text.lines().all(|line| line.len() <= STATUS_LINE_BYTES));
                assert!(text
                    .lines()
                    .last()
                    .unwrap()
                    .starts_with("@ok power-status "));
            }
        }
    }
}
