//! A screen-space companion with grip, inertia and a recoverable slip.
//!
//! BMI270 samples stay in physical units at this boundary. Screen coordinates
//! are X right / Y down. The mounting convention used here is sensor X right,
//! Y up: specific force is opposite gravity, hence (-ax, +ay). Confirm that
//! mounting on the physical board before accepting the direction of movement.
use super::affect::{EventKind, SensorEvent};
use crate::companion::render::math::{atan2, cos, inv_sqrt, sin, PI};

const STALE_MS: u64 = 300;
const ORBIT: f32 = 24.0;
const DEG: f32 = 180.0 / PI;
const TURN_START_DEG: f32 = 65.0;
const TURN_CANCEL_DEG: f32 = 50.0;
const TURN_CONFIRM_MS: u64 = 80;
const FAST_TURN_DEG: f32 = 140.0;
const FAST_MEMORY_MS: u64 = 650;
const GRIP_MS: u32 = 280;
const FALL_MS: u32 = 380;
const LAND_MS: u32 = 680;
const RECOVER_MS: u32 = 620;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BalanceStage {
    #[default]
    Resting,
    Scooting,
    Struggling,
    Falling,
    Landing,
    Recovering,
}

/// Rotation expressions follow the motion itself, independently of landing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BalanceFace {
    #[default]
    Neutral,
    Effort,
    Startled,
    Dizzy,
    Weary,
}

/// One continuous pose, independent of the Lua/TS expression owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BalanceVisual {
    pub active: bool,
    pub stage: BalanceStage,
    pub face: BalanceFace,
    /// The current Confused expression came from the same IMU motion.
    pub rotation_reaction: bool,
    pub angle_deg: f32,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    /// Landing compression; 0 is the original silhouette.
    pub squash: f32,
    /// How hard the paws reach for support, 0..1.
    pub reach: f32,
    /// Alternating paw movement, -1..1.
    pub step: f32,
    /// Body dragged below the held paws; source-space extension, 0..1.
    pub drag: f32,
    /// Held-paw pose blends out during the first part of release.
    pub grip: f32,
    /// Side chosen at the start of the hold (-1 left, +1 right).
    pub hold_side: f32,
}
impl BalanceVisual {
    pub const REST: Self = Self {
        active: false,
        stage: BalanceStage::Resting,
        face: BalanceFace::Neutral,
        rotation_reaction: false,
        angle_deg: 0.0,
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        squash: 0.0,
        reach: 0.0,
        step: 0.0,
        drag: 0.0,
        grip: 0.0,
        hold_side: 1.0,
    };
}

pub struct Balance {
    last_tick: u64,
    accel_at: Option<u64>,
    gyro_at: Option<u64>,
    upright: bool,
    target: f32,
    raw_angle: Option<f32>,
    /// Signed travel from the last chosen side, including +/-180 crossings.
    turn_deg: f32,
    turn_started_at: Option<u64>,
    turn_side: f32,
    fast_at: Option<u64>,
    peak_rate: f32,
    accel_speed: f32,
    gyro_speed: f32,
    /// A chosen resting side. Sensor noise cannot move this destination.
    goal: f32,
    angle: f32,
    position: [f32; 2],
    from_angle: f32,
    from_position: [f32; 2],
    crawl_ms: u32,
    blend: f32,
    room: f32,
    stage: BalanceStage,
    stage_ms: u32,
    walk_ms: u32,
    recovery_grip: f32,
    recovery_drag: f32,
    hold_side: f32,
    impact_strength: f32,
    face: BalanceFace,
    face_ms: u32,
    face_quiet_ms: u32,
}

impl Balance {
    pub fn new(now_ms: u64) -> Self {
        Self {
            last_tick: now_ms,
            accel_at: None,
            gyro_at: None,
            upright: false,
            target: 0.0,
            raw_angle: None,
            turn_deg: 0.0,
            turn_started_at: None,
            turn_side: 1.0,
            fast_at: None,
            peak_rate: 0.0,
            accel_speed: 0.0,
            gyro_speed: 0.0,
            goal: 0.0,
            angle: 0.0,
            position: [0.0; 2],
            from_angle: 0.0,
            from_position: [0.0; 2],
            crawl_ms: 1400,
            blend: 0.0,
            room: 0.0,
            stage: BalanceStage::Resting,
            stage_ms: 0,
            walk_ms: 0,
            recovery_grip: 0.0,
            recovery_drag: 0.0,
            hold_side: 1.0,
            impact_strength: 0.0,
            face: BalanceFace::Neutral,
            face_ms: 0,
            face_quiet_ms: 0,
        }
    }

    pub fn observe(&mut self, event: SensorEvent) {
        match event.kind {
            EventKind::Accel {
                ax_mg,
                ay_mg,
                az_mg,
            } => {
                if self.accel_at.is_some_and(|t| event.at_ms <= t) {
                    return;
                }
                let (x, y, z) = (-(ax_mg as f32), ay_mg as f32, az_mg as f32);
                let total = x * x + y * y + z * z;
                if !(650.0 * 650.0..=1350.0 * 1350.0).contains(&total) {
                    return;
                }
                let threshold = if self.upright { 350.0 } else { 600.0 };
                self.upright = x * x + y * y > threshold * threshold;
                let previous_at = self.accel_at.replace(event.at_ms);
                if !self.upright {
                    self.raw_angle = None;
                    self.turn_deg = 0.0;
                    self.turn_started_at = None;
                    self.accel_speed = 0.0;
                    return;
                }
                let a = atan2(-x, y) * DEG;
                let dt = previous_at
                    .map_or(20, |t| event.at_ms.saturating_sub(t))
                    .clamp(1, 100);
                self.accel_speed = self
                    .raw_angle
                    .map_or(0.0, |p| wrap(a - p).abs() * 1000.0 / dt as f32);
                self.turn_deg += wrap(a - self.raw_angle.unwrap_or(self.goal));
                self.raw_angle = Some(a);
                self.target =
                    wrap(self.target + wrap(a - self.target) * dt as f32 / (60.0 + dt as f32));
                if self.stage == BalanceStage::Resting {
                    if self.turn_started_at.is_some()
                        && self.turn_deg * self.turn_side < TURN_CANCEL_DEG
                    {
                        self.turn_started_at = None;
                    }
                    if self.turn_started_at.is_none() && self.turn_deg.abs() >= TURN_START_DEG {
                        self.turn_started_at = Some(event.at_ms);
                        self.turn_side = self.turn_deg.signum();
                    }
                }
                if self.accel_speed >= 250.0
                    && self
                        .gyro_at
                        .is_some_and(|at| event.at_ms.saturating_sub(at) <= 120)
                    && self.gyro_speed >= 200.0
                {
                    self.note_fast(event.at_ms);
                }
            }
            EventKind::Gyro {
                gx_dps,
                gy_dps,
                gz_dps,
            } => {
                if self.gyro_at.is_some_and(|t| event.at_ms <= t) {
                    return;
                }
                self.gyro_at = Some(event.at_ms);
                self.gyro_speed = (gx_dps as f32)
                    .abs()
                    .max((gy_dps as f32).abs())
                    .max((gz_dps as f32).abs());
                if self.gyro_speed >= 250.0 && self.accel_speed >= 180.0 {
                    self.note_fast(event.at_ms);
                }
            }
            _ => {}
        }
    }

    fn note_fast(&mut self, at: u64) {
        self.fast_at = Some(at);
        self.peak_rate = self.peak_rate.max(self.accel_speed.max(self.gyro_speed));
    }

    pub fn tick(&mut self, now_ms: u64) -> BalanceVisual {
        if now_ms <= self.last_tick {
            return self.visual();
        }
        if self
            .accel_at
            .is_none_or(|at| now_ms.saturating_sub(at) > STALE_MS)
        {
            *self = Self::new(now_ms);
            return BalanceVisual::REST;
        }
        let mut remaining = now_ms.saturating_sub(self.last_tick).min(250) as u32;
        self.last_tick = now_ms;
        while remaining > 0 {
            let ms = remaining.min(20);
            self.advance(ms, now_ms);
            remaining -= ms;
        }
        self.visual()
    }

    fn enter(&mut self, stage: BalanceStage) {
        if stage == BalanceStage::Recovering {
            let previous = self.visual();
            self.recovery_grip = previous.grip;
            self.recovery_drag = previous.drag;
        }
        self.from_angle = self.angle;
        self.from_position = self.position;
        self.stage = stage;
        self.stage_ms = 0;
    }

    fn start_crawl(&mut self, goal: f32) {
        self.goal = goal;
        self.hold_side = if wrap(goal - self.angle) < 0.0 {
            -1.0
        } else {
            1.0
        };
        self.crawl_ms = (wrap(goal - self.angle).abs() * 1000.0 / 65.0).clamp(900.0, 2800.0) as u32;
        self.turn_started_at = None;
        self.enter(BalanceStage::Scooting);
    }

    fn finish(&mut self) {
        self.angle = self.goal;
        self.position = destination(self.goal);
        self.turn_deg = wrap(self.raw_angle.unwrap_or(self.goal) - self.goal);
        self.turn_started_at = None;
        self.fast_at = None;
        self.peak_rate = 0.0;
        self.enter(BalanceStage::Resting);
    }

    fn advance(&mut self, ms: u32, now: u64) {
        let dt = ms as f32 / 1000.0;
        self.stage_ms = self.stage_ms.saturating_add(ms);
        self.walk_ms = self.walk_ms.wrapping_add(ms);
        self.blend = approach(self.blend, if self.upright { 1.0 } else { 0.0 }, dt * 4.0);
        if !self.upright {
            self.room = approach(self.room, 0.0, dt * 3.0);
            self.goal = 0.0;
            self.fast_at = None;
            self.peak_rate = 0.0;
            self.angle += wrap(-self.angle) * (dt * 5.0).min(1.0);
            self.enter(BalanceStage::Resting);
            self.set_face(BalanceFace::Neutral);
            return;
        }
        let rate = self.motion_rate(now);
        let heading = self.raw_angle.unwrap_or(self.target);
        if self.stage == BalanceStage::Resting {
            let confirmed = self.turn_started_at.is_some_and(|started| {
                self.accel_at.unwrap_or(started).saturating_sub(started) >= TURN_CONFIRM_MS
                    && self.turn_deg * self.turn_side >= TURN_START_DEG
            });
            if confirmed {
                self.start_crawl(side_angle(heading));
            } else {
                self.advance_face(ms, rate);
                return;
            }
        }
        // Coalesce a new side into the current action. Fine angle changes do
        // not alter either the path or its end time.
        if matches!(
            self.stage,
            BalanceStage::Scooting | BalanceStage::Struggling
        ) && wrap(heading - self.goal).abs() >= TURN_START_DEG
        {
            let goal = side_angle(heading);
            if self.stage == BalanceStage::Scooting {
                self.start_crawl(goal);
            } else {
                self.goal = goal;
            }
        }
        // A quick half-turn stays committed after the hand stops. Otherwise
        // a completed 180-degree flip incorrectly takes the rescue shortcut.
        if self.stage == BalanceStage::Scooting
            && self.turn_deg.abs() >= FAST_TURN_DEG
            && self
                .fast_at
                .is_some_and(|at| now.saturating_sub(at) <= FAST_MEMORY_MS)
        {
            self.goal = side_angle(heading);
            self.impact_strength = ((self.peak_rate - 180.0) / 500.0).clamp(0.0, 1.0);
            self.enter(BalanceStage::Struggling);
            self.set_face(BalanceFace::Startled);
        }
        self.room = approach(self.room, 1.0, dt * 3.0);
        let dest = destination(self.goal);
        match self.stage {
            BalanceStage::Scooting => {
                let u = (self.stage_ms as f32 / self.crawl_ms as f32).min(1.0);
                self.move_to(dest, smooth(u), smooth(u));
                if u >= 1.0 {
                    self.finish();
                }
            }
            BalanceStage::Struggling => {
                // Brief catch and wobble, with the body visibly lagging the paws.
                let u = (self.stage_ms as f32 / GRIP_MS as f32).min(1.0);
                self.angle =
                    wrap(self.from_angle + wrap(self.goal - self.from_angle) * smooth(u) * 0.10);
                if self.stage_ms >= GRIP_MS {
                    self.enter(BalanceStage::Falling);
                }
            }
            BalanceStage::Falling => {
                let u = (self.stage_ms as f32 / FALL_MS as f32).min(1.0);
                self.move_to(dest, smooth(u), u * u);
                if u >= 1.0 {
                    self.angle = self.goal;
                    self.position = dest;
                    self.enter(BalanceStage::Landing);
                }
            }
            BalanceStage::Landing => {
                if self.stage_ms >= LAND_MS {
                    self.enter(BalanceStage::Recovering);
                }
            }
            BalanceStage::Recovering => {
                if self.stage_ms >= RECOVER_MS {
                    self.finish();
                }
            }
            BalanceStage::Resting => {}
        }
        self.advance_face(ms, rate);
    }

    fn move_to(&mut self, dest: [f32; 2], angle_progress: f32, position_progress: f32) {
        self.angle = wrap(self.from_angle + wrap(self.goal - self.from_angle) * angle_progress);
        for (i, value) in dest.into_iter().enumerate() {
            self.position[i] =
                self.from_position[i] + (value - self.from_position[i]) * position_progress;
        }
    }

    fn motion_rate(&self, now: u64) -> f32 {
        self.accel_speed.max(
            if self.gyro_at.is_some_and(|t| now.saturating_sub(t) < 120) {
                self.gyro_speed
            } else {
                0.0
            },
        )
    }

    fn set_face(&mut self, face: BalanceFace) {
        if face != self.face {
            self.face = face;
            self.face_ms = 0;
            self.face_quiet_ms = 0;
        }
    }

    fn advance_face(&mut self, ms: u32, rate: f32) {
        self.face_ms = self.face_ms.saturating_add(ms);
        self.face_quiet_ms = if rate < 35.0 {
            self.face_quiet_ms.saturating_add(ms)
        } else {
            0
        };
        let spinning = self.turn_deg.abs() >= 300.0 && rate > 90.0;
        let face = match self.stage {
            BalanceStage::Struggling => BalanceFace::Startled,
            BalanceStage::Falling if spinning => BalanceFace::Dizzy,
            BalanceStage::Falling => BalanceFace::Startled,
            BalanceStage::Landing if self.stage_ms < 120 => self.face,
            BalanceStage::Landing | BalanceStage::Recovering if spinning => BalanceFace::Dizzy,
            BalanceStage::Landing | BalanceStage::Recovering => BalanceFace::Weary,
            _ if matches!(self.face, BalanceFace::Dizzy | BalanceFace::Startled)
                && self.face_quiet_ms >= 160 =>
            {
                BalanceFace::Weary
            }
            _ if self.face == BalanceFace::Weary && self.face_ms >= 900 => BalanceFace::Neutral,
            _ => self.face,
        };
        self.set_face(face);
    }

    fn visual(&self) -> BalanceVisual {
        if self.blend <= 0.001 {
            return BalanceVisual::REST;
        }
        let activity = self.room * self.blend;
        let (reach, step, squash) = match self.stage {
            BalanceStage::Scooting => {
                let envelope = (self.stage_ms as f32 / 160.0)
                    .min(1.0)
                    .min(self.crawl_ms.saturating_sub(self.stage_ms) as f32 / 180.0);
                (
                    0.22 * envelope,
                    sin(self.walk_ms as f32 / 120.0) * envelope,
                    0.0,
                )
            }
            BalanceStage::Struggling => (
                0.3 + 0.7 * (self.stage_ms as f32 / 200.0).min(1.0),
                sin(self.stage_ms as f32 / 38.0),
                0.0,
            ),
            BalanceStage::Falling => (0.55, sin(self.stage_ms as f32 / 70.0), 0.0),
            BalanceStage::Landing => {
                let release = smooth((self.stage_ms.saturating_sub(120) as f32 / 440.0).min(1.0));
                (
                    0.08 * (1.0 - release),
                    0.0,
                    (0.26 + self.impact_strength * 0.16) * (1.0 - release),
                )
            }
            BalanceStage::Recovering => (
                0.06 * (1.0 - self.stage_ms as f32 / RECOVER_MS as f32),
                0.0,
                0.0,
            ),
            BalanceStage::Resting => (0.0, 0.0, 0.0),
        };
        let norm2 = self.position[0] * self.position[0] + self.position[1] * self.position[1];
        let limit = if norm2 > 38.0 * 38.0 {
            38.0 * inv_sqrt(norm2)
        } else {
            1.0
        };
        BalanceVisual {
            active: true,
            stage: self.stage,
            face: self.face,
            rotation_reaction: false,
            angle_deg: self.angle * self.blend,
            x: self.position[0] * activity * limit,
            y: self.position[1] * activity * limit,
            scale: 1.0,
            squash,
            reach,
            step,
            drag: match self.stage {
                BalanceStage::Struggling => (self.stage_ms as f32 / 200.0).min(1.0),
                BalanceStage::Falling => 1.0,
                BalanceStage::Recovering => {
                    self.recovery_drag * (1.0 - self.stage_ms as f32 / RECOVER_MS as f32).max(0.0)
                }
                _ => 0.0,
            },
            grip: match self.stage {
                BalanceStage::Struggling => (self.stage_ms as f32 / 100.0).min(1.0),
                BalanceStage::Falling => (1.0 - self.stage_ms as f32 / 160.0).max(0.0),
                BalanceStage::Recovering => {
                    self.recovery_grip * (1.0 - self.stage_ms as f32 / RECOVER_MS as f32).max(0.0)
                }
                _ => 0.0,
            },
            hold_side: self.hold_side,
        }
    }
}

fn destination(angle: f32) -> [f32; 2] {
    [-sin(angle / DEG) * ORBIT, cos(angle / DEG) * ORBIT]
}

fn side_angle(angle: f32) -> f32 {
    wrap(crate::companion::render::math::round(angle / 90.0) as f32 * 90.0)
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn approach(value: f32, target: f32, step: f32) -> f32 {
    value + (target - value).clamp(-step, step)
}

fn wrap(angle: f32) -> f32 {
    let mut a = angle % 360.0;
    if a > 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

#[cfg(test)]
mod tests;
