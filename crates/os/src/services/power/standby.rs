//! Screen standby policy, independent of the panel or MCU sleep backend.
//! A bounded screen-off request leaves the CPU running.

/// A manual screen-off request expires within a minute.
pub const MAX_SCREEN_OFF_SECONDS: u8 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    ScreenOff(u8),
    ScreenOn,
}

impl Command {
    pub fn parse(verb: &str, args: &[&str]) -> Result<Self, &'static str> {
        match (verb, args) {
            ("power-status", []) => Ok(Self::Status),
            ("screen-on", []) => Ok(Self::ScreenOn),
            ("screen-off", [seconds]) if seconds.bytes().all(|b| b.is_ascii_digit()) => {
                match seconds.parse::<u8>() {
                    Ok(n) if (1..=MAX_SCREEN_OFF_SECONDS).contains(&n) => Ok(Self::ScreenOff(n)),
                    _ => Err("screen-off expects 1..60 seconds"),
                }
            }
            _ => Err("expected power-status, screen-on, or screen-off <1..60>"),
        }
    }

    pub const fn verb(self) -> &'static str {
        match self {
            Self::Status => "power-status",
            Self::ScreenOff(_) => "screen-off",
            Self::ScreenOn => "screen-on",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Active,
    Entering,
    ScreenOff,
    Restoring,
    Fault,
}
impl State {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Entering => "entering",
            Self::ScreenOff => "screen-off",
            Self::Restoring => "restoring",
            Self::Fault => "fault",
        }
    }
}

pub struct Policy {
    state: State,
    deadline_ms: u64,
    wake_epoch: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self::new()
    }
}
impl Policy {
    pub const fn new() -> Self {
        Self {
            state: State::Active,
            deadline_ms: 0,
            wake_epoch: 0,
        }
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn request(
        &mut self,
        seconds: u8,
        now_ms: u64,
        wake_epoch: u32,
    ) -> Result<(), &'static str> {
        if self.state != State::Active {
            return Err("display not active");
        }
        if !(1..=MAX_SCREEN_OFF_SECONDS).contains(&seconds) {
            return Err("screen-off expects 1..60 seconds");
        }
        self.deadline_ms = now_ms.saturating_add(u64::from(seconds) * 1000);
        self.wake_epoch = wake_epoch;
        self.state = State::Entering;
        Ok(())
    }
    /// Entry failure leaves hardware state uncertain: always attempt Sleep Out.
    pub fn entered(&mut self, success: bool, now_ms: u64, wake_epoch: u32) {
        if self.state != State::Entering {
            return;
        }
        self.state = if success {
            State::ScreenOff
        } else {
            State::Restoring
        };
        self.poll(now_ms, wake_epoch);
    }
    pub fn poll(&mut self, now_ms: u64, wake_epoch: u32) {
        if matches!(self.state, State::Entering | State::ScreenOff)
            && (now_ms >= self.deadline_ms || wake_epoch != self.wake_epoch)
        {
            self.state = State::Restoring;
        }
    }
    pub fn wake(&mut self) {
        if self.state != State::Active {
            self.state = State::Restoring;
        }
    }
    pub fn restored(&mut self, success: bool) {
        self.state = if success { State::Active } else { State::Fault };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wake_during_entry_is_not_lost_by_a_successful_ack() {
        let mut p = Policy::new();
        p.request(30, 100, 7).unwrap();
        p.entered(true, 400, 8);
        assert_eq!(p.state(), State::Restoring);
        p.restored(true);
        assert_eq!(p.state(), State::Active);
    }

    #[test]
    fn explicit_cancellation_survives_a_late_entry_ack() {
        let mut p = Policy::new();
        p.request(30, 0, 5).unwrap();
        p.wake();
        p.entered(true, 100, 5);
        assert_eq!(p.state(), State::Restoring);
    }
    #[test]
    fn deadline_includes_entry_time_and_is_not_extended_by_polling() {
        let mut p = Policy::new();
        p.request(1, 100, 0).unwrap();
        p.entered(true, 200, 0);
        p.poll(1099, 0);
        assert_eq!(p.state(), State::ScreenOff);
        p.poll(1100, 0);
        assert_eq!(p.state(), State::Restoring);
    }
    #[test]
    fn failed_entry_and_restore_never_report_active_or_asleep() {
        let mut p = Policy::new();
        p.request(5, 0, 0).unwrap();
        p.entered(false, 200, 0);
        assert_eq!(p.state(), State::Restoring);
        p.restored(false);
        assert_eq!(p.state(), State::Fault);
        assert!(p.request(5, 300, 0).is_err());
        p.wake();
        p.restored(true);
        assert_eq!(p.state(), State::Active);
    }
    #[test]
    fn no_reentry_and_explicit_wake_recover_a_fault() {
        let mut p = Policy::new();
        p.request(30, 0, u32::MAX).unwrap();
        assert!(p.request(30, 1, u32::MAX).is_err());
        p.entered(true, 100, 0);
        assert_eq!(p.state(), State::Restoring);
    }
    #[test]
    fn screen_off_rejects_durations_outside_the_supported_range() {
        for s in ["0", "61", "256", "-1", "+5", "", "1.0"] {
            assert!(Command::parse("screen-off", &[s]).is_err());
        }
        assert_eq!(
            Command::parse("screen-off", &["60"]),
            Ok(Command::ScreenOff(60))
        );
        assert_eq!(
            Command::parse("screen-off", &["01"]),
            Ok(Command::ScreenOff(1))
        );
        assert!(Command::parse("screen-on", &["extra"]).is_err());
    }
}
