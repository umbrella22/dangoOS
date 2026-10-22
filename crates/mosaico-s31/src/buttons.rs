//! Host-testable onboard-button timelines and one-way power-off sequencing.
//! All times are monotonic milliseconds; GPIO sampling belongs to the task.

pub const DEBOUNCE_MS: u64 = 20;
pub const CLICK_GAP_MS: u64 = 180;
pub const LONG_MS: u64 = 1500;
pub const SLEEP_VISIBLE_MS: u64 = 1500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Short,
    Double,
    Long,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub edge: Option<bool>,
    pub gesture: Option<Gesture>,
}

/// Suppresses an initial held key until a stable release, debounces both edges,
/// delays a single until the click window expires, and never clicks after long.
pub struct Button {
    candidate: bool,
    candidate_at: u64,
    pressed: bool,
    armed: bool,
    down_at: Option<u64>,
    long_sent: bool,
    clicks: u8,
    released_at: Option<u64>,
}
impl Button {
    pub fn new(now: u64, pressed: bool) -> Self {
        Self {
            candidate: pressed,
            candidate_at: now,
            pressed,
            armed: false,
            down_at: None,
            long_sent: false,
            clicks: 0,
            released_at: None,
        }
    }
    pub fn pressed(&self) -> bool {
        self.pressed
    }
    pub fn sample(&mut self, now: u64, pressed: bool) -> Update {
        let mut out = Update::default();
        if pressed != self.candidate {
            self.candidate = pressed;
            self.candidate_at = now;
        }
        if now.saturating_sub(self.candidate_at) >= DEBOUNCE_MS {
            if self.pressed != self.candidate {
                self.pressed = self.candidate;
                out.edge = Some(self.pressed);
                if self.armed {
                    if self.pressed {
                        self.down_at = Some(now);
                        self.long_sent = false;
                    } else if self.long_sent {
                        self.down_at = None;
                    } else {
                        self.clicks = self.clicks.saturating_add(1).min(2);
                        self.released_at = Some(now);
                        self.down_at = None;
                    }
                }
            }
            if !self.pressed {
                self.armed = true;
            }
        }
        if let Some(at) = self.down_at {
            if self.pressed && !self.long_sent && now.saturating_sub(at) >= LONG_MS {
                self.long_sent = true;
                self.clicks = 0;
                self.released_at = None;
                out.gesture = Some(Gesture::Long);
            }
        } else if let Some(at) = self.released_at {
            if now.saturating_sub(at) >= CLICK_GAP_MS {
                out.gesture = Some(if self.clicks == 1 {
                    Gesture::Short
                } else {
                    Gesture::Double
                });
                self.clicks = 0;
                self.released_at = None;
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shutdown {
    Idle,
    WaitingForSleep { requested_at: u64 },
    ShowingSleep { since: u64 },
    Cut,
}
impl Shutdown {
    /// Latches once; release, subsequent gestures and repeated requests cannot restart it.
    pub fn request(&mut self, now: u64) -> bool {
        if *self != Self::Idle {
            return false;
        }
        *self = Self::WaitingForSleep { requested_at: now };
        true
    }
    pub fn requested(&self) -> bool {
        *self != Self::Idle
    }
    /// Plane completion receipt: stale, unrelated or failed uploads never arm
    /// the cut timer. Repeated successful frames do not restart that timer.
    pub fn uploaded(&mut self, now: u64, sleep_frame: bool, completed: bool) {
        if sleep_frame && completed && matches!(self, Self::WaitingForSleep { .. }) {
            *self = Self::ShowingSleep { since: now };
        }
    }
    /// Exactly one low assertion, at least 1500 ms after the first actual upload.
    /// No frame acknowledgement means no cut, even if rendering stalls forever.
    pub fn poll(&mut self, now: u64) -> bool {
        if let Self::ShowingSleep { since } = *self {
            if now.saturating_sub(since) >= SLEEP_VISIBLE_MS {
                *self = Self::Cut;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn armed() -> Button {
        let mut b = Button::new(0, false);
        b.sample(20, false);
        b
    }
    fn edge(b: &mut Button, at: u64, pressed: bool) -> Update {
        b.sample(at, pressed);
        b.sample(at + DEBOUNCE_MS, pressed)
    }
    #[test]
    fn bounce_is_not_a_click_and_single_waits_for_double_window() {
        let mut b = armed();
        assert_eq!(b.sample(30, true), Update::default());
        b.sample(35, false);
        assert_eq!(b.sample(55, false), Update::default());
        assert_eq!(edge(&mut b, 100, true).edge, Some(true));
        assert_eq!(edge(&mut b, 150, false).edge, Some(false));
        assert_eq!(b.sample(349, false).gesture, None);
        assert_eq!(b.sample(350, false).gesture, Some(Gesture::Short));
        assert_eq!(b.sample(900, false).gesture, None);
    }
    #[test]
    fn double_has_no_single_and_long_cancels_prior_click() {
        let mut b = armed();
        edge(&mut b, 100, true);
        edge(&mut b, 140, false);
        edge(&mut b, 200, true);
        edge(&mut b, 240, false);
        assert_eq!(b.sample(440, false).gesture, Some(Gesture::Double));
        edge(&mut b, 500, true);
        edge(&mut b, 550, false);
        edge(&mut b, 600, true);
        assert_eq!(b.sample(2119, true).gesture, None);
        assert_eq!(b.sample(2120, true).gesture, Some(Gesture::Long));
        assert_eq!(b.sample(5000, true).gesture, None);
        edge(&mut b, 5100, false);
        assert_eq!(b.sample(5500, false).gesture, None);
    }
    #[test]
    fn boot_held_and_release_bounce_do_not_invent_gestures() {
        let mut b = Button::new(0, true);
        assert_eq!(b.sample(5000, true).gesture, None);
        b.sample(5100, false);
        b.sample(5110, true);
        b.sample(6000, true);
        assert_eq!(edge(&mut b, 6100, false).gesture, None);
        assert_eq!(b.sample(7000, false).gesture, None);
        edge(&mut b, 7100, true);
        assert_eq!(b.sample(8620, true).gesture, Some(Gesture::Long));
    }
    #[test]
    fn cancelled_hold_does_not_shutdown_and_triple_is_bounded() {
        let mut b = armed();
        edge(&mut b, 100, true);
        edge(&mut b, 1500, false);
        assert_eq!(b.sample(1700, false).gesture, Some(Gesture::Short));
        for at in [2000, 2100, 2200] {
            edge(&mut b, at, true);
            edge(&mut b, at + 30, false);
        }
        assert_eq!(b.sample(2450, false).gesture, Some(Gesture::Double));
        assert_eq!(b.sample(3000, false).gesture, None);
    }
    #[test]
    fn shutdown_waits_for_successful_sleep_upload_and_cuts_once() {
        let mut s = Shutdown::Idle;
        s.uploaded(1, true, true);
        assert!(!s.poll(9000));
        assert!(s.request(10000));
        assert!(!s.request(11000));
        assert!(!s.poll(999999)); // no display receipt
        s.uploaded(999999, true, false);
        s.uploaded(999999, false, true);
        assert!(!s.poll(999999));
        s.uploaded(1_000_000, true, true);
        s.uploaded(1_000_100, true, true);
        assert!(!s.poll(1_001_499));
        assert!(s.poll(1_001_500));
        assert!(!s.poll(2_000_000));
        assert!(!s.request(3_000_000));
    }
}
