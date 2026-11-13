//! Bounded recovery of an ACKing slot whose full descriptor transfer failed.
//! Tick once per 250 ms scan: nominal 1/2/4 s backoff, not wall deadlines.
//! Only ACKing scans spend the countdown; there is no catch-up burst.

pub const MAX_RETRIES: u8 = 3;
const FIRST_WAIT_SCANS: u8 = 4;

pub struct DescriptorRetry {
    retries: u8,
    wait: Option<u8>,
}

impl DescriptorRetry {
    pub const fn new() -> Self {
        Self {
            retries: 0,
            wait: None,
        }
    }

    /// Arm the next transfer retry, or park until removal after three retries.
    pub fn read_failed(&mut self) -> bool {
        self.wait = (self.retries < MAX_RETRIES).then(|| FIRST_WAIT_SCANS << self.retries);
        self.wait.is_some()
    }

    /// Return each retry number once. Never trigger a transfer without ACK.
    pub fn scan(&mut self, ack: bool) -> Option<u8> {
        if !ack {
            return None;
        }
        let wait = self.wait.as_mut()?;
        *wait -= 1;
        if *wait != 0 {
            return None;
        }
        self.wait = None;
        self.retries += 1;
        Some(self.retries)
    }

    /// Successful identification or removal starts a fresh insertion budget.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_failure_means_no_retry() {
        let mut retry = DescriptorRetry::new();
        for _ in 0..100 {
            assert_eq!(retry.scan(true), None);
        }
    }

    #[test]
    fn failures_back_off_one_two_four_seconds_then_stop() {
        let mut retry = DescriptorRetry::new();
        for (number, scans) in [(1, 4), (2, 8), (3, 16)] {
            assert!(retry.read_failed());
            for _ in 1..scans {
                assert_eq!(retry.scan(true), None);
            }
            assert_eq!(retry.scan(true), Some(number));
        }
        assert!(!retry.read_failed());
        for _ in 0..100 {
            assert_eq!(retry.scan(true), None);
        }
    }

    #[test]
    fn missing_ack_never_starts_a_descriptor_transfer() {
        let mut retry = DescriptorRetry::new();
        retry.read_failed();
        for _ in 0..20 {
            assert_eq!(retry.scan(false), None);
        }
        for _ in 0..3 {
            assert_eq!(retry.scan(true), None);
        }
        assert_eq!(retry.scan(true), Some(1));
    }

    #[test]
    fn due_once_and_success_cancels_future_attempts() {
        let mut retry = DescriptorRetry::new();
        retry.read_failed();
        for _ in 0..4 {
            retry.scan(true);
        }
        for _ in 0..10 {
            assert_eq!(retry.scan(true), None);
        }
        retry.reset();
        for _ in 0..100 {
            assert_eq!(retry.scan(true), None);
        }
    }

    #[test]
    fn removal_discards_backoff_and_new_insert_gets_full_budget() {
        let mut retry = DescriptorRetry::new();
        retry.read_failed();
        for _ in 0..4 {
            retry.scan(true);
        }
        retry.read_failed();
        retry.scan(true);
        retry.reset();
        retry.read_failed();
        for _ in 0..3 {
            assert_eq!(retry.scan(true), None);
        }
        assert_eq!(retry.scan(true), Some(1));
    }
}
