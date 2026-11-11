//! A screen-space companion with grip, inertia and a recoverable slip.
//!
//! BMI270 samples stay in physical units at this boundary. Screen coordinates
//! are X right / Y down. The mounting convention used here is sensor X right,
//! Y up: specific force is opposite gravity, hence (-ax, +ay). Confirm that
//! mounting on the physical board before accepting the direction of movement.
use super::affect::{EventKind, SensorEvent};
use crate::companion::render::math::{PI, atan2, cos, inv_sqrt, sin};

const STALE_MS: u64 = 300;
const ORBIT: f32 = 24.0;
const DEG: f32 = 180.0 / PI;

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
    accel_speed: f32,
    gyro_speed: f32,
    angle: f32,
    position: [f32; 2],
    velocity: [f32; 2],
    blend: f32,
    room: f32,
    stage: BalanceStage,
    stage_ms: u32,
    fast_ms: u32,
    quiet_ms: u32,
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
            accel_speed: 0.0,
            gyro_speed: 0.0,
            angle: 0.0,
            position: [0.0, ORBIT],
            velocity: [0.0; 2],
            blend: 0.0,
            room: 0.0,
            stage: BalanceStage::Resting,
            stage_ms: 0,
            fast_ms: 0,
            quiet_ms: 0,
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
                // Impacts and free fall do not supply an upright reference.
                if !(650.0 * 650.0..=1350.0 * 1350.0).contains(&total) {
                    return;
                }
                let planar = x * x + y * y;
                let threshold = if self.upright { 350.0 } else { 600.0 };
                self.upright = planar > threshold * threshold;
                let previous_at = self.accel_at.replace(event.at_ms);
                if !self.upright {
                    self.raw_angle = None;
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
                self.raw_angle = Some(a);
                // The gyro supplies urgency; gravity supplies the lasting pose.
                self.target =
                    wrap(self.target + wrap(a - self.target) * dt as f32 / (60.0 + dt as f32));
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
            }
            _ => {}
        }
    }

    pub fn tick(&mut self, now_ms: u64) -> BalanceVisual {
        if now_ms <= self.last_tick {
            return self.visual();
        }
        if self
            .accel_at
            .is_none_or(|at| now_ms.saturating_sub(at) > STALE_MS)
        {
            // Do not extrapolate an old sensor stream into an endless fall.
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
        if stage == BalanceStage::Struggling {
            self.hold_side = if wrap(self.target - self.angle) < 0.0 {
                -1.0
            } else {
                1.0
            };
            self.impact_strength = 0.0;
            if matches!(self.face, BalanceFace::Neutral | BalanceFace::Weary) {
                self.face = BalanceFace::Effort;
                self.face_ms = 0;
                self.face_quiet_ms = 0;
            }
        }
        if stage == BalanceStage::Recovering {
            let previous = self.visual();
            self.recovery_grip = previous.grip;
            self.recovery_drag = previous.drag;
        }
        self.stage = stage;
        self.stage_ms = 0;
        self.quiet_ms = 0;
    }

    fn advance(&mut self, ms: u32, now: u64) {
        let dt = ms as f32 / 1000.0;
        self.stage_ms = self.stage_ms.saturating_add(ms);
        self.walk_ms = self.walk_ms.wrapping_add(ms);
        self.blend = approach(self.blend, if self.upright { 1.0 } else { 0.0 }, dt * 4.0);
        if !self.upright {
            self.room = approach(self.room, 0.0, dt * 3.0);
            self.enter(BalanceStage::Resting);
            self.fast_ms = 0;
            self.face = BalanceFace::Neutral;
            self.face_ms = 0;
            self.face_quiet_ms = 0;
            self.angle += wrap(-self.angle) * (dt * 5.0).min(1.0);
            for i in 0..2 {
                self.velocity[i] = 0.0;
            }
            return;
        }
        let room =
            (self.target.abs() / 18.0)
                .min(1.0)
                .max(if self.stage == BalanceStage::Resting {
                    0.0
                } else {
                    1.0
                });
        self.room = approach(self.room, room, dt * 2.5);
        let rate = self.accel_speed.max(
            if self.gyro_at.is_some_and(|t| now.saturating_sub(t) < 120) {
                self.gyro_speed
            } else {
                0.0
            },
        );
        let error = wrap(self.target - self.angle);
        let gravity = [-sin(self.target / DEG), cos(self.target / DEG)];
        let destination = [gravity[0] * ORBIT, gravity[1] * ORBIT];
        let fast = rate > 210.0 && error.abs() > 14.0;
        self.fast_ms = if fast {
            self.fast_ms.saturating_add(ms)
        } else {
            self.fast_ms.saturating_sub(ms * 2)
        };

        match self.stage {
            BalanceStage::Resting | BalanceStage::Scooting => {
                if self.fast_ms >= 100 || (error.abs() > 68.0 && rate > 130.0) {
                    self.enter(BalanceStage::Struggling);
                } else if error.abs() > 3.5 || self.accel_speed > 12.0 {
                    if self.stage != BalanceStage::Scooting {
                        self.enter(BalanceStage::Scooting);
                    }
                } else if self.stage == BalanceStage::Scooting && error.abs() < 1.5 && rate < 12.0 {
                    self.enter(BalanceStage::Resting);
                }
                self.angle = wrap(self.angle + error.clamp(-110.0 * dt, 110.0 * dt));
                for i in 0..2 {
                    self.position[i] += (destination[i] - self.position[i]) * (dt * 7.0).min(1.0);
                }
            }
            BalanceStage::Struggling => {
                self.impact_strength = self
                    .impact_strength
                    .max(((rate - 180.0) / 400.0).clamp(0.0, 1.0));
                // A visible grip attempt precedes release; stopping can save it.
                if rate < 95.0 {
                    self.quiet_ms = self.quiet_ms.saturating_add(ms);
                } else {
                    self.quiet_ms = 0;
                }
                self.angle = wrap(self.angle + error.clamp(-38.0 * dt, 38.0 * dt));
                if self.quiet_ms >= 140 && self.stage_ms >= 220 {
                    self.fast_ms = 0;
                    self.enter(BalanceStage::Recovering);
                } else if self.stage_ms >= 500 && (rate > 160.0 || error.abs() > 65.0) {
                    self.velocity = [-gravity[0] * 20.0, -gravity[1] * 20.0];
                    self.enter(BalanceStage::Falling);
                }
            }
            BalanceStage::Falling => {
                self.angle = wrap(self.angle + error.clamp(-280.0 * dt, 280.0 * dt));
                for i in 0..2 {
                    self.velocity[i] += gravity[i] * 560.0 * dt;
                    self.position[i] += self.velocity[i] * dt;
                }
                let depth = self.position[0] * gravity[0] + self.position[1] * gravity[1];
                if depth >= ORBIT || self.stage_ms >= 650 {
                    self.position = destination;
                    self.velocity = [0.0; 2];
                    self.fast_ms = 0;
                    self.enter(BalanceStage::Landing);
                }
            }
            BalanceStage::Landing => {
                self.angle = wrap(self.angle + error.clamp(-180.0 * dt, 180.0 * dt));
                for i in 0..2 {
                    self.position[i] += (destination[i] - self.position[i]) * (dt * 8.0).min(1.0);
                }
                if self.stage_ms >= 650 {
                    self.enter(BalanceStage::Recovering);
                }
            }
            BalanceStage::Recovering => {
                self.angle = wrap(self.angle + error.clamp(-150.0 * dt, 150.0 * dt));
                for i in 0..2 {
                    self.position[i] += (destination[i] - self.position[i]) * (dt * 8.0).min(1.0);
                }
                if self.stage_ms >= 500 {
                    self.enter(BalanceStage::Resting);
                }
            }
        }
        self.advance_face(ms, rate);
    }

    fn advance_face(&mut self, ms: u32, rate: f32) {
        if self.face == BalanceFace::Neutral {
            return;
        }
        self.face_ms = self.face_ms.saturating_add(ms);
        self.face_quiet_ms = if rate < 35.0 {
            self.face_quiet_ms.saturating_add(ms)
        } else {
            0
        };
        let next = match self.face {
            BalanceFace::Effort | BalanceFace::Dizzy if self.face_quiet_ms >= 160 => {
                BalanceFace::Weary
            }
            BalanceFace::Effort if self.face_ms >= 280 => BalanceFace::Dizzy,
            BalanceFace::Weary
                if self.face_ms >= 900
                    && matches!(self.stage, BalanceStage::Resting | BalanceStage::Scooting) =>
            {
                BalanceFace::Neutral
            }
            face => face,
        };
        if next != self.face {
            self.face = next;
            self.face_ms = 0;
        }
    }

    fn visual(&self) -> BalanceVisual {
        if self.blend <= 0.001 {
            return BalanceVisual::REST;
        }
        let angle = self.angle * self.blend;
        let activity = self.room * self.blend;
        let r = angle / DEG;
        let (c, s) = (cos(r).abs(), sin(r).abs());
        let fit = (352.0 / (352.0 * c + 336.0 * s))
            .min(336.0 / (336.0 * c + 352.0 * s))
            .min(1.0);
        let (reach, step, squash) = match self.stage {
            BalanceStage::Scooting => (0.24, sin(self.walk_ms as f32 / 80.0), 0.0),
            BalanceStage::Struggling => (
                0.3 + 0.7 * (self.stage_ms as f32 / 400.0).min(1.0),
                sin(self.stage_ms as f32 / 42.0),
                0.025,
            ),
            BalanceStage::Falling => (0.65, sin(self.stage_ms as f32 / 45.0), -0.04),
            BalanceStage::Landing => (
                0.12 * (1.0 - self.stage_ms as f32 / 650.0),
                0.0,
                (0.16 + self.impact_strength * 0.17)
                    * (1.0 - self.stage_ms as f32 / 480.0).max(0.0)
                    * cos(self.stage_ms as f32 / 72.0),
            ),
            BalanceStage::Recovering => (0.10 * (1.0 - self.stage_ms as f32 / 500.0), 0.0, 0.0),
            BalanceStage::Resting => (0.0, 0.0, 0.0),
        };
        // A disk bounds the moving centre even if the device keeps turning in
        // mid-air. Pixel placement never escapes the fixed panel upload window.
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
            angle_deg: angle,
            x: self.position[0] * activity * limit,
            y: self.position[1] * activity * limit,
            scale: fit * (1.0 - activity * 0.12),
            squash,
            reach,
            step,
            drag: match self.stage {
                BalanceStage::Struggling => (self.stage_ms as f32 / 400.0).min(1.0),
                BalanceStage::Falling => 1.0,
                BalanceStage::Recovering => {
                    self.recovery_drag * (1.0 - self.stage_ms as f32 / 500.0).max(0.0)
                }
                _ => 0.0,
            },
            grip: match self.stage {
                BalanceStage::Struggling => (self.stage_ms as f32 / 140.0).min(1.0),
                BalanceStage::Falling => (1.0 - self.stage_ms as f32 / 180.0).max(0.0),
                BalanceStage::Recovering => {
                    self.recovery_grip * (1.0 - self.stage_ms as f32 / 500.0).max(0.0)
                }
                _ => 0.0,
            },
            hold_side: self.hold_side,
        }
    }
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
