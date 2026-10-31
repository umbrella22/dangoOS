//! Expression plus time to [`Scene`]: breathing, blinks, mouth flaps, gaze,
//! pops, hops, shakes and the per-pose symbols.
//!
//! The timing per expression follows `assets/fox-dango/tools/fox_anim.py`,
//! which renders the same animation on the host for review.

use crate::companion::render::skin::Skin;
use alloc::rc::Rc;
use {
    crate::companion::render::math::clamp, crate::companion::render::math::cos,
    crate::companion::render::math::exp_neg, crate::companion::render::math::inv_sqrt,
    crate::companion::render::math::round, crate::companion::render::math::sin,
    crate::companion::render::math::PI, crate::companion::render::math::TAU,
};
use {
    crate::companion::render::scene::Layer, crate::companion::render::scene::Scene,
    crate::companion::render::scene::Warp,
};
use {crate::companion::render::FoxPose, crate::companion::render::H, crate::companion::render::W};

fn phase(t: u32, period: u32, offset: u32) -> f32 {
    ((t + offset) % period) as f32 / period as f32
}

/// Breathing period (ms) and depth (fraction) per pose; 0 = none.
fn breath(pose: FoxPose) -> (u32, f32) {
    match pose {
        FoxPose::Sleep => (4500, 0.025),
        FoxPose::Puzzled | FoxPose::Thinking => (3600, 0.012),
        FoxPose::Peering => (3600, 0.010),
        FoxPose::Satisfied => (3600, 0.015),
        FoxPose::Panic => (600, 0.02),
        FoxPose::Tired => (4000, 0.015),
        FoxPose::Dying => (6000, 0.008),
        FoxPose::Happy | FoxPose::Surprised | FoxPose::Error | FoxPose::Dazzled => (3000, 0.01),
        _ => (3000, 0.015),
    }
}

/// Blink interval range in ms per pose; `None` for closed-eye poses.
fn blink_range(pose: FoxPose, skin: &Skin) -> Option<(u32, u32)> {
    match pose {
        FoxPose::Panic => Some((1500, 3000)),
        FoxPose::Tired => Some((2000, 4000)),
        FoxPose::Speaking => Some((2500, 5000)),
        _ if skin.pose(pose).blink.is_empty() => None,
        FoxPose::Idle | FoxPose::Curious | FoxPose::Listening | FoxPose::Charging => {
            Some((2500, 6000))
        }
        // Wide eyes in the dark hardly blink.
        FoxPose::Peering => Some((3500, 7000)),
        _ => Some((3000, 6000)),
    }
}

const BLINK_MS: u32 = 150;
/// Old pose stays up this long into a switch, at the deepest squash.
const SWAP_MS: u32 = 60;
const POP_MS: u32 = 600;

/// External pose input from the platform (entrance, tap pulse, gaze).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoxInput {
    pub scale_x: f32,
    pub scale_y: f32,
    /// Lean in canvas pixels (petting).
    pub offset_x: i32,
    pub offset_y: i32,
    /// Personality gaze toward the finger, source units (about -10..10).
    pub look_x: f32,
}

impl FoxInput {
    pub const REST: Self = Self {
        scale_x: 1.0,
        scale_y: 1.0,
        offset_x: 0,
        offset_y: 0,
        look_x: 0.0,
    };
}

/// Expression player: time, blinks and switch transitions.
pub struct FoxAnimator {
    skin: Rc<Skin>,
    pose: FoxPose,
    prev: Option<FoxPose>,
    /// Milliseconds since `pose` was entered.
    t: u32,
    rng: u32,
    next_blink: u32,
    /// Next idle extra (gaze drift / ear flick), ms since entering.
    next_extra: u32,
    extra_kind: u8,
    extra_until: u32,
}

impl FoxAnimator {
    pub fn new(pose: FoxPose) -> Self {
        Self::with_skin(pose, Skin::builtin())
    }

    /// Start with one resource before scheduling the first seeded animation.
    pub fn with_skin(pose: FoxPose, skin: Rc<Skin>) -> Self {
        let mut a = Self {
            skin,
            pose,
            prev: None,
            t: POP_MS,
            rng: 0x2545_f491,
            next_blink: 0,
            next_extra: 0,
            extra_kind: 0,
            extra_until: 0,
        };
        a.schedule();
        a
    }

    pub fn set_skin(&mut self, skin: Rc<Skin>) {
        if !Skin::same_resources(&self.skin, &skin) {
            self.skin = skin;
            self.prev = None;
            self.schedule();
        }
    }

    pub fn pose(&self) -> FoxPose {
        self.pose
    }

    fn rand(&mut self, lo: u32, hi: u32) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        lo + self.rng % (hi - lo).max(1)
    }

    fn schedule(&mut self) {
        let t = self.t;
        self.next_blink = match blink_range(self.pose, &self.skin) {
            Some((lo, hi)) => t + self.rand(lo, hi),
            None => u32::MAX,
        };
        self.next_extra = t + self.rand(2500, 5000);
    }

    /// Switch expression; the old pose shows briefly under a squash.
    pub fn set_pose(&mut self, pose: FoxPose) {
        if pose != self.pose {
            self.prev = Some(self.pose);
            self.pose = pose;
            self.t = 0;
            self.extra_until = 0;
            self.schedule();
        }
    }

    pub fn advance(&mut self, dt_ms: u32) {
        self.t = self.t.saturating_add(dt_ms.min(1000));
        if self.t >= self.next_blink.saturating_add(BLINK_MS) {
            if let Some((lo, hi)) = blink_range(self.pose, &self.skin) {
                self.next_blink = self.t + self.rand(lo, hi);
                // Occasional double blink.
                if self.rand(0, 5) == 0 {
                    self.next_blink = self.t + 120;
                }
            }
        }
        if self.t >= self.next_extra {
            self.extra_kind = self.rand(0, 3) as u8;
            self.extra_until = self.t + if self.extra_kind == 2 { 300 } else { 1100 };
            self.next_extra = self.t + self.rand(3500, 7000);
        }
    }

    fn blinking(&self) -> bool {
        self.t >= self.next_blink && self.t < self.next_blink + BLINK_MS
    }

    /// Build the scene for the current time.
    pub fn scene(&self, input: FoxInput) -> Scene {
        let t = self.t;
        let shown = match self.prev {
            Some(prev) if t < SWAP_MS => prev,
            _ => self.pose,
        };
        let pose = self.skin.pose(shown);
        let mut s = Scene::with_skin(shown, Rc::clone(&self.skin));

        let (period, depth) = breath(shown);
        let b = sin(TAU * phase(t, period, 0));
        let mut sx = 1.0 - depth * 0.5 * b;
        let mut sy = 1.0 + depth * b;
        let mut dx = 0i32;
        let mut dy = 0i32;

        // Switch pop: damped spring squash, pose swaps at the deepest point.
        if self.prev.is_some() && t < POP_MS {
            let tt = t as f32 / 1000.0;
            let amp = match shown {
                FoxPose::Surprised => -0.10,
                // Recoil from the light: a deeper squash.
                FoxPose::Dazzled => 0.18,
                _ => 0.12,
            };
            let k = amp * exp_neg(tt / 0.10) * cos(TAU * tt / 0.24);
            sy -= k;
            sx += k * 0.6;
        }

        if self.blinking() && shown == self.pose {
            s.under.extend(pose.blink);
        }

        match shown {
            FoxPose::Idle => {
                if t < self.extra_until && !self.blinking() {
                    match self.extra_kind {
                        0 => s.under.extend(pose.gaze_left),
                        1 => s.under.extend(pose.gaze_right),
                        _ => {
                            if (self.extra_until - t) % 300 > 150 {
                                s.under.extend(pose.ear);
                            }
                        }
                    }
                } else if !self.blinking() {
                    // Follow the finger with the gaze patches.
                    if input.look_x < -3.0 {
                        s.under.extend(pose.gaze_left);
                    } else if input.look_x > 3.0 {
                        s.under.extend(pose.gaze_right);
                    }
                }
            }
            FoxPose::Listening => {
                if t < self.extra_until {
                    s.under.extend(pose.ear);
                }
            }
            FoxPose::Thinking => {
                let lit = (t / 300) % 5;
                for (n, img) in pose.symbols.iter().enumerate() {
                    if (n as u32) < lit {
                        s.over.push(Layer::at(img));
                    }
                }
            }
            FoxPose::Speaking => {
                if t % 4000 < 2800 {
                    match (t / 120) % 4 {
                        0 => s.under.extend(pose.mouth_closed),
                        2 => s.under.extend(pose.mouth_wide),
                        _ => {}
                    }
                } else {
                    s.under.extend(pose.mouth_closed);
                }
            }
            FoxPose::Sleep => {
                for (n, img) in pose.symbols.iter().enumerate() {
                    let ph = phase(t, 2400, 2400 - (n as u32 * 600) % 2400);
                    let fade = if ph < 0.05 {
                        0.0
                    } else {
                        (3.0 * (1.0 - ph)).min(1.0)
                    };
                    s.over.push(Layer {
                        img: *img,
                        dx: (ph * 6.0) as i16,
                        dy: -(ph * 24.0) as i16,
                        scale_q8: 256,
                        fade: (fade * 16.0) as u8,
                    });
                }
            }
            FoxPose::Panic => {
                dx = if (t / 50) % 2 == 0 { 3 } else { -3 };
                for (n, img) in pose.symbols.iter().enumerate() {
                    let ph = phase(t, 700, n as u32 * 130);
                    let (cx, cy) = img.centre();
                    let (vx, vy) = (cx - W as f32 / 2.0, cy - H as f32 * 0.55);
                    let d = (vx * vx + vy * vy).max(1.0);
                    let inv = inv_sqrt(d);
                    let layer = Layer {
                        img: *img,
                        dx: (vx * inv * 12.0 * ph) as i16,
                        dy: (vy * inv * 12.0 * ph) as i16,
                        scale_q8: 256,
                        fade: ((1.0 - ph * ph) * 16.0) as u8,
                    };
                    if img.over_bg {
                        s.over.push(layer);
                    } else {
                        s.under.push(Layer {
                            dx: 0,
                            dy: 0,
                            fade: 16,
                            ..layer
                        });
                    }
                }
                let on = (t / 120) % 2 == 0;
                for img in pose.tremble {
                    s.over.push(Layer {
                        dx: if on { 1 } else { -1 },
                        fade: if on { 16 } else { 10 },
                        ..Layer::at(img)
                    });
                }
            }
            FoxPose::Charging => {
                for (n, img) in pose.symbols.iter().enumerate() {
                    if n == 0 {
                        let bob = round(4.0 * sin(TAU * phase(t, 1200, 0)));
                        s.over.push(Layer {
                            dy: bob as i16,
                            ..Layer::at(img)
                        });
                    } else if (t / 250 + n as u32 * 7 / 4) % 4 < 3 {
                        s.over.push(Layer::at(img));
                    }
                }
            }
            FoxPose::Confused => {
                let frame = (t / 100) % 4;
                if frame > 0 {
                    s.under.extend(pose.spiral[frame as usize - 1]);
                }
                dx = round(2.0 * sin(TAU * phase(t, 2000, 0)));
                if (t / 500) % 2 == 0 {
                    for img in pose.symbols {
                        if img.over_bg {
                            s.over.push(Layer::at(img));
                        } else {
                            s.under.push(Layer::at(img));
                        }
                    }
                }
            }
            FoxPose::Angry => {
                let big = (t / 400) % 2 == 0;
                for img in pose.symbols {
                    s.under.push(Layer {
                        scale_q8: if big { 302 } else { 236 },
                        ..Layer::at(img)
                    });
                }
                let st = t % 1500;
                if st < 300 {
                    let k = 0.06 * sin(PI * st as f32 / 300.0);
                    sy -= k;
                    sx += k * 0.6;
                    dx = if (st / 35) % 2 == 0 { 2 } else { -2 };
                }
            }
            FoxPose::Puzzled => {
                let bob = round(-4.0 * (0.5 - 0.5 * cos(TAU * phase(t, 1200, 0))));
                for img in pose.symbols {
                    s.over.push(Layer {
                        dy: bob as i16,
                        ..Layer::at(img)
                    });
                }
            }
            FoxPose::Annoyed => {
                let ph = phase(t, 3000, 0);
                for img in pose.symbols {
                    let layer = Layer {
                        dx: (ph * 10.0) as i16,
                        dy: -(ph * 4.0) as i16,
                        fade: ((1.0 - ph).min(1.0) * 16.0) as u8,
                        ..Layer::at(img)
                    };
                    if img.over_bg {
                        s.over.push(layer);
                    } else {
                        s.under.push(Layer {
                            fade: 16,
                            dx: 0,
                            dy: 0,
                            ..layer
                        });
                    }
                }
                if ph < 0.12 {
                    let k = 0.04 * sin(PI * ph / 0.12);
                    sy -= k;
                    sx += k * 0.6;
                }
            }
            FoxPose::Happy => {
                // Three hops after the pop, then a hop every 3 s.
                let lt = if t < 2000 { t } else { (t - 2000) % 3000 + 350 };
                if (350..1350).contains(&lt) {
                    let ph = (lt - 350) % 550;
                    if ph < 400 {
                        dy = -round(18.0 * sin(PI * ph as f32 / 400.0));
                    } else {
                        let k = 0.08 * sin(PI * (ph - 400) as f32 / 150.0);
                        sy -= k;
                        sx += k * 0.6;
                    }
                }
            }
            FoxPose::MusicPlaying => {
                dx = round(3.0 * sin(TAU * phase(t, 1200, 0)));
                dy = -round(3.0 * (0.5 + 0.5 * sin(TAU * phase(t, 600, 0))));
            }
            FoxPose::NetworkDisconnected
            | FoxPose::NetworkConnected
            | FoxPose::BluetoothDisconnected
            | FoxPose::BluetoothConnected => {}
            FoxPose::Satisfied => {
                dx = round(2.0 * sin(TAU * phase(t, 3600, 0)));
            }
            FoxPose::Error => {
                if t < 300 {
                    dy = -round(20.0 * (1.0 - t as f32 / 300.0));
                } else if t % 2000 < 200 {
                    dx = if (t / 40) % 2 == 0 { 2 } else { -2 };
                }
            }
            FoxPose::Tired => {
                let nt = t % 4000;
                if nt < 600 {
                    dy = round(6.0 * sin(PI * nt as f32 / 600.0));
                }
            }
            FoxPose::Peering => {
                // Look around in the dark: left, ahead, right, ahead.
                let st = t % 4800;
                if !self.blinking() {
                    if (700..1700).contains(&st) {
                        s.under.extend(pose.gaze_left);
                    } else if (3100..4100).contains(&st) {
                        s.under.extend(pose.gaze_right);
                    }
                }
                // Straining forward: a slow, shallow lean.
                let k = 0.012 * (0.5 - 0.5 * cos(TAU * phase(t, 4800, 0)));
                sy -= k;
                sx += k * 0.5;
            }
            FoxPose::Dazzled => {
                // A wincing tremble right after the flash, then a flinch
                // every 2 s while the light still stings.
                if t < 900 {
                    dx = if (t / 60) % 2 == 0 { 1 } else { -1 };
                } else if t % 2000 < 160 {
                    let k = 0.04 * sin(PI * (t % 2000) as f32 / 160.0);
                    sy -= k;
                    sx += k * 0.6;
                }
            }
            FoxPose::Curious | FoxPose::Surprised | FoxPose::Dying => {}
        }

        sx *= input.scale_x;
        sy *= input.scale_y;
        s.warp = Warp {
            sx: clamp(sx, 0.5, 1.2),
            sy: clamp(sy, 0.5, 1.2),
            dx: dx + input.offset_x,
            dy: dy + input.offset_y,
        };
        s
    }
}
