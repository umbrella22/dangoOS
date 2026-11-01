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

use crate::companion::render::math::round;
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
    xlut: &[Sample; W],
    out: &mut [u32; W],
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

fn warp(indices: &[u8], palette: &[u16], transform: Warp, out: &mut [u16]) {
    if transform == Warp::IDENTITY {
        for (o, &v) in out[..PIXELS].iter_mut().zip(indices) {
            *o = palette.get(v as usize).copied().unwrap_or(0);
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
    let mut xlut = [Sample::default(); W];
    for (x, slot) in xlut.iter_mut().enumerate() {
        *slot = sample(ax + (x as f32 - transform.dx as f32 - ax) / transform.sx);
    }
    let mut row_a = [0; W];
    let mut row_b = [0; W];
    let (mut a, mut b) = (&mut row_a, &mut row_b);
    let (mut a_blank, mut b_blank) = (true, true);
    let mut previous = None;
    for y in 0..H {
        let row = &mut out[y * W..(y + 1) * W];
        let sy = sample(ay - (ay - (y as f32 - transform.dy as f32)) / transform.sy);
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
                b_blank = sample_row(indices, top + 1, &colors, &xlut, b);
            }
            _ => {
                a_blank = sample_row(indices, top, &colors, &xlut, a);
                b_blank = sample_row(indices, top + 1, &colors, &xlut, b);
            }
        }
        previous = Some(top);
        if a_blank && b_blank {
            row.fill(0);
            continue;
        }
        for ((o, &top), &bottom) in row.iter_mut().zip(a.iter()).zip(b.iter()) {
            let rgb = if top == bottom {
                top
            } else {
                blend(top, bottom, u32::from(sy.weight))
            };
            *o = (rgb | rgb >> 16) as u16;
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
        if scene.skin.indexed_zlib() {
            // Base poses are validated opaque and canvas-sized: inflate
            // directly into the composition buffer, with no second canvas.
            scene.skin.decode_indexed(&pose.base, &mut self.idx);
        } else {
            stamp(
                &mut self.idx,
                W,
                H,
                &pose.base,
                &scene.skin,
                0,
                0,
                &mut self.decoded,
            );
        }
        for l in scene.under.iter() {
            self.layer_into_idx(l, &scene.skin);
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
        assert!(out.len() >= PIXELS, "fox canvas too short");
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
        self::warp(&self.idx, pal, warp, out);

        for l in scene.over.iter() {
            let img = &l.img;
            let (fx, fy) = warp.forward(img.x as f32 + l.dx as f32, img.y as f32 + l.dy as f32);
            let (x0, y0) = (round(fx), round(fy));
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

#[cfg(test)]
mod tests {
    use super::*;

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
