//! Pure WS2812 lifecycle: a completed startup clear never acknowledges a
//! shutdown requested while that transfer was in flight.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    Startup,
    Shutdown,
}

pub struct Lifecycle {
    startup_cleared: bool,
    shutdown: bool,
    shutdown_cleared: bool,
}

impl Lifecycle {
    pub const fn new() -> Self {
        Self {
            startup_cleared: false,
            shutdown: false,
            shutdown_cleared: false,
        }
    }
    pub fn request_shutdown(&mut self) -> bool {
        let first = !self.shutdown;
        self.shutdown = true;
        first
    }
    pub fn shutting_down(&self) -> bool {
        self.shutdown
    }
    pub fn next_clear(&self) -> Option<Clear> {
        if self.shutdown {
            (!self.shutdown_cleared).then_some(Clear::Shutdown)
        } else {
            (!self.startup_cleared).then_some(Clear::Startup)
        }
    }
    /// Called only after a successful black TX and its latch interval.
    pub fn cleared(&mut self, attempt: Clear) {
        match attempt {
            Clear::Startup => self.startup_cleared = true,
            Clear::Shutdown if self.shutdown => self.shutdown_cleared = true,
            Clear::Shutdown => {}
        }
    }
    pub fn allows_color(&self) -> bool {
        self.startup_cleared && !self.shutdown
    }
    pub fn shutdown_cleared(&self) -> bool {
        self.shutdown && self.shutdown_cleared
    }
}

#[cfg(test)]
#[path = "../../power/buttons.rs"]
mod buttons;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cutoff_waits_for_both_sleep_interval_and_fresh_clear() {
        let mut light = Lifecycle::new();
        light.cleared(Clear::Startup);
        let mut power = super::buttons::Shutdown::Idle;
        power.request(0);
        light.request_shutdown();
        assert!(!power.poll(10_000, light.shutdown_cleared()));
        power.uploaded(10_000, true, true);
        // Repeated failed or absent clear attempts cannot advance power to Cut.
        for now in [11_500, 20_000, 1_000_000] {
            assert!(!power.poll(now, light.shutdown_cleared()));
        }
        light.cleared(Clear::Shutdown);
        assert!(power.poll(1_000_001, light.shutdown_cleared()));
        assert!(!power.poll(1_000_002, light.shutdown_cleared()));
        let mut early = super::buttons::Shutdown::Idle;
        early.request(0);
        early.uploaded(100, true, true);
        assert!(!early.poll(1599, light.shutdown_cleared()));
        assert!(early.poll(1600, light.shutdown_cleared()));
    }
    #[test]
    fn startup_failure_retries_and_color_waits_for_success() {
        let mut p = Lifecycle::new();
        assert!(!p.allows_color());
        for _ in 0..5 {
            assert_eq!(p.next_clear(), Some(Clear::Startup));
        }
        p.cleared(Clear::Startup);
        assert!(p.allows_color());
        assert_eq!(p.next_clear(), None);
        assert!(!p.shutdown_cleared());
    }
    #[test]
    fn shutdown_requires_a_new_clear_and_never_reopens_color() {
        let mut p = Lifecycle::new();
        p.cleared(Clear::Startup);
        assert!(p.request_shutdown());
        assert!(!p.allows_color());
        for _ in 0..5 {
            assert!(!p.request_shutdown());
            assert!(!p.shutdown_cleared());
            assert_eq!(p.next_clear(), Some(Clear::Shutdown));
        }
        p.cleared(Clear::Shutdown);
        assert!(p.shutdown_cleared());
        assert_eq!(p.next_clear(), None);
        assert!(!p.request_shutdown());
        assert!(!p.allows_color());
        assert!(p.shutdown_cleared());
    }
    #[test]
    fn in_flight_startup_clear_cannot_acknowledge_shutdown() {
        let mut p = Lifecycle::new();
        let in_flight = p.next_clear().unwrap();
        p.request_shutdown();
        p.cleared(in_flight);
        assert!(!p.shutdown_cleared());
        assert!(!p.allows_color());
        assert_eq!(p.next_clear(), Some(Clear::Shutdown));
        p.cleared(Clear::Shutdown);
        assert!(p.shutdown_cleared());
    }
}
