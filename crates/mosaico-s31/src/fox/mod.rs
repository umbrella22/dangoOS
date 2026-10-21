//! Fox-dango character: palette sprites instead of the vector ball.
//!
//! The art lives in `assets/fox-dango/` and is packed by
//! `assets/fox-dango/tools/fox_pack.py` into [`data`]: one 128-colour RGB565
//! palette, 18 base poses and their parts, all as row RLE of palette indices.
//!
//! Every frame is composed in the order the asset README documents:
//!
//! 1. decode the base pose into an index buffer;
//! 2. stamp patches (blink, mouth, ears, gaze, spiral) and body sprites;
//! 3. nearest-neighbour warp (breath, pop, hop, shake, entrance) through the
//!    palette into the RGB565 canvas;
//! 4. stamp background sprites (Zzz, sweat, sparks) after the warp, so they
//!    move on their own and can stipple-fade without alpha blending.
//!
//! Step 1–2 are skipped when the composition did not change since the previous
//! frame, which is the common case (only the warp changes while breathing).
//!
//! [`FoxAnimator`] turns an expression plus time into a [`Scene`]; the
//! behaviour per expression follows `assets/fox-dango/tools/fox_anim.py`.

use alloc::vec;
use alloc::vec::Vec;

use grok_ball::types::BallExpression;

pub mod data;

pub use data::{H, W};

/// Canvas pixel count.
pub const PIXELS: usize = W * H;
/// Transparent palette index in sprite RLE.
pub const TRANSPARENT: u8 = 255;

/// One RLE image placed on the canvas.
#[derive(Clone, Copy, Debug)]
pub struct Img {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
    pub off: u32,
    pub len: u32,
    /// Sprite over the black background (moves after the warp).
    pub over_bg: bool,
}

impl Img {
    fn rle(&self) -> &'static [u8] {
        &data::BLOB[self.off as usize..(self.off + self.len) as usize]
    }

    fn centre(&self) -> (f32, f32) {
        (self.x as f32 + self.w as f32 / 2.0, self.y as f32 + self.h as f32 / 2.0)
    }
}

/// One expression: base pose and its parts.
pub struct Pose {
    pub base: Img,
    pub blink: &'static [Img],
    pub mouth_closed: &'static [Img],
    pub mouth_wide: &'static [Img],
    pub ear: &'static [Img],
    pub gaze_left: &'static [Img],
    pub gaze_right: &'static [Img],
    pub symbols: &'static [Img],
    pub tremble: &'static [Img],
    pub spiral: [&'static [Img]; 3],
}

/// The 18 fox poses, in the order of [`data::POSES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FoxPose {
    Idle,
    Sleep,
    Curious,
    Happy,
    Puzzled,
    Confused,
    Surprised,
    Panic,
    Angry,
    Satisfied,
    Listening,
    Thinking,
    Speaking,
    Error,
    Tired,
    Dying,
    Annoyed,
    Charging,
}

impl FoxPose {
    pub fn data(self) -> &'static Pose {
        &data::POSES[self as usize]
    }
}

impl From<BallExpression> for FoxPose {
    fn from(e: BallExpression) -> Self {
        match e {
            BallExpression::Idle => Self::Idle,
            BallExpression::Sleep => Self::Sleep,
            BallExpression::Curious => Self::Curious,
            BallExpression::Happy => Self::Happy,
            BallExpression::Puzzled => Self::Puzzled,
            BallExpression::Confused => Self::Confused,
            BallExpression::Surprised => Self::Surprised,
            BallExpression::Panic => Self::Panic,
            BallExpression::Angry => Self::Angry,
            BallExpression::Satisfied => Self::Satisfied,
            BallExpression::Listening => Self::Listening,
            BallExpression::Thinking => Self::Thinking,
            BallExpression::Speaking => Self::Speaking,
            BallExpression::Error => Self::Error,
            BallExpression::Tired => Self::Tired,
            BallExpression::Dying => Self::Dying,
        }
    }
}

/// A part placed in a scene.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub img: &'static Img,
    pub dx: i16,
    pub dy: i16,
    /// Nearest-neighbour scale about the part centre, 1/256 units.
    pub scale_q8: u16,
    /// Visible fraction 0..=16 (16 = opaque); applied as a 4x4 Bayer stipple.
    pub fade: u8,
}

impl Layer {
    pub fn at(img: &'static Img) -> Self {
        Self { img, dx: 0, dy: 0, scale_q8: 256, fade: 16 }
    }
}

const MAX_LAYERS: usize = 24;

/// Fixed-capacity layer list (no allocation per frame).
#[derive(Clone, Copy)]
pub struct Layers {
    items: [Option<Layer>; MAX_LAYERS],
    n: usize,
}

impl Layers {
    pub const fn new() -> Self {
        Self { items: [None; MAX_LAYERS], n: 0 }
    }

    pub fn push(&mut self, layer: Layer) {
        if self.n < MAX_LAYERS && layer.fade > 0 {
            self.items[self.n] = Some(layer);
            self.n += 1;
        }
    }

    pub fn extend(&mut self, imgs: &'static [Img]) {
        for img in imgs {
            self.push(Layer::at(img));
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Layer> {
        self.items[..self.n].iter().flatten()
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Cheap identity of the list, used to skip recomposition.
    fn signature(&self, mut h: u64) -> u64 {
        for l in self.iter() {
            for v in [l.img.off as u64, l.dx as u64, l.dy as u64, l.scale_q8 as u64, l.fade as u64] {
                h = (h ^ v).wrapping_mul(0x100_0000_01b3);
            }
        }
        h
    }
}

impl Default for Layers {
    fn default() -> Self {
        Self::new()
    }
}

/// Whole-canvas nearest-neighbour warp about the bottom centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub sx: f32,
    pub sy: f32,
    pub dx: i32,
    pub dy: i32,
}

impl Warp {
    pub const IDENTITY: Self = Self { sx: 1.0, sy: 1.0, dx: 0, dy: 0 };

    /// Where a source point lands on the output canvas.
    fn forward(&self, x: f32, y: f32) -> (f32, f32) {
        let ax = W as f32 / 2.0;
        let ay = (H - 1) as f32;
        (ax + (x - ax) * self.sx + self.dx as f32, ay - (ay - y) * self.sy + self.dy as f32)
    }
}

/// Everything needed to draw one frame.
#[derive(Clone, Copy)]
pub struct Scene {
    pub pose: FoxPose,
    /// Patches and body sprites, stamped before the warp.
    pub under: Layers,
    pub warp: Warp,
    /// Background sprites, stamped after the warp.
    pub over: Layers,
}

impl Scene {
    pub fn still(pose: FoxPose) -> Self {
        Self { pose, under: Layers::new(), warp: Warp::IDENTITY, over: Layers::new() }
    }
}

const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Decode `img` into a `stride`-wide index buffer at (`x0`, `y0`), clipped,
/// skipping transparent runs.
fn stamp(buf: &mut [u8], stride: usize, rows: usize, img: &Img, x0: i32, y0: i32) {
    let rle = img.rle();
    let w = img.w as i32;
    let mut i = 0;
    for row in 0..img.h as i32 {
        let y = y0 + row;
        let mut x = 0i32;
        while x < w && i + 1 < rle.len() {
            let n = rle[i] as i32;
            let v = rle[i + 1];
            i += 2;
            if v != TRANSPARENT && y >= 0 && (y as usize) < rows {
                let a = (x0 + x).max(0);
                let b = (x0 + x + n).min(stride as i32);
                if a < b {
                    let base = y as usize * stride;
                    buf[base + a as usize..base + b as usize].fill(v);
                }
            }
            x += n;
        }
    }
}

/// Composes scenes into RGB565 canvases.
pub struct FoxRenderer {
    idx: Vec<u8>,
    scratch: Vec<u8>,
    signature: u64,
    /// Frames that reused the composed index buffer.
    pub reused: u32,
}

impl FoxRenderer {
    pub fn new() -> Self {
        Self { idx: vec![0; PIXELS], scratch: Vec::new(), signature: 0, reused: 0 }
    }

    fn compose(&mut self, scene: &Scene) {
        let sig = scene.under.signature(0xcbf2_9ce4_8422_2325 ^ scene.pose as u64);
        if sig == self.signature {
            self.reused = self.reused.wrapping_add(1);
            return;
        }
        self.signature = sig;
        let pose = scene.pose.data();
        stamp(&mut self.idx, W, H, &pose.base, 0, 0);
        for l in scene.under.iter() {
            self.layer_into_idx(l);
        }
    }

    fn layer_into_idx(&mut self, l: &Layer) {
        let img = l.img;
        if l.scale_q8 == 256 && l.fade >= 16 {
            stamp(&mut self.idx, W, H, img, img.x as i32 + l.dx as i32, img.y as i32 + l.dy as i32);
            return;
        }
        let (w, h) = (img.w as usize, img.h as usize);
        self.scratch.clear();
        self.scratch.resize(w * h, TRANSPARENT);
        stamp(&mut self.scratch, w, h, img, 0, 0);
        let s = l.scale_q8.max(16) as i32;
        let nw = (w as i32 * s + 128) >> 8;
        let nh = (h as i32 * s + 128) >> 8;
        let x0 = img.x as i32 + l.dx as i32 - (nw - w as i32) / 2;
        let y0 = img.y as i32 + l.dy as i32 - (nh - h as i32) / 2;
        for oy in 0..nh {
            let y = y0 + oy;
            if y < 0 || y >= H as i32 {
                continue;
            }
            let sy = ((oy << 8) / s).min(h as i32 - 1) as usize;
            for ox in 0..nw {
                let x = x0 + ox;
                if x < 0 || x >= W as i32 {
                    continue;
                }
                if l.fade < 16 && BAYER[(y & 3) as usize][(x & 3) as usize] >= l.fade {
                    continue;
                }
                let sx = ((ox << 8) / s).min(w as i32 - 1) as usize;
                let v = self.scratch[sy * w + sx];
                if v != TRANSPARENT {
                    self.idx[y as usize * W + x as usize] = v;
                }
            }
        }
    }

    /// Draw `scene` into `out` (`W * H` RGB565 pixels).
    pub fn render(&mut self, scene: &Scene, out: &mut [u16]) {
        assert!(out.len() >= PIXELS, "fox canvas too short");
        self.compose(scene);
        let pal = &data::PALETTE;
        let lookup = |v: u8| -> u16 { if (v as usize) < pal.len() { pal[v as usize] } else { 0 } };

        let warp = scene.warp;
        if warp == Warp::IDENTITY {
            for (o, &v) in out[..PIXELS].iter_mut().zip(self.idx.iter()) {
                *o = lookup(v);
            }
        } else {
            let ax = W as f32 / 2.0;
            let ay = (H - 1) as f32;
            let mut xlut = [0i16; W];
            for (x, slot) in xlut.iter_mut().enumerate() {
                let sx = round(ax + (x as f32 - warp.dx as f32 - ax) / warp.sx);
                *slot = if sx >= 0 && sx < W as i32 { sx as i16 } else { -1 };
            }
            for y in 0..H {
                let row = &mut out[y * W..(y + 1) * W];
                let sy = round(ay - (ay - (y as f32 - warp.dy as f32)) / warp.sy);
                if sy < 0 || sy >= H as i32 {
                    row.fill(0);
                    continue;
                }
                let src = &self.idx[sy as usize * W..(sy as usize + 1) * W];
                for (o, &sx) in row.iter_mut().zip(xlut.iter()) {
                    *o = if sx >= 0 { lookup(src[sx as usize]) } else { 0 };
                }
            }
        }

        for l in scene.over.iter() {
            let img = l.img;
            let (fx, fy) = warp.forward(img.x as f32 + l.dx as f32, img.y as f32 + l.dy as f32);
            let (x0, y0) = (round(fx), round(fy));
            self.scratch.clear();
            self.scratch.resize(img.w as usize * img.h as usize, TRANSPARENT);
            stamp(&mut self.scratch, img.w as usize, img.h as usize, img, 0, 0);
            for sy in 0..img.h as i32 {
                let y = y0 + sy;
                if y < 0 || y >= H as i32 {
                    continue;
                }
                for sx in 0..img.w as i32 {
                    let x = x0 + sx;
                    if x < 0 || x >= W as i32 {
                        continue;
                    }
                    if l.fade < 16 && BAYER[(y & 3) as usize][(x & 3) as usize] >= l.fade {
                        continue;
                    }
                    let v = self.scratch[sy as usize * img.w as usize + sx as usize];
                    if v != TRANSPARENT {
                        out[y as usize * W + x as usize] = lookup(v);
                    }
                }
            }
        }
    }
}

impl Default for FoxRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn round(v: f32) -> i32 {
    if v >= 0.0 {
        (v + 0.5) as i32
    } else {
        -((-v + 0.5) as i32)
    }
}

// ---------------------------------------------------------------- animation

const PI: f32 = core::f32::consts::PI;
const TAU: f32 = core::f32::consts::TAU;

/// Sine without libm (error below 1e-3).
fn sin(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let mut x = x % TAU;
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }
    // Bhaskara-style parabola with one refinement step.
    let y = 4.0 / PI * x - 4.0 / (PI * PI) * x * if x < 0.0 { -x } else { x };
    0.225 * (y * if y < 0.0 { -y } else { y } - y) + y
}

fn cos(x: f32) -> f32 {
    sin(x + PI / 2.0)
}

/// `exp(-x)` for `x >= 0` without libm.
fn exp_neg(x: f32) -> f32 {
    let t = 1.0 + x.max(0.0) / 16.0;
    let t2 = t * t;
    let t4 = t2 * t2;
    let t8 = t4 * t4;
    1.0 / (t8 * t8)
}

/// Phase in `0..1` of `t` within a repeating `period`.
fn phase(t: u32, period: u32, offset: u32) -> f32 {
    ((t + offset) % period) as f32 / period as f32
}

/// Breathing period (ms) and depth (fraction) per pose; 0 = none.
fn breath(pose: FoxPose) -> (u32, f32) {
    match pose {
        FoxPose::Sleep => (4500, 0.025),
        FoxPose::Puzzled | FoxPose::Thinking => (3600, 0.012),
        FoxPose::Satisfied => (3600, 0.015),
        FoxPose::Panic => (600, 0.02),
        FoxPose::Tired => (4000, 0.015),
        FoxPose::Dying => (6000, 0.008),
        FoxPose::Happy | FoxPose::Surprised | FoxPose::Error => (3000, 0.01),
        _ => (3000, 0.015),
    }
}

/// Blink interval range in ms per pose; `None` for closed-eye poses.
fn blink_range(pose: FoxPose) -> Option<(u32, u32)> {
    match pose {
        FoxPose::Panic => Some((1500, 3000)),
        FoxPose::Tired => Some((2000, 4000)),
        FoxPose::Speaking => Some((2500, 5000)),
        _ if pose.data().blink.is_empty() => None,
        FoxPose::Idle | FoxPose::Curious | FoxPose::Listening | FoxPose::Charging => Some((2500, 6000)),
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
    pub offset_y: i32,
    /// Personality gaze toward the finger, source units (about -10..10).
    pub look_x: f32,
}

impl FoxInput {
    pub const REST: Self = Self { scale_x: 1.0, scale_y: 1.0, offset_y: 0, look_x: 0.0 };
}

/// Expression player: time, blinks and switch transitions.
pub struct FoxAnimator {
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
        let mut a = Self {
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
        self.next_blink = match blink_range(self.pose) {
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
            if let Some((lo, hi)) = blink_range(self.pose) {
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
        let pose = shown.data();
        let mut s = Scene::still(shown);

        let (period, depth) = breath(shown);
        let b = sin(TAU * phase(t, period, 0));
        let mut sx = 1.0 - depth * 0.5 * b;
        let mut sy = 1.0 + depth * b;
        let mut dx = 0i32;
        let mut dy = 0i32;

        // Switch pop: damped spring squash, pose swaps at the deepest point.
        if self.prev.is_some() && t < POP_MS {
            let tt = t as f32 / 1000.0;
            let amp = if shown == FoxPose::Surprised { -0.10 } else { 0.12 };
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
                    let fade = if ph < 0.05 { 0.0 } else { (3.0 * (1.0 - ph)).min(1.0) };
                    s.over.push(Layer {
                        img,
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
                        img,
                        dx: (vx * inv * 12.0 * ph) as i16,
                        dy: (vy * inv * 12.0 * ph) as i16,
                        scale_q8: 256,
                        fade: ((1.0 - ph * ph) * 16.0) as u8,
                    };
                    if img.over_bg {
                        s.over.push(layer);
                    } else {
                        s.under.push(Layer { dx: 0, dy: 0, fade: 16, ..layer });
                    }
                }
                let on = (t / 120) % 2 == 0;
                for img in pose.tremble {
                    s.over.push(Layer { dx: if on { 1 } else { -1 }, fade: if on { 16 } else { 10 }, ..Layer::at(img) });
                }
            }
            FoxPose::Charging => {
                for (n, img) in pose.symbols.iter().enumerate() {
                    if n == 0 {
                        let bob = round(4.0 * sin(TAU * phase(t, 1200, 0)));
                        s.over.push(Layer { dy: bob as i16, ..Layer::at(img) });
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
                    s.under.push(Layer { scale_q8: if big { 302 } else { 236 }, ..Layer::at(img) });
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
                    s.over.push(Layer { dy: bob as i16, ..Layer::at(img) });
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
                        s.under.push(Layer { fade: 16, dx: 0, dy: 0, ..layer });
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
            FoxPose::Curious | FoxPose::Surprised | FoxPose::Dying => {}
        }

        sx *= input.scale_x;
        sy *= input.scale_y;
        s.warp = Warp { sx: clamp(sx, 0.5, 1.2), sy: clamp(sy, 0.5, 1.2), dx, dy: dy + input.offset_y };
        s
    }
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if !v.is_finite() {
        1.0
    } else if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `1 / sqrt(x)` for `x > 0`, two Newton steps.
fn inv_sqrt(x: f32) -> f32 {
    let i = 0x5f37_59df - (x.to_bits() >> 1);
    let mut y = f32::from_bits(i);
    y *= 1.5 - 0.5 * x * y * y;
    y *= 1.5 - 0.5 * x * y * y;
    y
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn every_image_decodes_to_its_size() {
        for (n, pose) in data::POSES.iter().enumerate() {
            let mut all: Vec<&Img> = vec![&pose.base];
            for list in [pose.blink, pose.mouth_closed, pose.mouth_wide, pose.ear, pose.gaze_left,
                         pose.gaze_right, pose.symbols, pose.tremble]
                .into_iter()
                .chain(pose.spiral)
            {
                all.extend(list.iter());
            }
            for img in all {
                let rle = img.rle();
                assert_eq!(rle.len() % 2, 0);
                let mut px = 0usize;
                let mut x = 0usize;
                for pair in rle.chunks_exact(2) {
                    assert!(pair[0] > 0);
                    assert!(pair[1] == TRANSPARENT || (pair[1] as usize) < data::PALETTE.len());
                    x += pair[0] as usize;
                    assert!(x <= img.w as usize, "pose {n}: run crosses the row end");
                    if x == img.w as usize {
                        x = 0;
                    }
                    px += pair[0] as usize;
                }
                assert_eq!(px, img.w as usize * img.h as usize, "pose {n}");
                assert!(img.x >= 0 && img.y >= 0);
                assert!(img.x as usize + img.w as usize <= W && img.y as usize + img.h as usize <= H);
            }
        }
    }

    #[test]
    fn base_poses_are_opaque_and_canvas_sized() {
        for pose in &data::POSES {
            assert_eq!((pose.base.w as usize, pose.base.h as usize), (W, H));
            assert!(!pose.base.rle().chunks_exact(2).any(|p| p[1] == TRANSPARENT));
        }
    }

    #[test]
    fn identity_scene_matches_the_palette() {
        let mut r = FoxRenderer::new();
        let mut out = vec![0u16; PIXELS];
        r.render(&Scene::still(FoxPose::Idle), &mut out);
        assert!(out.iter().any(|&p| p != 0), "the fox is drawn");
        assert!(out.iter().all(|p| *p == 0 || data::PALETTE.contains(p)));
    }

    #[test]
    fn a_blink_changes_only_the_eye_patches() {
        let mut r = FoxRenderer::new();
        let mut open = vec![0u16; PIXELS];
        let mut shut = vec![0u16; PIXELS];
        r.render(&Scene::still(FoxPose::Idle), &mut open);
        let mut s = Scene::still(FoxPose::Idle);
        s.under.extend(FoxPose::Idle.data().blink);
        r.render(&s, &mut shut);
        let blink = FoxPose::Idle.data().blink;
        let mut changed = 0;
        for y in 0..H {
            for x in 0..W {
                if open[y * W + x] != shut[y * W + x] {
                    changed += 1;
                    assert!(blink.iter().any(|b| {
                        (b.x as usize..b.x as usize + b.w as usize).contains(&x)
                            && (b.y as usize..b.y as usize + b.h as usize).contains(&y)
                    }));
                }
            }
        }
        assert!(changed > 200);
    }

    #[test]
    fn unchanged_composition_is_reused() {
        let mut r = FoxRenderer::new();
        let mut out = vec![0u16; PIXELS];
        let mut a = FoxAnimator::new(FoxPose::Idle);
        for _ in 0..10 {
            a.advance(50);
            r.render(&a.scene(FoxInput::REST), &mut out);
        }
        assert!(r.reused > 0);
    }

    #[test]
    fn every_pose_animates_for_ten_seconds() {
        let mut r = FoxRenderer::new();
        let mut out = vec![0u16; PIXELS];
        for pose in 0..18u8 {
            // SAFETY-free conversion through the table index.
            let p = [FoxPose::Idle, FoxPose::Sleep, FoxPose::Curious, FoxPose::Happy, FoxPose::Puzzled,
                     FoxPose::Confused, FoxPose::Surprised, FoxPose::Panic, FoxPose::Angry,
                     FoxPose::Satisfied, FoxPose::Listening, FoxPose::Thinking, FoxPose::Speaking,
                     FoxPose::Error, FoxPose::Tired, FoxPose::Dying, FoxPose::Annoyed,
                     FoxPose::Charging][pose as usize];
            let mut a = FoxAnimator::new(FoxPose::Idle);
            a.set_pose(p);
            for _ in 0..200 {
                a.advance(50);
                let s = a.scene(FoxInput { scale_x: 1.03, scale_y: 0.97, offset_y: -10, look_x: 5.0 });
                r.render(&s, &mut out);
            }
        }
    }

    #[test]
    fn a_switch_shows_the_old_pose_first() {
        let mut a = FoxAnimator::new(FoxPose::Idle);
        a.set_pose(FoxPose::Happy);
        a.advance(10);
        assert_eq!(a.scene(FoxInput::REST).pose, FoxPose::Idle);
        a.advance(100);
        assert_eq!(a.scene(FoxInput::REST).pose, FoxPose::Happy);
    }

    #[test]
    fn trig_helpers_are_close() {
        for i in -40..40 {
            let x = i as f32 * 0.2;
            assert!((sin(x) - std::primitive::f32::sin(x)).abs() < 2e-3, "sin {x}");
        }
        for i in 0..40 {
            let x = i as f32 * 0.25;
            assert!((exp_neg(x) - std::primitive::f32::exp(-x)).abs() < 0.02, "exp {x}");
        }
    }
}
