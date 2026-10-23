//! Scheduler admission and cancellation, independent of the motor peripheral.

#[derive(Clone, Copy, Debug)]
pub struct Request<T> {
    pub pattern: T,
    pub priority: u8,
    epoch: u64,
}

pub struct Policy<T> {
    enabled: bool,
    epoch: u64,
    pending: Option<Request<T>>,
    busy: bool,
    faulted: bool,
}

impl<T: Copy> Policy<T> {
    pub const fn new() -> Self {
        Self {
            enabled: true,
            epoch: 0,
            pending: None,
            busy: false,
            faulted: false,
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.epoch = self.epoch.wrapping_add(1);
            self.pending = None;
        }
    }
    pub fn post(&mut self, pattern: T, priority: u8) -> bool {
        if !self.enabled || self.faulted || self.pending.is_some_and(|p| p.priority > priority) {
            return false;
        }
        self.pending = Some(Request {
            pattern,
            priority,
            epoch: self.epoch,
        });
        true
    }
    pub fn take(&mut self) -> Option<Request<T>> {
        let request = self.pending.take()?;
        self.busy = true;
        Some(request)
    }
    pub fn peek(&self) -> Option<Request<T>> {
        self.pending
    }
    pub fn valid(&self, request: Request<T>) -> bool {
        self.enabled && !self.faulted && request.epoch == self.epoch
    }
    /// Called after selected work unwinds and any transaction attempts stop.
    /// A preceding failure remains latched and cannot acknowledge silence.
    pub fn finish(&mut self) {
        self.busy = false;
    }
    /// A failed RMT start/stop leaves output state uncertain. Refuse new work
    /// and silence receipts until reboot rather than claiming successful stop.
    pub fn failed(&mut self) {
        self.faulted = true;
        self.pending = None;
    }
    pub fn quiescent(&self) -> bool {
        !self.enabled && !self.busy && !self.faulted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disable_clears_pending_and_rejects_all_priorities() {
        let mut p = Policy::new();
        assert!(p.post("angry", 1));
        p.set_enabled(false);
        assert!(p.take().is_none());
        assert!(!p.post("error", 3));
        assert!(p.quiescent());
    }
    #[test]
    fn off_on_invalidates_selected_request_epoch_but_preserves_fresh_pending() {
        let mut p = Policy::new();
        p.post("old", 1);
        let old = p.take().unwrap();
        p.set_enabled(false);
        assert!(!p.quiescent(), "selected work has not acknowledged stop");
        p.set_enabled(true);
        assert!(!p.valid(old));
        // New work arriving while the task unwinds cancellation must survive
        // the old request's stop acknowledgement.
        assert!(p.post("new", 1));
        p.finish();
        let new = p.take().unwrap();
        assert!(p.valid(new));
        assert_eq!(new.pattern, "new");
    }
    #[test]
    fn quiescence_requires_stop_receipt_and_failure_is_not_silence() {
        let mut p = Policy::new();
        p.post(1, 1);
        p.take();
        p.set_enabled(false);
        assert!(!p.quiescent());
        p.finish();
        assert!(p.quiescent());
        let mut faulted = Policy::new();
        faulted.post(1, 1);
        faulted.take();
        faulted.set_enabled(false);
        faulted.failed();
        // The scheduler finishes unwinding after the peripheral reports failure.
        faulted.finish();
        assert!(!faulted.quiescent());
        faulted.set_enabled(true);
        assert!(!faulted.post(2, 3));
    }
    #[test]
    fn pending_priority_and_equal_priority_replacement_are_preserved() {
        let mut p = Policy::new();
        assert!(p.post("error", 3));
        assert!(!p.post("angry", 1));
        assert!(p.post("new-error", 3));
        assert_eq!(p.take().unwrap().pattern, "new-error");
    }
}
