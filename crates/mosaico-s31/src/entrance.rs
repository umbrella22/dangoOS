//! Ball entrance animation after the boot wordmark.
//!
//! The ball drops in from above the panel, lands with a squash, and settles
//! with a small hop. The animation is a pure function of elapsed milliseconds,
//! so the host tests pin exact poses and the widget only has to feed the clock.
//!
//! # Why these numbers
//!
//! - A 380 ms fall over 360 px is slightly faster than real gravity at this
//!   scale; a physically exact drop reads as floaty on a 480x480 AMOLED.
//! - The landing squash uses a damped two-cycle cosine: 72% height on impact,
//!   overshoot, settle. It is the single detail that makes the drop feel like
//!   weight rather than a slide.
//! - The hop is 30 px and short (280 ms); it releases the tension without
//!   delaying the first interaction.
//!
//! ## Touch hold-off
//!
//! The platform passes [`TOTAL_MS`] to the personality engine's hold-off, so a
//! finger on the panel during the entrance cannot grab or interrupt the ball.

/// Ball offset above the canvas centre at the start of the entrance.
///
/// The drop is baked into the 240x240 bitmap now (the plane's rectangle is
/// fixed), so this is in canvas pixels, not screen pixels. 40 px is a visible
/// drop that still leaves the ball inside its own canvas.
pub const DROP_HEIGHT_PX: i32 = 40;
/// Fall duration.
pub const FALL_MS: u64 = 220;
/// Squash and settle duration.
pub const SETTLE_MS: u64 = 240;
/// Peak impact squash (height multiplier at touchdown).
///
/// `0.80` keeps the blob readable as a ball with a face; a stronger squash
/// (0.72) flattened the eyes into a pancake for a frame.
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
    /// Canvas offset from the panel center, in pixels.
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

/// Small polynomial sine approximation (radians), error below 1e-4 in
/// `0..=pi`. `core` has no float transcendentals on the target.
///
/// This is the same shape the ball engine uses (`grok_ball::engine`'s private
/// helper), duplicated here because the engine only exposes the animation
/// state, not the math.
fn sin_approx(x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    const TAU: f32 = core::f32::consts::TAU;
    if !x.is_finite() {
        return 0.0;
    }
    let mut x = x % TAU;
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }
    if x > PI * 0.5 {
        x = PI - x;
    } else if x < -PI * 0.5 {
        x = -PI - x;
    }
    let x2 = x * x;
    x * (1.0 + x2 * (-1.0 / 6.0 + x2 * (1.0 / 120.0 - x2 / 5040.0)))
}

fn cos_approx(x: f32) -> f32 {
    sin_approx(x + core::f32::consts::PI * 0.5)
}

#[inline]
fn round_i32(value: f32) -> i32 {
    if value >= 0.0 {
        (value + 0.5) as i32
    } else {
        (value - 0.5) as i32
    }
}

/// Pose of the entrance animation at `elapsed_ms`.
///
/// Before the first millisecond the ball is above the panel and fully hidden
/// (`offset_y = -DROP_HEIGHT_PX`, which places the canvas outside the screen).
pub fn pose(elapsed_ms: u64) -> EntrancePose {
    if elapsed_ms >= TOTAL_MS {
        return EntrancePose::REST;
    }

    if elapsed_ms < FALL_MS {
        // Ease-in quad: slow release, fast impact.
        let t = elapsed_ms as f32 / FALL_MS as f32;
        let y = -(DROP_HEIGHT_PX as f32) * (1.0 - t * t);
        return EntrancePose {
            offset_y: round_i32(y),
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
        let wave = cos_approx(u * core::f32::consts::PI * 2.0);
        let scale_y = 1.0 - IMPACT_SQUASH * damping * wave;
        // The blob already fills ~88% of its 240x240 canvas, so the widest
        // safe stretch is 1.10. A stronger "volume preserving" widening would
        // be cut off by the canvas edge and read as a clipped sprite.
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
    let height = sin_approx(u * core::f32::consts::PI);
    let scale_y = 1.0 + 0.05 * height;
    EntrancePose {
        offset_y: -round_i32(HOP_HEIGHT_PX * height),
        scale_x: 1.0 - 0.03 * height,
        scale_y,
        finished: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ball_starts_above_the_panel_and_lands_in_the_center() {
        let start = pose(0);
        assert_eq!(start.offset_y, -DROP_HEIGHT_PX);
        assert!(!start.finished);

        let landed = pose(FALL_MS);
        assert_eq!(landed.offset_y, 0);
        assert!((landed.scale_y - (1.0 - IMPACT_SQUASH)).abs() < 0.01, "impact squash");
        assert!(landed.scale_x > 1.0, "impact widens the ball");
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
        assert!(apex.offset_y < 0, "the hop lifts the ball");
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
            assert!(y >= previous, "the ball only moves down during the fall");
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
