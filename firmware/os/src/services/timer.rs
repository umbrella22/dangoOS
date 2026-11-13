//! The timer app's countdown, kept by the hub so it keeps running when the
//! timer page is not on screen.
//!
//! Time is absolute milliseconds from the caller's clock: a long UI frame
//! does not slow the countdown down, unlike the capped animation step.

/// The preset durations, minutes.
pub const PRESETS_MIN: [u32; 5] = [1, 3, 5, 10, 25];
/// Preset selected at boot (5 minutes).
pub const DEFAULT_PRESET: usize = 2;

/// Countdown phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Full duration, not started.
    Ready,
    /// Counting down toward a deadline.
    Running,
    /// Stopped part way.
    Paused,
    /// Reached zero; stays here until reset or a new preset.
    Done,
}

/// A preset countdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Countdown {
    preset: usize,
    phase: Phase,
    /// Remaining time while `Ready` or `Paused`.
    remaining_ms: u64,
    /// Deadline while `Running`.
    deadline_ms: u64,
}

impl Default for Countdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Countdown {
    pub const fn new() -> Self {
        Self {
            preset: DEFAULT_PRESET,
            phase: Phase::Ready,
            remaining_ms: PRESETS_MIN[DEFAULT_PRESET] as u64 * 60_000,
            deadline_ms: 0,
        }
    }

    pub fn preset(&self) -> usize {
        self.preset
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn total_ms(&self) -> u64 {
        PRESETS_MIN[self.preset] as u64 * 60_000
    }

    /// Choose a preset; stops and resets the countdown.
    pub fn select(&mut self, preset: usize) {
        self.preset = preset.min(PRESETS_MIN.len() - 1);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.phase = Phase::Ready;
        self.remaining_ms = self.total_ms();
    }

    /// Start or resume, or pause a running countdown.
    pub fn toggle(&mut self, now_ms: u64) {
        match self.phase {
            Phase::Ready | Phase::Paused => {
                self.deadline_ms = now_ms + self.remaining_ms;
                self.phase = Phase::Running;
            }
            Phase::Running => {
                self.remaining_ms = self.deadline_ms.saturating_sub(now_ms);
                self.phase = Phase::Paused;
            }
            Phase::Done => {
                self.reset();
                self.deadline_ms = now_ms + self.remaining_ms;
                self.phase = Phase::Running;
            }
        }
    }

    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        match self.phase {
            Phase::Running => self.deadline_ms.saturating_sub(now_ms),
            Phase::Done => 0,
            Phase::Ready | Phase::Paused => self.remaining_ms,
        }
    }

    /// Advance; returns `true` exactly once, on the call that reaches zero.
    pub fn poll(&mut self, now_ms: u64) -> bool {
        if self.phase == Phase::Running && now_ms >= self.deadline_ms {
            self.phase = Phase::Done;
            self.remaining_ms = 0;
            return true;
        }
        false
    }

    /// Whole seconds shown on the dial, rounded up so the display reads
    /// `05:00` for the whole first second and `00:00` only when done.
    pub fn display_seconds(&self, now_ms: u64) -> u32 {
        self.remaining_ms(now_ms).div_ceil(1000) as u32
    }

    /// Progress remaining, per mille (1000 = full).
    pub fn remaining_permille(&self, now_ms: u64) -> u32 {
        let total = self.total_ms().max(1);
        (self.remaining_ms(now_ms) * 1000 / total) as u32
    }
}

/// `mm:ss`.
pub fn format_mm_ss(seconds: u32) -> heapless_text::Text<8> {
    let mut text = heapless_text::Text::new();
    let minutes = (seconds / 60).min(99);
    let seconds = seconds % 60;
    text.push_2(minutes);
    text.push(b':');
    text.push_2(seconds);
    text
}

/// A tiny fixed-capacity ASCII string, enough for clock digits.
pub mod heapless_text {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Text<const N: usize> {
        bytes: [u8; N],
        len: usize,
    }

    impl<const N: usize> Text<N> {
        pub const fn new() -> Self {
            Self {
                bytes: [0; N],
                len: 0,
            }
        }

        pub fn push(&mut self, byte: u8) {
            if self.len < N {
                self.bytes[self.len] = byte;
                self.len += 1;
            }
        }

        pub fn push_2(&mut self, value: u32) {
            self.push(b'0' + (value / 10 % 10) as u8);
            self.push(b'0' + (value % 10) as u8);
        }

        pub fn as_str(&self) -> &str {
            // Only ASCII digits and ':' are pushed.
            core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
        }
    }

    impl<const N: usize> Default for Text<N> {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_ready_at_the_default_preset() {
        let c = Countdown::new();
        assert_eq!(c.phase(), Phase::Ready);
        assert_eq!(c.display_seconds(0), 5 * 60);
        assert_eq!(format_mm_ss(c.display_seconds(0)).as_str(), "05:00");
    }

    #[test]
    fn it_counts_from_a_deadline_and_fires_once() {
        let mut c = Countdown::new();
        c.select(0); // 1 minute
        c.toggle(1_000);
        assert_eq!(c.phase(), Phase::Running);
        assert_eq!(c.display_seconds(1_000), 60);
        assert_eq!(c.display_seconds(1_001), 60, "rounds up");
        assert_eq!(c.display_seconds(31_000), 30);
        assert!(!c.poll(60_999));
        assert!(c.poll(61_000));
        assert!(!c.poll(70_000), "fires once");
        assert_eq!(c.phase(), Phase::Done);
        assert_eq!(format_mm_ss(c.display_seconds(70_000)).as_str(), "00:00");
    }

    #[test]
    fn pause_keeps_the_remaining_time() {
        let mut c = Countdown::new();
        c.select(0);
        c.toggle(0);
        c.toggle(20_000);
        assert_eq!(c.phase(), Phase::Paused);
        assert_eq!(c.remaining_ms(500_000), 40_000, "frozen while paused");
        c.toggle(500_000);
        assert!(c.poll(540_000));
    }

    #[test]
    fn a_new_preset_resets() {
        let mut c = Countdown::new();
        c.toggle(0);
        c.select(4);
        assert_eq!(c.phase(), Phase::Ready);
        assert_eq!(c.display_seconds(99_999), 25 * 60);
        assert_eq!(c.remaining_permille(0), 1000);
    }

    #[test]
    fn toggling_when_done_restarts() {
        let mut c = Countdown::new();
        c.select(0);
        c.toggle(0);
        assert!(c.poll(60_000));
        c.toggle(100_000);
        assert_eq!(c.phase(), Phase::Running);
        assert_eq!(c.display_seconds(100_000), 60);
    }

    #[test]
    fn progress_drains_to_zero() {
        let mut c = Countdown::new();
        c.select(0);
        c.toggle(0);
        assert_eq!(c.remaining_permille(30_000), 500);
        c.poll(60_000);
        assert_eq!(c.remaining_permille(60_000), 0);
    }
}
