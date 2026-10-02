//! One-shot startup timing shared by the desktop and embedded platforms.

use core::time::Duration;

pub const DURATION: Duration = Duration::from_millis(1000);
pub const FRAME_INTERVAL: Duration = Duration::from_millis(67);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BootFrame {
    Progress(f32),
    Finished,
}

/// Emits at most one update per interval and skips overdue frames.
#[derive(Default)]
pub struct BootAnimation {
    last_step: Option<u32>,
    finished: bool,
}

impl BootAnimation {
    pub fn advance(&mut self, elapsed: Duration) -> Option<BootFrame> {
        if self.finished {
            return None;
        }
        if elapsed >= DURATION {
            self.finished = true;
            return Some(BootFrame::Finished);
        }

        let step = (elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as u32;
        if self.last_step.is_some_and(|last| step <= last) {
            return None;
        }
        self.last_step = Some(step);
        let progress =
            (step as u128 * FRAME_INTERVAL.as_millis()) as f32 / DURATION.as_millis() as f32;
        Some(BootFrame::Progress(progress))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero_and_limits_updates_between_frames() {
        let mut animation = BootAnimation::default();
        assert_eq!(
            animation.advance(Duration::ZERO),
            Some(BootFrame::Progress(0.0))
        );
        assert_eq!(animation.advance(Duration::from_millis(20)), None);
        assert_eq!(animation.advance(Duration::from_millis(60)), None);
        assert!(
            matches!(animation.advance(FRAME_INTERVAL), Some(BootFrame::Progress(p)) if p > 0.0 && p < 0.1)
        );
    }

    #[test]
    fn slow_rendering_skips_to_current_time_without_replaying_frames() {
        let mut animation = BootAnimation::default();
        animation.advance(Duration::ZERO);
        assert!(
            matches!(animation.advance(Duration::from_millis(800)), Some(BootFrame::Progress(p)) if (0.7..=0.8).contains(&p))
        );
        assert_eq!(animation.advance(Duration::from_millis(800)), None);
        assert_eq!(animation.advance(DURATION), Some(BootFrame::Finished));
    }

    #[test]
    fn delayed_start_finishes_once_and_cannot_restart() {
        let mut animation = BootAnimation::default();
        assert_eq!(
            animation.advance(Duration::from_secs(3)),
            Some(BootFrame::Finished)
        );
        assert_eq!(animation.advance(Duration::ZERO), None);
        assert_eq!(animation.advance(Duration::from_secs(4)), None);
    }

    #[test]
    fn frequent_polling_keeps_a_bounded_monotonic_frame_count() {
        let mut animation = BootAnimation::default();
        let mut frames = 0;
        let mut finishes = 0;
        let mut previous = 0.0;
        for ms in 0..=2000 {
            match animation.advance(Duration::from_millis(ms)) {
                Some(BootFrame::Progress(progress)) => {
                    assert!((previous..=1.0).contains(&progress));
                    previous = progress;
                    frames += 1;
                }
                Some(BootFrame::Finished) => finishes += 1,
                None => {}
            }
        }
        assert!((10..=16).contains(&frames));
        assert_eq!(finishes, 1);
    }
}
