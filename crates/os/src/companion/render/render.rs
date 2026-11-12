//! Scene to RGB565: compose palette indices, warp, look up the palette.
//!
//! 1. The base pose and the "under" layers are decoded into an index buffer.
//!    This step is skipped when the layer list did not change (signature
//!    cache), which is the common case while the fox only breathes.
//! 2. The index buffer is warped about the bottom centre with bilinear RGB565
//!    sampling. Subpixel breathing must not jump entire rows and columns.
//! 3. The "over" layers (Zzz, sweat, sparks) are stamped on top at their
//!    warped position, with a Bayer stipple instead of alpha for fades.

use crate::companion::render::skin::Skin;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use super::encode_rgb565;
use super::scene::ArticulatedLayer;
use crate::companion::render::math::{cos, round, sin, PI};
use {
    crate::companion::render::scene::Layer, crate::companion::render::scene::Scene,
    crate::companion::render::scene::Warp,
};
use {
    crate::companion::render::Img, crate::companion::render::H, crate::companion::render::PIXELS,
    crate::companion::render::TRANSPARENT, crate::companion::render::W,
};

/// 4x4 ordered-dither thresholds for [`Layer::fade`].
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Separate RGB565 into three lanes with room for a five-bit blend weight.
/// All weighted sums fit in u32 on the MCU; no 64-bit per-pixel arithmetic.
const COLOR_LANES: u32 = 0x07e0_f81f;
fn expanded(color: u16) -> u32 {
    let color = u32::from(color);
    (color | color << 16) & COLOR_LANES
}

fn blend(a: u32, b: u32, weight: u32) -> u32 {
    // Round each channel independently before removing its five guard bits.
    // a + (b - a) * weight / 32 saves a multiply; wrapping changes only
    // discarded high bits after the shift and final lane mask.
    a.wrapping_add(
        b.wrapping_sub(a)
            .wrapping_mul(weight)
            .wrapping_add(0x0200_8010)
            >> 5,
    ) & COLOR_LANES
}

#[derive(Clone, Copy, Default)]
struct Sample {
    left: i16,
    weight: u16,
}

fn sample(position: f32) -> Sample {
    let fixed = round(position * 32.0);
    Sample {
        left: (fixed >> 5) as i16,
        weight: (fixed & 31) as u16,
    }
}

/// Horizontal filtering of one source row. Reused as the next output row's
/// top row during breathing, instead of doing the same palette reads twice.
/// Returns whether the row is entirely black.
fn sample_row(
    indices: &[u8],
    source_y: i32,
    colors: &[u32; 256],
    xlut: &[Sample],
    out: &mut [u32],
) -> bool {
    if source_y < 0 || source_y >= H as i32 {
        out.fill(0);
        return true;
    }
    let row = &indices[source_y as usize * W..(source_y as usize + 1) * W];
    // Characters leave substantial black headroom for expression symbols.
    // Blank rows need neither horizontal filtering nor a vertical pixel loop.
    if row.iter().all(|&v| colors[v as usize] == 0) {
        out.fill(0);
        return true;
    }
    for (o, sx) in out.iter_mut().zip(xlut) {
        let x = i32::from(sx.left);
        let a = row.get(x as usize).map_or(0, |&v| colors[v as usize]);
        let b = row.get((x + 1) as usize).map_or(0, |&v| colors[v as usize]);
        *o = if a == b {
            a
        } else {
            blend(a, b, u32::from(sx.weight))
        };
    }
    false
}

#[cfg(test)]
fn warp(indices: &[u8], palette: &[u16], transform: Warp, out: &mut [u16]) {
    warp_to::<false>(indices, palette, transform, out, (W, H), (0, 0));
}

fn warp_to<const BIG_ENDIAN: bool>(
    indices: &[u8],
    palette: &[u16],
    transform: Warp,
    out: &mut [u16],
    size: (usize, usize),
    origin: (i32, i32),
) {
    if transform.angle_deg != 0.0 || transform.fit != 1.0 {
        warp_rotated::<BIG_ENDIAN>(indices, palette, transform, out, size, origin);
        return;
    }
    let (width, height) = size;
    let (ox, oy) = (origin.0 as usize, origin.1 as usize);
    if transform == Warp::IDENTITY {
        out[..width * height].fill(0);
        for y in 0..H {
            let row = &mut out[(oy + y) * width + ox..(oy + y) * width + ox + W];
            for (o, &v) in row.iter_mut().zip(&indices[y * W..(y + 1) * W]) {
                *o = encode_rgb565::<BIG_ENDIAN>(palette.get(v as usize).copied().unwrap_or(0));
            }
        }
        return;
    }

    // Small fixed tables on the UI stack, with no per-frame heap allocation
    // and no second RGB canvas. Palette indices were checked at skin load.
    let mut colors = [0; 256];
    for (slot, &color) in colors.iter_mut().zip(palette) {
        *slot = expanded(color);
    }
    let ax = W as f32 / 2.0;
    let ay = (H - 1) as f32;
    let mut xlut = [Sample::default(); super::VIEW_W];
    let xlut = &mut xlut[..width];
    for (x, slot) in xlut.iter_mut().enumerate() {
        *slot = sample(ax + (x as f32 - origin.0 as f32 - transform.dx as f32 - ax) / transform.sx);
    }
    // Outside this interval both horizontal taps are out of the source.
    // Clear those black margins directly instead of filtering them in every
    // source and destination row. Keep the original sample quantization.
    let intersects_source = |sx: &Sample| i32::from(sx.left) >= -1 && i32::from(sx.left) < W as i32;
    let Some(x0) = xlut.iter().position(intersects_source) else {
        out[..width * height].fill(0);
        return;
    };
    let x1 = xlut.iter().rposition(intersects_source).unwrap() + 1;
    let xlut = &xlut[x0..x1];
    let mut row_a = [0; super::VIEW_W];
    let mut row_b = [0; super::VIEW_W];
    let (mut a, mut b) = (&mut row_a[..xlut.len()], &mut row_b[..xlut.len()]);
    let (mut a_blank, mut b_blank) = (true, true);
    let mut previous = None;
    for y in 0..height {
        let row = &mut out[y * width..(y + 1) * width];
        let sy =
            sample(ay - (ay - (y as f32 - origin.1 as f32 - transform.dy as f32)) / transform.sy);
        let top = i32::from(sy.left);
        if top < -1 || top >= H as i32 {
            row.fill(0);
            continue;
        }
        match previous {
            Some(p) if p == top => {}
            Some(p) if p + 1 == top => {
                core::mem::swap(&mut a, &mut b);
                core::mem::swap(&mut a_blank, &mut b_blank);
                b_blank = sample_row(indices, top + 1, &colors, xlut, b);
            }
            _ => {
                a_blank = sample_row(indices, top, &colors, xlut, a);
                b_blank = sample_row(indices, top + 1, &colors, xlut, b);
            }
        }
        previous = Some(top);
        if a_blank && b_blank {
            row.fill(0);
            continue;
        }
        row[..x0].fill(0);
        row[x1..].fill(0);
        let row = &mut row[x0..x1];
        for ((o, &top), &bottom) in row.iter_mut().zip(a.iter()).zip(b.iter()) {
            let rgb = if top == bottom {
                top
            } else {
                blend(top, bottom, u32::from(sy.weight))
            };
            *o = encode_rgb565::<BIG_ENDIAN>((rgb | rgb >> 16) as u16);
        }
    }
}

/// Inverse affine sampling. The existing row-cache path remains available for
/// ordinary breathing; rotation adds no canvas and no per-frame allocation.
fn warp_rotated<const BIG_ENDIAN: bool>(
    indices: &[u8],
    palette: &[u16],
    t: Warp,
    out: &mut [u16],
    size: (usize, usize),
    origin: (i32, i32),
) {
    let (width, height) = size;
    let mut colors = [0; 256];
    for (slot, &color) in colors.iter_mut().zip(palette) {
        *slot = expanded(color);
    }
    let (cx, cy, ay) = (W as f32 / 2.0, H as f32 / 2.0, (H - 1) as f32);
    let r = t.angle_deg * PI / 180.0;
    let (c, s) = (cos(r) / t.fit, sin(r) / t.fit);
    let (du, dv) = (c / t.sx, -s / t.sy);
    let color = |x: i32, y: i32| {
        if x < 0 || x >= W as i32 || y < 0 || y >= H as i32 {
            0
        } else {
            colors[indices[y as usize * W + x as usize] as usize]
        }
    };
    out[..width * height].fill(0);
    // The source has large black margins above the ears. Keep the expanded
    // viewport from doubling affine work on those margins. Two source pixels
    // around the silhouette retain the complete bilinear filter footprint.
    let mut occupied = [W, H, 0, 0];
    for (y, row) in indices.chunks_exact(W).enumerate() {
        if let Some(left) = row.iter().position(|&v| colors[v as usize] != 0) {
            let right = row.iter().rposition(|&v| colors[v as usize] != 0).unwrap();
            occupied[0] = occupied[0].min(left);
            occupied[1] = occupied[1].min(y);
            occupied[2] = occupied[2].max(right + 1);
            occupied[3] = y + 1;
        }
    }
    if occupied[2] == 0 {
        return;
    }
    let (l, source_top, r, b) = (
        occupied[0] as f32 - 2.0,
        occupied[1] as f32 - 2.0,
        occupied[2] as f32 + 2.0,
        occupied[3] as f32 + 2.0,
    );
    let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (x, y) in [(l, source_top), (r, source_top), (l, b), (r, b)] {
        let (x, y) = t.forward(x, y);
        bounds[0] = bounds[0].min(x + origin.0 as f32);
        bounds[1] = bounds[1].min(y + origin.1 as f32);
        bounds[2] = bounds[2].max(x + origin.0 as f32);
        bounds[3] = bounds[3].max(y + origin.1 as f32);
    }
    let left = (round(bounds[0]) - 2).clamp(0, width as i32) as usize;
    let right = (round(bounds[2]) + 2).clamp(0, width as i32) as usize;
    let top = (round(bounds[1]) - 2).clamp(0, height as i32) as usize;
    let bottom = (round(bounds[3]) + 2).clamp(0, height as i32) as usize;
    for y in top..bottom {
        let (x0, y0) = (
            left as f32 - origin.0 as f32 - t.dx as f32 - cx,
            y as f32 - origin.1 as f32 - t.dy as f32 - cy,
        );
        let mut u = cx + (x0 * c + y0 * s) / t.sx;
        let mut v = ay + (cy - ay - x0 * s + y0 * c) / t.sy;
        for o in &mut out[y * width + left..y * width + right] {
            let (px, py) = (sample(u), sample(v));
            let (x, y) = (i32::from(px.left), i32::from(py.left));
            let top = blend(color(x, y), color(x + 1, y), u32::from(px.weight));
            let bottom = blend(color(x, y + 1), color(x + 1, y + 1), u32::from(px.weight));
            let rgb = blend(top, bottom, u32::from(py.weight));
            *o = encode_rgb565::<BIG_ENDIAN>((rgb | rgb >> 16) as u16);
            u += du;
            v += dv;
        }
    }
}

/// Decode `img` into a `stride`-wide index buffer at (`x0`, `y0`), clipped,
/// skipping transparent runs.
fn stamp(
    buf: &mut [u8],
    stride: usize,
    rows: usize,
    img: &Img,
    skin: &Skin,
    x0: i32,
    y0: i32,
    decoded: &mut Vec<u8>,
) {
    let rle = &skin.blob()[img.off as usize..img.off as usize + img.len as usize];
    if skin.indexed_zlib() {
        // Reuse one image-sized buffer. All streams and palette indices were
        // checked at load; the package remains immutable throughout rendering.
        decoded.resize(img.w as usize * img.h as usize, TRANSPARENT);
        skin.decode_indexed(img, decoded);
        for row in 0..img.h as usize {
            let y = y0 + row as i32;
            if y < 0 || y as usize >= rows {
                continue;
            }
            let start = row * img.w as usize;
            for x in 0..img.w as usize {
                let px = decoded[start + x];
                let dx = x0 + x as i32;
                if px != TRANSPARENT && dx >= 0 && (dx as usize) < stride {
                    buf[y as usize * stride + dx as usize] = px;
                }
            }
        }
        return;
    }
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
    decoded: Vec<u8>,
    signature: u64,
    skin: Option<Rc<Skin>>,
    /// Frames that reused the composed index buffer.
    pub reused: u32,
}

impl FoxRenderer {
    pub fn new() -> Self {
        Self {
            idx: vec![0; PIXELS],
            scratch: Vec::new(),
            decoded: Vec::new(),
            signature: 0,
            skin: None,
            reused: 0,
        }
    }

    fn compose(&mut self, scene: &Scene) {
        let sig = scene
            .under
            .signature(0xcbf2_9ce4_8422_2325 ^ scene.pose as u64);
        let sig = scene.rig.map_or(sig, |rig| rig.signature(sig));
        let same_skin = self
            .skin
            .as_ref()
            .is_some_and(|s| Skin::same_resources(s, &scene.skin));
        if same_skin && sig == self.signature {
            self.reused = self.reused.wrapping_add(1);
            return;
        }
        self.signature = sig;
        self.skin = Some(Rc::clone(&scene.skin));
        let pose = scene.skin.pose(scene.pose);
        let base = scene.rig.map_or(pose.base, |rig| rig.body);
        if scene.skin.indexed_zlib() {
            // Base poses are validated opaque and canvas-sized: inflate
            // directly into the composition buffer, with no second canvas.
            scene.skin.decode_indexed(&base, &mut self.idx);
        } else {
            stamp(
                &mut self.idx,
                W,
                H,
                &base,
                &scene.skin,
                0,
                0,
                &mut self.decoded,
            );
        }
        if let Some(rig) = scene.rig {
            let dx = rig.body_dx.unsigned_abs() as usize;
            if dx > 0 && dx < W {
                let background = self.idx[0];
                for row in self.idx.chunks_exact_mut(W) {
                    if rig.body_dx > 0 {
                        row.copy_within(..W - dx, dx);
                        row[..dx].fill(background);
                    } else {
                        row.copy_within(dx.., 0);
                        row[W - dx..].fill(background);
                    }
                }
            }
            let dy = rig.body_dy.unsigned_abs() as usize;
            if dy > 0 && dy < H {
                let background = self.idx[0];
                if rig.body_dy > 0 {
                    self.idx.copy_within(..(H - dy) * W, dy * W);
                    self.idx[..dy * W].fill(background);
                } else {
                    self.idx.copy_within(dy * W.., 0);
                    self.idx[(H - dy) * W..].fill(background);
                }
            }
            for ear in rig.ears.iter().flatten() {
                self.articulated_into_idx(*ear, &scene.skin, true);
            }
        }
        for l in scene.under.iter() {
            let mut layer = *l;
            layer.dx += scene.rig.map_or(0, |rig| rig.body_dx);
            layer.dy += scene.rig.map_or(0, |rig| rig.body_dy);
            self.layer_into_idx(&layer, &scene.skin);
        }
        if let Some(rig) = scene.rig {
            for hand in rig.hands {
                self.articulated_into_idx(hand, &scene.skin, false);
            }
        }
    }

    fn articulated_into_idx(&mut self, hand: ArticulatedLayer, skin: &Skin, behind: bool) {
        let img = hand.img;
        let (w, h) = (img.w as usize, img.h as usize);
        self.scratch.resize(w * h, TRANSPARENT);
        self.scratch.fill(TRANSPARENT);
        stamp(&mut self.scratch, w, h, &img, skin, 0, 0, &mut self.decoded);
        let r = hand.angle_deg * PI / 180.0;
        let (c, s) = (cos(r), sin(r));
        let (px, py) = (hand.pivot[0] as f32, hand.pivot[1] as f32);
        let (cx, cy) = (
            img.x as f32 + px + hand.dx as f32,
            img.y as f32 + py + hand.dy as f32,
        );
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for (x, y) in [
            (0.0, 0.0),
            (w as f32, 0.0),
            (0.0, h as f32),
            (w as f32, h as f32),
        ] {
            let (x, y) = (
                cx + (x - px) * hand.scale_x * c - (y - py) * hand.scale_y * s,
                cy + (x - px) * hand.scale_x * s + (y - py) * hand.scale_y * c,
            );
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x);
            bounds[3] = bounds[3].max(y);
        }
        for y in (round(bounds[1]) - 1).max(0)..(round(bounds[3]) + 2).min(H as i32) {
            for x in (round(bounds[0]) - 1).max(0)..(round(bounds[2]) + 2).min(W as i32) {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let (u, v) = (
                    round(px + (dx * c + dy * s) / hand.scale_x),
                    round(py + (-dx * s + dy * c) / hand.scale_y),
                );
                if u < 0 || v < 0 || u >= w as i32 || v >= h as i32 {
                    continue;
                }
                let index = self.scratch[v as usize * w + u as usize];
                if index != TRANSPARENT
                    && (!behind || self.idx[y as usize * W + x as usize] == TRANSPARENT)
                {
                    self.idx[y as usize * W + x as usize] = index;
                }
            }
        }
    }

    fn layer_into_idx(&mut self, l: &Layer, skin: &Skin) {
        let img = &l.img;
        if l.scale_q8 == 256 && l.fade >= 16 {
            stamp(
                &mut self.idx,
                W,
                H,
                img,
                skin,
                img.x as i32 + l.dx as i32,
                img.y as i32 + l.dy as i32,
                &mut self.decoded,
            );
            return;
        }
        let (w, h) = (img.w as usize, img.h as usize);
        self.scratch.clear();
        self.scratch.resize(w * h, TRANSPARENT);
        stamp(&mut self.scratch, w, h, img, skin, 0, 0, &mut self.decoded);
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
        self.render_target::<false>(scene, out, (W, H), (0, 0));
    }

    /// Draw at the original asset size into the full panel, without fitting.
    pub fn render_panel(&mut self, scene: &Scene, out: &mut [u16]) {
        self.render_target::<false>(
            scene,
            out,
            (super::VIEW_W, super::VIEW_H),
            (super::VIEW_ORIGIN.0 as i32, super::VIEW_ORIGIN.1 as i32),
        );
    }

    /// Render the same colors with each storage word in big-endian byte order.
    /// The caller must publish the canvas with that encoding, never as native
    /// LVGL pixels. Encoding is folded into the final pixel store.
    pub fn render_panel_rgb565be(&mut self, scene: &Scene, out: &mut [u16]) {
        self.render_target::<true>(
            scene,
            out,
            (super::VIEW_W, super::VIEW_H),
            (super::VIEW_ORIGIN.0 as i32, super::VIEW_ORIGIN.1 as i32),
        );
    }

    fn render_target<const BIG_ENDIAN: bool>(
        &mut self,
        scene: &Scene,
        out: &mut [u16],
        size: (usize, usize),
        origin: (i32, i32),
    ) {
        let (width, height) = size;
        assert!(out.len() >= width * height, "fox canvas too short");
        self.compose(scene);
        let pal = scene.skin.palette();
        let lookup = |v: u8| -> u16 {
            if (v as usize) < pal.len() {
                pal[v as usize]
            } else {
                0
            }
        };

        let warp = scene.warp;
        warp_to::<BIG_ENDIAN>(&self.idx, pal, warp, out, size, origin);

        for l in scene.over.iter() {
            let img = &l.img;
            let (fx, fy) = warp.forward(img.x as f32 + l.dx as f32, img.y as f32 + l.dy as f32);
            let (x0, y0) = (round(fx) + origin.0, round(fy) + origin.1);
            self.scratch.clear();
            self.scratch
                .resize(img.w as usize * img.h as usize, TRANSPARENT);
            stamp(
                &mut self.scratch,
                img.w as usize,
                img.h as usize,
                img,
                &scene.skin,
                0,
                0,
                &mut self.decoded,
            );
            for sy in 0..img.h as i32 {
                let y = y0 + sy;
                if y < 0 || y >= height as i32 {
                    continue;
                }
                for sx in 0..img.w as i32 {
                    let x = x0 + sx;
                    if x < 0 || x >= width as i32 {
                        continue;
                    }
                    if l.fade < 16 && BAYER[(y & 3) as usize][(x & 3) as usize] >= l.fade {
                        continue;
                    }
                    let v = self.scratch[sy as usize * img.w as usize + sx as usize];
                    if v != TRANSPARENT {
                        out[y as usize * width + x as usize] =
                            encode_rgb565::<BIG_ENDIAN>(lookup(v));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_breathing_filter_matches_full_width_bilinear_pixels_and_preserves_the_tail() {
        fn check<const BIG_ENDIAN: bool>() {
            let palette: Vec<u16> = (0..32).map(|v| (v * 2137) as u16).collect();
            let indices: Vec<u8> = (0..PIXELS)
                .map(|i| {
                    if i % 17 == 0 {
                        TRANSPARENT
                    } else {
                        (i % 32) as u8
                    }
                })
                .collect();
            let source = |x: i32, y: i32| {
                if !(0..W as i32).contains(&x) || !(0..H as i32).contains(&y) {
                    return 0;
                }
                let index = indices[y as usize * W + x as usize];
                expanded(palette.get(index as usize).copied().unwrap_or(0))
            };
            let mix = |a, b, weight| if a == b { a } else { blend(a, b, weight) };
            for size in [(W, H), (super::super::VIEW_W, super::super::VIEW_H)] {
                let origin = if size == (W, H) { (0, 0) } else { (64, 88) };
                for (sx, sy) in [(0.88, 1.05), (1.12, 0.92), (-0.9, 1.15), (0.7, 0.8)] {
                    for (dx, dy) in [(-500, 0), (-80, -140), (0, 0), (120, 90), (500, 400)] {
                        let transform = Warp {
                            sx,
                            sy,
                            dx,
                            dy,
                            ..Warp::IDENTITY
                        };
                        let mut actual = vec![0x1234; size.0 * size.1 + 16];
                        warp_to::<BIG_ENDIAN>(
                            &indices,
                            &palette,
                            transform,
                            &mut actual,
                            size,
                            origin,
                        );
                        let ax = W as f32 / 2.0;
                        let ay = (H - 1) as f32;
                        // Independent scalar oracle filters all destination
                        // columns, including black margins and clipped edges.
                        for y in 0..size.1 {
                            let py =
                                sample(ay - (ay - (y as f32 - origin.1 as f32 - dy as f32)) / sy);
                            for x in 0..size.0 {
                                let px =
                                    sample(ax + (x as f32 - origin.0 as f32 - dx as f32 - ax) / sx);
                                let (left, top) = (i32::from(px.left), i32::from(py.left));
                                let a = mix(
                                    source(left, top),
                                    source(left + 1, top),
                                    u32::from(px.weight),
                                );
                                let b = mix(
                                    source(left, top + 1),
                                    source(left + 1, top + 1),
                                    u32::from(px.weight),
                                );
                                let rgb = mix(a, b, u32::from(py.weight));
                                assert_eq!(actual[y * size.0 + x], encode_rgb565::<BIG_ENDIAN>((rgb | rgb >> 16) as u16),
                                    "size={size:?} scale=({sx},{sy}) offset=({dx},{dy}) pixel=({x},{y})");
                            }
                        }
                        assert!(actual[size.0 * size.1..].iter().all(|&v| v == 0x1234));
                    }
                }
            }
        }
        check::<false>();
        check::<true>();
    }

    #[test]
    fn wire_pixels_match_native_across_articulation_rotation_and_skin_changes() {
        use crate::companion::render::{FoxAnimator, FoxInput, FoxPose, VIEW_PIXELS};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/skins");
        let mut native = FoxRenderer::new();
        let mut wire = FoxRenderer::new();
        let mut actual = vec![0; VIEW_PIXELS];
        let mut expected = vec![0; VIEW_PIXELS];
        for name in ["eclipse", "normal", "black-blue", "eclipse"] {
            let skin =
                Skin::parse(std::fs::read(root.join(name).join("fox.bin")).unwrap()).unwrap();
            let mut animator = FoxAnimator::new(FoxPose::Idle);
            animator.set_skin(skin);
            for pose in [FoxPose::Idle, FoxPose::Panic, FoxPose::Sleep, FoxPose::Idle] {
                animator.set_pose(pose);
                for (step, angle) in [(33, 0.0), (71, 90.0), (193, 180.0), (53, 270.0)] {
                    animator.advance(step);
                    let mut scene = animator.scene(FoxInput::REST);
                    scene.warp.angle_deg = angle;
                    actual.fill(0xabcd);
                    expected.fill(0x1234);
                    wire.render_panel_rgb565be(&scene, &mut actual);
                    native.render_panel(&scene, &mut expected);
                    assert!(
                        actual
                            .iter()
                            .zip(&expected)
                            .all(|(&w, &n)| u16::from_be(w) == n),
                        "{name} {pose:?} step={step} angle={angle}"
                    );
                }
            }
        }
        // Exercise the unscaled palette path as well as breathing and rotation.
        let scene = Scene::still(FoxPose::Idle);
        wire.render_panel_rgb565be(&scene, &mut actual);
        native.render_panel(&scene, &mut expected);
        assert!(actual
            .iter()
            .zip(&expected)
            .all(|(&w, &n)| u16::from_be(w) == n));
    }

    #[test]
    fn panel_keeps_original_pixels_and_full_size_rotations_inside_the_viewport() {
        use crate::companion::render::{VIEW_H, VIEW_ORIGIN, VIEW_PIXELS, VIEW_W};
        let scene = Scene::still(super::super::FoxPose::Idle);
        let mut renderer = FoxRenderer::new();
        let mut source = vec![0; PIXELS];
        renderer.render(&scene, &mut source);
        let mut panel = vec![0x1234; VIEW_PIXELS + 4];
        renderer.render_panel(&scene, &mut panel);
        for y in 0..H {
            let at = (VIEW_ORIGIN.1 + y) * VIEW_W + VIEW_ORIGIN.0;
            assert_eq!(&panel[at..at + W], &source[y * W..(y + 1) * W]);
        }
        // Compare every quarter/diagonal with a larger reference render. Any
        // lost source pixel or clipped silhouette changes the nonblack count.
        let palette = scene.skin.palette();
        let mut reference = vec![0; 720 * 720];
        for angle in (0..360).step_by(15) {
            let t = Warp {
                angle_deg: angle as f32,
                ..Warp::IDENTITY
            };
            warp_to::<false>(
                &renderer.idx,
                palette,
                t,
                &mut panel,
                (VIEW_W, VIEW_H),
                (VIEW_ORIGIN.0 as i32, VIEW_ORIGIN.1 as i32),
            );
            warp_to::<false>(
                &renderer.idx,
                palette,
                t,
                &mut reference,
                (720, 720),
                (VIEW_ORIGIN.0 as i32 + 120, VIEW_ORIGIN.1 as i32 + 120),
            );
            assert!(
                reference.chunks_exact(720).enumerate().all(|(y, row)| {
                    row.iter().enumerate().all(|(x, &pixel)| {
                        pixel == 0
                            || ((120..120 + VIEW_W).contains(&x)
                                && (120..120 + VIEW_H).contains(&y))
                    })
                }),
                "silhouette extends beyond the panel at {angle}"
            );
        }
        assert_eq!(&panel[VIEW_PIXELS..], &[0x1234; 4]);
    }

    #[test]
    fn rgb565_blends_round_each_channel_without_bleeding_between_lanes() {
        let colors = [0, 0xffff, 0xf800, 0x07e0, 0x001f, 0x07ff, 0x8410];
        for a in colors {
            for b in colors {
                for weight in 0..=32 {
                    let mixed = blend(expanded(a), expanded(b), weight);
                    let actual = (mixed | mixed >> 16) as u16;
                    let mut expected = 0;
                    for (shift, mask) in [(11, 31), (5, 63), (0, 31)] {
                        let a = u32::from((a >> shift) & mask);
                        let b = u32::from((b >> shift) & mask);
                        expected |= (((a * (32 - weight) + b * weight + 16) / 32) as u16) << shift;
                    }
                    assert_eq!(actual, expected, "{a:04x} {b:04x} {weight}");
                }
            }
        }
    }

    #[test]
    fn breathing_does_not_snap_rows_or_columns_at_the_nearest_pixel_boundary() {
        let mut indices = vec![0; PIXELS];
        let mut before = vec![0; PIXELS];
        let mut after = vec![0; PIXELS];
        for horizontal in [true, false] {
            for y in 0..H {
                for x in 0..W {
                    indices[y * W + x] = (if horizontal { x } else { y } % 2) as u8;
                }
            }
            let (anchor, target) = if horizontal {
                (W / 2, 100)
            } else {
                (H - 1, 150)
            };
            for (fraction, out) in [(0.49, &mut before), (0.51, &mut after)] {
                let scale =
                    (target as f32 - anchor as f32) / (target as f32 + fraction - anchor as f32);
                let transform = Warp {
                    sx: if horizontal { scale } else { 1.0 },
                    sy: if horizontal { 1.0 } else { scale },
                    ..Warp::IDENTITY
                };
                warp(&indices, &[0, 0xffff], transform, out);
            }
            // A 0.02-pixel change formerly flipped this entire line from
            // black to white, exposing a moving rectangular sampling seam.
            let at = 150 * W + 100;
            assert_eq!(before[at], 0x8410);
            assert_eq!(after[at], before[at]);
        }
    }

    #[test]
    fn warping_preserves_integer_pixels_and_clips_to_black_without_touching_the_tail() {
        // Palette zero deliberately is not black: outside the source image
        // must still be black, independent of a package's palette ordering.
        let indices = vec![0; PIXELS];
        let mut out = vec![0x1234; PIXELS + 4];
        warp(&indices, &[0xf800], Warp::IDENTITY, &mut out);
        assert!(out[..PIXELS].iter().all(|&c| c == 0xf800));
        warp(
            &indices,
            &[0xf800],
            Warp {
                sx: 0.5,
                sy: 0.5,
                ..Warp::IDENTITY
            },
            &mut out,
        );
        assert_eq!(out[0], 0);
        assert_eq!(out[168 * W + 87], 0);
        assert_eq!(out[168 * W + 88], 0xf800);
        assert_eq!(out[(H - 1) * W + W / 2], 0xf800);
        assert_eq!(&out[PIXELS..], &[0x1234; 4]);
    }

    #[test]
    fn rotated_sampling_matches_forward_placement_and_keeps_output_bounds() {
        let mut indices = vec![0; PIXELS];
        for y in 190..211 {
            indices[y * W + 165..y * W + 186].fill(1);
        }
        let mut out = vec![0x1234; PIXELS + 4];
        for angle in [-180.0, -90.0, -37.0, 0.0, 45.0, 90.0, 180.0] {
            let transform = Warp {
                angle_deg: angle,
                fit: 0.78,
                dx: 9,
                dy: -12,
                sx: 1.03,
                sy: 0.97,
            };
            warp(&indices, &[0, 0xffff], transform, &mut out);
            let (x, y) = transform.forward(175.0, 200.0);
            assert_eq!(
                out[round(y) as usize * W + round(x) as usize],
                0xffff,
                "{angle}"
            );
            assert_eq!(&out[PIXELS..], &[0x1234; 4]);
        }
    }

    #[test]
    fn filtered_rows_survive_repeated_source_rows_and_skipped_source_rows() {
        let mut indices = vec![0; PIXELS];
        let mut out = vec![0; PIXELS];
        indices[170 * W..171 * W].fill(1);
        warp(
            &indices,
            &[0, 0xffff],
            Warp {
                sy: 2.0,
                ..Warp::IDENTITY
            },
            &mut out,
        );
        for (y, expected) in [(3, 0), (4, 0x8410), (5, 0xffff), (6, 0x8410), (7, 0)] {
            assert_eq!(out[y * W + W / 2], expected, "enlarged row {y}");
        }
        indices.fill(0);
        indices[169 * W..170 * W].fill(1);
        warp(
            &indices,
            &[0, 0xffff],
            Warp {
                sy: 0.5,
                ..Warp::IDENTITY
            },
            &mut out,
        );
        for (y, expected) in [(251, 0), (252, 0xffff), (253, 0)] {
            assert_eq!(out[y * W + W / 2], expected, "reduced row {y}");
        }
    }
}
