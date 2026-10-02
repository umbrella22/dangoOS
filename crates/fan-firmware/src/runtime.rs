//! Scheduling and retry policy, independent of the MCU clock and UI runtime.

#[path = "screen_contact.rs"]
pub mod screen_contact;

pub const GAME_STEP_US: u64 = 33_333;
pub const IMU_SAMPLE_US: u64 = 100_000;

pub fn app_active(app: i32, expected: i32, locked: bool, drawer_open: bool) -> bool {
    app == expected && !locked && !drawer_open
}

/// A contact belongs to the game only if it started there. Overlay/navigation
/// changes revoke ownership until release, including a gesture that unlocks.
#[derive(Default)]
pub struct GameContact {
    owned: bool,
}
impl GameContact {
    pub fn begin(&mut self, game_active: bool, y: f32) {
        self.owned = game_active && y >= 56.0;
    }
    pub fn update(&mut self, game_active: bool) {
        if !game_active {
            self.owned = false;
        }
    }
    pub fn end(&mut self) {
        self.owned = false;
    }
    pub fn owned(&self) -> bool {
        self.owned
    }
}

/// Fixed steps use elapsed monotonic time. Pauses discard their elapsed time;
/// a long stall is capped to four steps so input/rendering cannot be starved.
pub struct Periodic {
    period_us: u64,
    next_us: Option<u64>,
}
impl Periodic {
    pub fn new(period_us: u64) -> Self {
        Self {
            period_us,
            next_us: None,
        }
    }
    pub fn due(&mut self, now_us: u64, active: bool) -> u32 {
        if !active {
            self.next_us = None;
            return 0;
        }
        let Some(next) = self.next_us else {
            self.next_us = Some(now_us.saturating_add(self.period_us));
            return 0;
        };
        if now_us < next {
            return 0;
        }
        let elapsed = 1 + (now_us - next) / self.period_us;
        self.next_us = Some(next.saturating_add(elapsed.saturating_mul(self.period_us)));
        elapsed.min(4) as u32
    }
}

/// A failed publication retains the existing rendered frame for a full retry,
/// even if Slint has consumed all of its dirty regions.
#[derive(Default)]
pub struct Publication {
    pending: bool,
}
impl Publication {
    pub fn rendered(&mut self) {
        self.pending = true;
    }
    pub fn pending(&self) -> bool {
        self.pending
    }
    pub fn completed(&mut self, success: bool) {
        self.pending = !success;
    }
}

#[derive(Default)]
pub struct Brightness {
    applied: Option<u8>,
}
impl Brightness {
    pub fn requested(&self, value: i32) -> Option<u8> {
        let value = value.clamp(0, 100) as u8;
        (self.applied != Some(value)).then_some(value)
    }
    pub fn completed(&mut self, value: u8, success: bool) {
        if success {
            self.applied = Some(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unlocking_gesture_never_becomes_paddle_input() {
        let mut touch = GameContact::default();
        touch.begin(false, 400.0);
        touch.update(true);
        assert!(!touch.owned());
        touch.end();
        touch.begin(true, 300.0);
        assert!(touch.owned());
        touch.update(false);
        touch.update(true);
        assert!(!touch.owned());
        touch.end();
        touch.begin(true, 20.0);
        assert!(!touch.owned());
    }
    #[test]
    fn elapsed_time_not_loop_count() {
        let mut tick = Periodic::new(GAME_STEP_US);
        assert_eq!(tick.due(0, true), 0);
        for now in [1, 8000, 16000, 24000, 32000] {
            assert_eq!(tick.due(now, true), 0);
        }
        assert_eq!(tick.due(33333, true), 1);
        assert_eq!(tick.due(100000, true), 2);
    }
    #[test]
    fn pause_does_not_accumulate_steps() {
        let mut tick = Periodic::new(GAME_STEP_US);
        tick.due(0, true);
        assert_eq!(tick.due(30000, false), 0);
        assert_eq!(tick.due(10_000_000, true), 0);
        assert_eq!(tick.due(10_033_333, true), 1);
        assert!(!app_active(3, 3, true, false));
        assert!(!app_active(3, 3, false, true));
        assert!(!app_active(-1, 3, false, false));
    }
    #[test]
    fn imu_samples_100ms_and_stalls_are_bounded() {
        let mut tick = Periodic::new(IMU_SAMPLE_US);
        tick.due(0, true);
        assert_eq!(tick.due(99999, true), 0);
        assert_eq!(tick.due(100000, true), 1);
        assert_eq!(tick.due(10_000_000, true), 4);
        assert_eq!(tick.due(10_000_000, true), 0);
    }
    #[test]
    fn failed_frame_retries_without_new_render() {
        let mut p = Publication::default();
        p.rendered();
        p.completed(false);
        assert!(p.pending());
        p.completed(true);
        assert!(!p.pending());
    }
    #[test]
    fn brightness_retries_failures_and_suppresses_redundant_writes() {
        let mut b = Brightness::default();
        assert_eq!(b.requested(70), Some(70));
        b.completed(70, false);
        assert_eq!(b.requested(70), Some(70));
        b.completed(70, true);
        assert_eq!(b.requested(70), None);
        assert_eq!(b.requested(500), Some(100));
    }
    #[test]
    fn rounded_viewport_retains_the_contact_origin() {
        use super::screen_contact::{contains, ScreenContact};
        assert!(!contains(0.0, 0.0, 60.0));
        assert!(!contains(479.0, 479.0, 60.0));
        assert!(!contains(16.0, 16.0, 60.0));
        assert!(contains(40.0, 40.0, 60.0));
        assert!(contains(36.0, 40.0, 60.0)); // Camera back glyph.
        assert!(contains(60.0, 0.0, 60.0));
        assert!(contains(0.0, 60.0, 60.0));
        assert!(contains(0.0, 0.0, 0.0));
        assert!(!contains(480.0, 240.0, 0.0));
        let mut contact = ScreenContact::default();
        assert!(!contact.sample(5.0, 5.0, 60.0));
        assert!(!contact.sample(240.0, 240.0, 60.0));
        assert!(!contact.release());
        assert!(contact.sample(40.0, 40.0, 60.0));
        assert!(contact.sample(5.0, 5.0, 60.0));
        assert!(contact.release());
        assert!(!contact.sample(5.0, 5.0, 60.0));
        contact.transport_error();
        assert!(!contact.sample(40.0, 40.0, 60.0));
        assert!(!contact.release());
        assert!(contact.sample(40.0, 40.0, 60.0));
        contact.transport_error();
        assert!(contact.sample(5.0, 5.0, 60.0));
        assert!(contact.release());
    }
}
