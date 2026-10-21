//! Ball engine core (G0.2).
//!
//! Ports the animation subset of the pinned MIT-licensed `tycoding/grok-ball`
//! engine: expression selection, deterministic pool rotation, cubic ring
//! transitions, gaze drift, blink spring and body breathing. The engine is
//! pure `core` Rust: no allocation, no I/O, no globals, no RTOS and no panic
//! path on normal input.
//!
//! The frame keeps the source-space ring polygons. Only the rasterizer applies
//! the canvas transform (`320 / 259`, source center 114.2705).

use crate::data;
use crate::types::{
    BallExpression, BodyParams, ExpressionSeed, EyePose, EyeRings, Point, BODY_POINTS, RING_POINTS,
};

/// Largest time step applied by a single `advance` call.
const MAX_ADVANCE_MS: u32 = 1_000;

/// Fixed simulation sub-step used for timers and the blink spring.
const SIM_STEP_MS: u32 = 4;

/// Blink spring frequency and damping ratio, matching the upstream engine.
const BLINK_FREQ: f32 = 26.0;
const BLINK_DAMPING: f32 = 1.0;

/// Upstream blink keyframes: closed, overshoot, settle.
const BLINK_CLOSE_MS: u64 = 70;
const BLINK_OVERSHOOT_MS: u64 = 150;
const BLINK_SETTLE_MS: u64 = 300;
const BLINK_CLOSED: f32 = 0.05;
const BLINK_OVERSHOOT: f32 = 1.08;

/// Openness clamp applied before the pose is exposed.
const OPEN_MIN: f32 = 0.02;
const OPEN_MAX: f32 = 1.5;

const PI: f32 = core::f32::consts::PI;
const TAU: f32 = core::f32::consts::TAU;

/// Breathing period from the upstream engine: 3.6 s.
const BREATHE_PERIOD_MS: f32 = 3_600.0;
const BREATHE_Y_AMPLITUDE: f32 = 55.0;

/// Fixed xorshift seed. Every engine instance replays the same sequence so an
/// identical timeline always produces identical frames.
const PRNG_SEED: u32 = 0x9E37_79B9;

/// Fallback used when `data::seed` (or a custom seed table) has no entry for a
/// requested expression.
static EMPTY_SEED: ExpressionSeed = ExpressionSeed::EMPTY;

/// Fallback used when a pool references a ring outside the configured table.
static EMPTY_RINGS: EyeRings = EyeRings::EMPTY;

/// One animated frame in source coordinates.
///
/// `body_ring` borrows the pinned body outline. `left_ring` and `right_ring`
/// are owned copies: settled frames hold the selected table ring, and
/// transition frames hold interpolated points. No allocation and no borrowed
/// engine state are involved, so a frame can outlive the engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BallFrame {
    pub body_ring: &'static [Point; BODY_POINTS],
    pub left_ring: [Point; RING_POINTS],
    pub right_ring: [Point; RING_POINTS],
    pub body: BodyParams,
    pub left: EyePose,
    pub right: EyePose,
    pub openness: f32,
}

/// Deterministic expression, ring, gaze, blink and body animation state.
///
/// There is no global state: any number of engines can run independently. All
/// time is integer milliseconds; float state is confined to the blink spring
/// and the derived pose.
pub struct BallEngine {
    ring_table: &'static [EyeRings],
    seed_table: &'static [ExpressionSeed],
    seed: &'static ExpressionSeed,
    has_seed: bool,

    now_ms: u64,
    rng: Rng,

    // Expression pool.
    pool_index: usize,
    rotation_enabled: bool,
    next_rotation_ms: u64,

    // Ring transition. `from_*` is a frozen source-space snapshot; `to_index`
    // names the target ring set in `ring_table`.
    from_left: [Point; RING_POINTS],
    from_right: [Point; RING_POINTS],
    to_index: u8,
    transition_start_ms: u64,
    transition_ms: u32,
    transition_active: bool,

    // Rings exposed by `frame()`. When `display_static` is true the frame
    // borrows `ring_table[to_index]` directly and these arrays are stale.
    display_left: [Point; RING_POINTS],
    display_right: [Point; RING_POINTS],
    display_static: bool,

    // Blink.
    open_spring: Spring,
    blink_enabled: bool,
    next_blink_ms: u64,
    blink_start_ms: u64,
    blinking: bool,
}

impl BallEngine {
    /// Create an engine over the pinned [`crate::data`] tables.
    pub fn new() -> Self {
        Self::with_tables(&data::RINGS, &data::SEEDS)
    }

    /// Create an engine over caller-supplied static tables.
    ///
    /// `new()` is the production constructor. This entry point lets tests and
    /// future alternate shape sets provide their own ring/seed tables without
    /// changing the frozen data module.
    pub fn with_tables(rings: &'static [EyeRings], seeds: &'static [ExpressionSeed]) -> Self {
        let seed: &'static ExpressionSeed = &EMPTY_SEED;
        let mut engine = Self {
            ring_table: rings,
            seed_table: seeds,
            seed,
            has_seed: false,
            now_ms: 0,
            rng: Rng::new(PRNG_SEED),
            pool_index: 0,
            rotation_enabled: false,
            next_rotation_ms: u64::MAX,
            from_left: [Point::ZERO; RING_POINTS],
            from_right: [Point::ZERO; RING_POINTS],
            to_index: 0,
            transition_start_ms: 0,
            transition_ms: 0,
            transition_active: false,
            display_left: [Point::ZERO; RING_POINTS],
            display_right: [Point::ZERO; RING_POINTS],
            display_static: true,
            open_spring: Spring::new(1.0),
            blink_enabled: false,
            next_blink_ms: u64::MAX,
            blink_start_ms: 0,
            blinking: false,
        };
        engine.refresh_display();
        engine
    }

    /// Select one of the fourteen expressions through its upstream seed id.
    ///
    /// Unknown seeds fall back to [`ExpressionSeed::EMPTY`] and the engine
    /// keeps running. Selecting the same expression again is a no-op.
    pub fn set_expression(&mut self, expression: BallExpression) {
        let requested = expression.seed_id();
        let seed = self
            .seed_table
            .iter()
            .find(|candidate| candidate.id == requested)
            .unwrap_or(&EMPTY_SEED);
        self.apply_seed(seed);
    }

    /// Select a seed directly. Used by tests and future direct control.
    ///
    /// Selecting the same seed again is a no-op.
    pub fn set_seed(&mut self, seed: &'static ExpressionSeed) {
        self.apply_seed(seed);
    }

    /// Advance the engine and return the new frame.
    ///
    /// `elapsed_ms` is clamped to 1000 ms per call. Zero is a pure no-op.
    /// The call performs no allocation and never panics for arbitrary input.
    pub fn advance(&mut self, elapsed_ms: u32) -> BallFrame {
        let dt = elapsed_ms.min(MAX_ADVANCE_MS);
        if dt == 0 {
            return self.frame();
        }

        let mut remaining = dt;
        while remaining > 0 {
            let step = remaining.min(SIM_STEP_MS);
            remaining -= step;
            self.now_ms = self.now_ms.saturating_add(step as u64);
            self.step_timers();
            self.step_springs(step);
        }

        self.update_display();
        self.frame()
    }

    /// Compose the current frame without advancing time.
    pub fn frame(&self) -> BallFrame {
        let openness = clampf(self.open_spring.x, OPEN_MIN, OPEN_MAX);
        let (body, left, right) = self.compose_pose(openness);

        let (left_ring, right_ring) = if self.display_static {
            let rings = self.ring_at(self.to_index);
            (rings.left, rings.right)
        } else {
            (self.display_left, self.display_right)
        };

        BallFrame {
            body_ring: &data::BODY_RING,
            left_ring,
            right_ring,
            body,
            left,
            right,
            openness,
        }
    }

    // ---- expression / seed selection ------------------------------------

    fn apply_seed(&mut self, seed: &'static ExpressionSeed) {
        if core::ptr::eq(self.seed, seed) {
            return;
        }

        let initial = !self.has_seed;
        self.seed = seed;
        self.has_seed = true;
        self.pool_index = 0;

        let target = seed.pool.first().copied().unwrap_or(0);
        if initial {
            // The first expression is shown immediately; there is no previous
            // pose to transition from.
            self.transition_active = false;
            self.transition_ms = 0;
            self.to_index = target;
            let base = clampf(finite_or(seed.openness, 1.0), OPEN_MIN, OPEN_MAX);
            self.open_spring = Spring::new(base);
        } else {
            self.begin_transition(target);
            self.open_spring.target = clampf(finite_or(seed.openness, 1.0), OPEN_MIN, OPEN_MAX);
        }

        self.rotation_enabled = seed.pool_min_ms > 0 && seed.pool.len() >= 2;
        self.next_rotation_ms = if self.rotation_enabled {
            self.now_ms
                .saturating_add(self.rng.range(seed.pool_min_ms, seed.pool_max_ms) as u64)
        } else {
            u64::MAX
        };

        self.blink_enabled = seed.blink_min_ms > 0 || seed.blink_max_ms > 0;
        self.next_blink_ms = if self.blink_enabled {
            self.now_ms
                .saturating_add(self.rng.range(seed.blink_min_ms, seed.blink_max_ms) as u64)
        } else {
            u64::MAX
        };
        self.blink_start_ms = 0;
        self.blinking = false;

        self.refresh_display();
    }

    /// Resolve a ring index without panicking on out-of-range pool data.
    fn ring_at(&self, index: u8) -> &'static EyeRings {
        self.ring_table.get(index as usize).unwrap_or(&EMPTY_RINGS)
    }

    // ---- ring transition ------------------------------------------------

    fn begin_transition(&mut self, target: u8) {
        let (left, right) = self.sample_rings(self.now_ms);
        self.from_left = left;
        self.from_right = right;
        self.to_index = target;
        self.transition_start_ms = self.now_ms;
        self.transition_ms = self.seed.transition_ms as u32;
        self.transition_active = self.transition_ms > 0;

        if !self.transition_active {
            self.display_static = true;
        }
    }

    /// Sample the displayed rings at `at_ms` without mutating scratch state.
    fn sample_rings(&self, at_ms: u64) -> ([Point; RING_POINTS], [Point; RING_POINTS]) {
        let target = self.ring_at(self.to_index);
        if !self.transition_active || self.transition_ms == 0 {
            return (target.left, target.right);
        }

        let elapsed = at_ms.saturating_sub(self.transition_start_ms);
        if elapsed >= self.transition_ms as u64 {
            return (target.left, target.right);
        }

        let progress = elapsed as f32 / self.transition_ms as f32;
        let eased = ease_in_out_cubic(clampf(progress, 0.0, 1.0));

        let mut left = [Point::ZERO; RING_POINTS];
        let mut right = [Point::ZERO; RING_POINTS];
        for i in 0..RING_POINTS {
            left[i] = lerp_point(self.from_left[i], target.left[i], eased);
            right[i] = lerp_point(self.from_right[i], target.right[i], eased);
        }
        (left, right)
    }

    fn refresh_display(&mut self) {
        self.update_display();
    }

    fn update_display(&mut self) {
        if !self.transition_active {
            self.display_static = true;
            return;
        }

        let elapsed = self.now_ms.saturating_sub(self.transition_start_ms);
        if elapsed >= self.transition_ms as u64 {
            self.transition_active = false;
            self.display_static = true;
            return;
        }

        let (left, right) = self.sample_rings(self.now_ms);
        self.display_left = left;
        self.display_right = right;
        self.display_static = false;
    }

    // ---- timers and springs ---------------------------------------------

    fn step_timers(&mut self) {
        if self.rotation_enabled && self.now_ms >= self.next_rotation_ms {
            let len = self.seed.pool.len();
            if len >= 2 {
                self.pool_index = (self.pool_index + 1) % len;
                if let Some(&target) = self.seed.pool.get(self.pool_index) {
                    self.begin_transition(target);
                }
            }
            let interval = self
                .rng
                .range(self.seed.pool_min_ms, self.seed.pool_max_ms)
                .max(1) as u64;
            self.next_rotation_ms = self.now_ms.saturating_add(interval);
        }

        if self.blink_enabled && self.now_ms >= self.next_blink_ms {
            self.blink_start_ms = self.now_ms;
            self.blinking = true;
            let interval = self
                .rng
                .range(self.seed.blink_min_ms, self.seed.blink_max_ms)
                .max(1) as u64;
            self.next_blink_ms = self.now_ms.saturating_add(interval);
        }

        self.open_spring.target = self.blink_target();
    }

    fn blink_target(&mut self) -> f32 {
        if !self.blinking {
            return self.baseline_openness();
        }

        let elapsed = self.now_ms.saturating_sub(self.blink_start_ms);
        if elapsed < BLINK_CLOSE_MS {
            BLINK_CLOSED
        } else if elapsed < BLINK_OVERSHOOT_MS {
            BLINK_OVERSHOOT
        } else if elapsed < BLINK_SETTLE_MS {
            1.0
        } else {
            self.blinking = false;
            self.baseline_openness()
        }
    }

    fn baseline_openness(&self) -> f32 {
        clampf(finite_or(self.seed.openness, 1.0), OPEN_MIN, OPEN_MAX)
    }

    fn step_springs(&mut self, dt_ms: u32) {
        let dt = dt_ms as f32 / 1_000.0;
        self.open_spring.step(BLINK_FREQ, BLINK_DAMPING, dt);
    }

    // ---- pose composition -----------------------------------------------

    fn compose_pose(&self, openness: f32) -> (BodyParams, EyePose, EyePose) {
        let seed = self.seed;
        let now = self.now_ms as f32;
        let seconds = now / 1_000.0;

        let breathe = finite_or(seed.body.breathe, 0.0);
        let phase = TAU * now / BREATHE_PERIOD_MS;
        let sin_phase = sin_approx(phase);
        let cos_phase = cos_approx(phase);
        // sin(phase + 0.6) expanded so both approximations are exercised.
        let breathe_y = sin_phase * COS_06 + cos_phase * SIN_06;

        let body = BodyParams {
            y: finite_or(seed.body.y, 0.0) + breathe * BREATHE_Y_AMPLITUDE * breathe_y,
            scale: finite_or(seed.body.scale, 1.0) + breathe * sin_phase,
            rotate: finite_or(seed.body.rotate, 0.0),
            breathe,
            color: seed.body.color,
        };

        // Slow deterministic gaze drift, one phase per eye (upstream values).
        let left_drift_x = 1.4 * sin_approx(0.42 * seconds) + 0.5 * sin_approx(seconds);
        let right_drift_x =
            1.4 * sin_approx(0.42 * seconds + 1.0) + 0.5 * sin_approx(seconds + 2.0);
        let left_drift_y = 0.9 * sin_approx(0.58 * seconds);
        let right_drift_y = 0.9 * sin_approx(0.58 * seconds + 1.0);

        let left = compose_eye(seed.eyes.left, openness, left_drift_x, left_drift_y);
        let right = compose_eye(seed.eyes.right, openness, right_drift_x, right_drift_y);

        (body, left, right)
    }
}

impl Default for BallEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn compose_eye(base: EyePose, openness: f32, drift_x: f32, drift_y: f32) -> EyePose {
    EyePose {
        open: clampf(finite_or(base.open, 1.0), 0.0, 1.3) * openness,
        scale_x: finite_or(base.scale_x, 1.0).max(0.05),
        scale_y: finite_or(base.scale_y, 1.0).max(0.05),
        x: finite_or(base.x, 0.0),
        y: finite_or(base.y, 0.0),
        look_x: finite_or(base.look_x, 0.0) + drift_x,
        look_y: finite_or(base.look_y, 0.0) + drift_y,
    }
}

// ---- deterministic PRNG --------------------------------------------------

struct Rng {
    state: u32,
}

impl Rng {
    const fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { PRNG_SEED } else { seed },
        }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Inclusive range. Reversed bounds are normalized; the span is bounded by
    /// `u16`, so `span + 1` cannot overflow `u32`.
    fn range(&mut self, a: u16, b: u16) -> u16 {
        let lo = a.min(b) as u32;
        let hi = a.max(b) as u32;
        let span = hi - lo;
        let offset = if span == 0 {
            0
        } else {
            self.next_u32() % (span + 1)
        };
        (lo + offset) as u16
    }
}

// ---- critically damped spring -------------------------------------------

#[derive(Clone, Copy)]
struct Spring {
    x: f32,
    v: f32,
    target: f32,
}

impl Spring {
    const fn new(x: f32) -> Self {
        Self {
            x,
            v: 0.0,
            target: x,
        }
    }

    /// One explicit Euler step. The caller uses 4 ms sub-steps, well inside the
    /// stability limit for the blink frequency; a non-finite result is reset to
    /// the target instead of propagating NaN.
    fn step(&mut self, omega: f32, zeta: f32, dt: f32) {
        self.v += (-2.0 * zeta * omega * self.v - omega * omega * (self.x - self.target)) * dt;
        self.x += self.v * dt;
        if !self.x.is_finite() || !self.v.is_finite() {
            self.x = self.target;
            self.v = 0.0;
        }
    }
}

// ---- math helpers --------------------------------------------------------

/// Cubic ease-in-out from the upstream engine.
fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - u * u * u * 0.5
    }
}

fn lerp_point(a: Point, b: Point, t: f32) -> Point {
    Point {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
    }
}

/// Small polynomial sine approximation (radians). Maximum absolute error near
/// `pi/2` is about 1.6e-4, which is far below one source unit.
///
/// `pub(crate)` so the personality layer reuses the same math instead of
/// linking a second approximation.
pub(crate) fn sin_approx(x: f32) -> f32 {
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
    // x - x^3/6 + x^5/120 - x^7/5040
    x * (1.0 + x2 * (-1.0 / 6.0 + x2 * (1.0 / 120.0 - x2 / 5040.0)))
}

fn cos_approx(x: f32) -> f32 {
    sin_approx(x + PI * 0.5)
}

const SIN_06: f32 = 0.564_642_5;
const COS_06: f32 = 0.825_335_6;

#[inline]
fn clampf(v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

#[inline]
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}
