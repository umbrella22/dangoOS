//! Scene to RGB565: compose palette indices, warp, look up the palette.
//!
//! 1. The base pose and the "under" layers are decoded into an index buffer.
//!    This step is skipped when the layer list did not change (signature
//!    cache), which is the common case while the fox only breathes.
//! 2. The index buffer is warped (nearest neighbour, about the bottom centre)
//!    and looked up in the palette into the output canvas.
//! 3. The "over" layers (Zzz, sweat, sparks) are stamped on top at their
//!    warped position, with a Bayer stipple instead of alpha for fades.

use alloc::vec;
use alloc::vec::Vec;

use super::math::round;
use super::scene::{Layer, Scene, Warp};
use super::{data, Img, H, PIXELS, TRANSPARENT, W};

/// 4x4 ordered-dither thresholds for [`Layer::fade`].
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
