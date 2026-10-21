//! The fox's entrance after boot.
//!
//! The fox drops in from above, lands with a squash, and settles with a small
//! hop. The animation is a pure function of elapsed milliseconds, so the host
//! tests pin exact poses and the widget only has to feed the clock.
//!
//! The panel plane's rectangle is fixed, so the drop is drawn inside the
//! canvas: [`EntrancePose::offset_y`] and the scales become the renderer's
//! warp (see `FoxInput`), not a moved LVGL object.
//!
//! # Why these numbers
//!
//! - The fall is an ease-in quad: slow release, fast impact.
//! - The landing squash is a damped two-cycle cosine: squashed on impact,
//!   overshoot, settle. It is the detail that makes the drop read as weight
//!   rather than a slide.
//! - The hop is short (280 ms); it releases the tension without delaying the
//!   first interaction.
//!
//! The platform passes [`TOTAL_MS`] to the personality engine's hold-off, so a
//! finger on the panel during the entrance cannot interrupt it.

use super::math::{cos, round, sin, PI};

/// Offset above the rest position at the start, in canvas pixels.
pub const DROP_HEIGHT_PX: i32 = 40;
/// Fall duration.
pub const FALL_MS: u64 = 220;
/// Squash and settle duration.
pub const SETTLE_MS: u64 = 240;
/// Peak impact squash: the height is `1 - IMPACT_SQUASH` at touchdown.
pub const IMPACT_SQUASH: f32 = 0.20;
/// Small hop after the landing.
pub const HOP_MS: u64 = 280;
/// Hop height in canvas pixels.
pub const HOP_HEIGHT_PX: f32 = 30.0;
/// Total entrance duration.
pub const TOTAL_MS: u64 = FALL_MS + SETTLE_MS + HOP_MS;

/// One entrance frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntrancePose {
    /// Vertical offset from the rest position, in canvas pixels.
    pub offset_y: i32,
    /// Horizontal and vertical scale multipliers (`1.0` is normal).
    pub scale_x: f32,
    pub scale_y: f32,
    /// `true` on the last frame.
    pub finished: bool,
}

impl EntrancePose {
    /// The resting pose.
    pub const REST: Self = Self {
        offset_y: 0,
        scale_x: 1.0,
        scale_y: 1.0,
        finished: true,
    };
}

/// Pose of the entrance animation at `elapsed_ms`.
///
/// At 0 ms the fox is [`DROP_HEIGHT_PX`] above its rest position.
pub fn pose(elapsed_ms: u64) -> EntrancePose {
    if elapsed_ms >= TOTAL_MS {
        return EntrancePose::REST;
    }

    if elapsed_ms < FALL_MS {
        // Ease-in quad: slow release, fast impact.
        let t = elapsed_ms as f32 / FALL_MS as f32;
        let y = -(DROP_HEIGHT_PX as f32) * (1.0 - t * t);
        return EntrancePose {
            offset_y: round(y),
            scale_x: 1.0,
            scale_y: 1.0,
            finished: false,
        };
    }

    let settle_elapsed = elapsed_ms - FALL_MS;
    if settle_elapsed < SETTLE_MS {
        let u = settle_elapsed as f32 / SETTLE_MS as f32;
        // Damped two-cycle cosine: impact at u=0 (scale_y = 1 - IMPACT_SQUASH),
        // reset at u=0.25, overshoot at u=0.5, settle at u=1.
        let damping = 1.0 - u;
        let wave = cos(u * PI * 2.0);
        let scale_y = 1.0 - IMPACT_SQUASH * damping * wave;
        // Half the squash goes into width; the widget caps the result at
        // `MAX_POSE_SCALE` so the ears stay inside the canvas.
        let scale_x = 1.0 + (1.0 - scale_y) * 0.5;
        return EntrancePose {
            offset_y: 0,
            scale_x,
            scale_y,
            finished: false,
        };
    }

    // The hop: one sine arch, landing back at the center.
    let hop_elapsed = settle_elapsed - SETTLE_MS;
    let u = hop_elapsed as f32 / HOP_MS as f32;
    let height = sin(u * PI);
    let scale_y = 1.0 + 0.05 * height;
    EntrancePose {
        offset_y: -round(HOP_HEIGHT_PX * height),
        scale_x: 1.0 - 0.03 * height,
        scale_y,
        finished: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fox_starts_above_the_panel_and_lands_in_the_center() {
        let start = pose(0);
        assert_eq!(start.offset_y, -DROP_HEIGHT_PX);
        assert!(!start.finished);

        let landed = pose(FALL_MS);
        assert_eq!(landed.offset_y, 0);
        assert!((landed.scale_y - (1.0 - IMPACT_SQUASH)).abs() < 0.01, "impact squash");
        assert!(landed.scale_x > 1.0, "impact widens the fox");
    }

    #[test]
    fn the_squash_overshoots_then_settles() {
        let quarter = pose(FALL_MS + SETTLE_MS / 4);
        assert!(
            (quarter.scale_y - 1.0).abs() < 0.02,
            "first recovery passes the rest shape"
        );
        let half = pose(FALL_MS + SETTLE_MS / 2);
        assert!(
            half.scale_y > 1.0,
            "the second half of the damped wave overshoots upward"
        );
        let settled = pose(FALL_MS + SETTLE_MS - 1);
        assert!((settled.scale_y - 1.0).abs() < 0.05);
    }

    #[test]
    fn the_hop_returns_to_the_center() {
        let apex = pose(FALL_MS + SETTLE_MS + HOP_MS / 2);
        assert!(apex.offset_y < 0, "the hop lifts the fox");
        assert!(apex.offset_y >= -(HOP_HEIGHT_PX as i32) - 1);
        let end = pose(TOTAL_MS);
        assert_eq!(end, EntrancePose::REST);
        assert!(end.finished);
    }

    #[test]
    fn the_animation_is_monotonic_during_the_fall() {
        let mut previous = i32::MIN;
        let mut t = 0;
        while t < FALL_MS {
            let y = pose(t).offset_y;
            assert!(y >= previous, "the fox only moves down during the fall");
            previous = y;
            t += 20;
        }
        assert_eq!(pose(FALL_MS).offset_y, 0, "the fall ends at the center");
    }

    #[test]
    fn every_frame_is_finite_and_bounded() {
        let mut t = 0;
        while t <= TOTAL_MS + 100 {
            let frame = pose(t);
            assert!(frame.scale_x.is_finite() && frame.scale_y.is_finite());
            assert!((0.5..1.5).contains(&frame.scale_x), "scale_x {}", frame.scale_x);
            assert!((0.5..1.5).contains(&frame.scale_y), "scale_y {}", frame.scale_y);
            assert!(frame.offset_y.abs() <= DROP_HEIGHT_PX + 1);
            t += 10;
        }
    }
}
