//! Main-image health and handoff to the independently bootable installer.
pub use boot_contract::{image, State};

/// A candidate must keep producing healthy UI-loop observations for 30 seconds.
#[derive(Default)]
pub struct HealthWindow {
    last: Option<u64>,
    since: Option<u64>,
}
impl HealthWindow {
    pub const fn new() -> Self {
        Self {
            last: None,
            since: None,
        }
    }
    pub fn observe(&mut self, now_ms: u64, healthy: bool) -> bool {
        if !healthy {
            self.last = None;
            self.since = None;
            return false;
        }
        if self
            .last
            .is_none_or(|last| now_ms < last || now_ms - last > 1000)
        {
            self.since = Some(now_ms);
        }
        self.last = Some(now_ms);
        now_ms.saturating_sub(self.since.unwrap_or(now_ms)) >= 30_000
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebootAck {
    Waiting,
    Sent,
    TimedOut,
}
pub fn reboot_ack(sent: bool, elapsed_ms: u64) -> RebootAck {
    if sent {
        RebootAck::Sent
    } else if elapsed_ms >= 3000 {
        RebootAck::TimedOut
    } else {
        RebootAck::Waiting
    }
}

#[cfg(target_os = "none")]
pub(crate) mod device;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn health_requires_continuity_and_restarts_after_clock_or_frame_gaps() {
        let mut h = HealthWindow::new();
        for n in 0..30 {
            assert!(!h.observe(n * 1000, true));
        }
        assert!(h.observe(30_000, true));
        assert!(!h.observe(31_001, true));
        assert!(!h.observe(1, true));
        assert!(!h.observe(2, false));
        for n in 0..30 {
            assert!(!h.observe(100_000 + n * 1000, true));
        }
        assert!(h.observe(130_000, true));
        assert_eq!(reboot_ack(false, 2999), RebootAck::Waiting);
        assert_eq!(reboot_ack(false, 3000), RebootAck::TimedOut);
        assert_eq!(reboot_ack(true, 3001), RebootAck::Sent);
    }
}
