//! What one frame shows: the pose, its parts, and the whole-canvas warp.
//!
//! [`super::FoxAnimator`] produces a [`Scene`]; [`super::FoxRenderer`] draws
//! it. Nothing here allocates, so a scene can be built every frame.

use super::{FoxPose, Img, H, W};

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
        Self {
            img,
            dx: 0,
            dy: 0,
            scale_q8: 256,
            fade: 16,
        }
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
        Self {
            items: [None; MAX_LAYERS],
            n: 0,
        }
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
    pub(super) fn signature(&self, mut h: u64) -> u64 {
        for l in self.iter() {
            for v in [
                l.img.off as u64,
                l.dx as u64,
                l.dy as u64,
                l.scale_q8 as u64,
                l.fade as u64,
            ] {
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
    pub const IDENTITY: Self = Self {
        sx: 1.0,
        sy: 1.0,
        dx: 0,
        dy: 0,
    };

    /// Where a source point lands on the output canvas.
    pub(super) fn forward(&self, x: f32, y: f32) -> (f32, f32) {
        let ax = W as f32 / 2.0;
        let ay = (H - 1) as f32;
        (
            ax + (x - ax) * self.sx + self.dx as f32,
            ay - (ay - y) * self.sy + self.dy as f32,
        )
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
        Self {
            pose,
            under: Layers::new(),
            warp: Warp::IDENTITY,
            over: Layers::new(),
        }
    }
}
