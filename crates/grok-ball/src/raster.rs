//! Software RGB565 rasterizer (G0.3).
//!
//! The public API writes into a caller-owned canvas buffer and performs no
//! allocation. [`render`] is the original 320x320 / 2x2-supersampling entry
//! point; [`render_sized_with_quality`] adds a runtime canvas size (the
//! D-BALL-01 240x240 fallback) and cheaper edge sampling for tight frame
//! budgets.
//!
//! # Pipeline
//!
//! 1. The engine keeps every polygon in the upstream source space
//!    (`viewBox = -15 -15 259 259`). This module applies the body transform,
//!    the per-eye pose and finally the canvas mapping:
//!    `x' = (x + 15) * size / 259`.
//! 2. The body ring is filled with a two-stop radial gradient, then each eye
//!    ring is filled with a dark near-black color.
//! 3. Edges are antialiased with configurable sampling (2x2 by default; 2x1
//!    or 1x1 for lower frame cost). The canvas is clipped to `size` and the
//!    renderer returns the full canvas as the dirty rectangle.
//!
//! # Documented approximations
//!
//! - **Body transform**: the upstream SVG group is
//!   `translate(HEAD_C + y) rotate(rotate) scale(scale) translate(-HEAD_C)`.
//!   The frame's `breathe` value is treated as a static scale offset for this
//!   call: `scale = body.scale * (1 + body.breathe)`.
//! - **Eye pose**: the upstream engine additionally projects the eyes onto a
//!   spherical face. G0.3 uses the simplified contract: scale the ring around
//!   its own centroid, squash vertically by `open` (clamped to `0..=1.5`),
//!   translate by `x/y`, add `look_x/look_y`, then apply the body transform.
//! - **Body gradient**: the upstream SVG uses a three-stop radial gradient at
//!   `38% / 32%`, `r = 75%` of the object bounding box. The engine exposes a
//!   single body color, so the rasterizer uses the body color at the center
//!   and the same color multiplied by `0.72` at the outer radius of a radial
//!   gradient centered at 38% / 32% of the canvas with `r = 75%` of the canvas
//!   width. The ramp is evaluated through a 64-entry LUT indexed by the
//!   normalized squared distance, which avoids a square root per pixel.
//! - **No allocation**: all scratch state is fixed-size stack storage.

use core::cmp::Ordering;

use crate::engine::BallFrame;
use crate::types::{Point, Rgb, BODY_POINTS, RING_POINTS};

/// Canvas edge in pixels.
pub const CANVAS_SIZE: usize = 320;

/// Canvas pixel count.
pub const CANVAS_PIXELS: usize = CANVAS_SIZE * CANVAS_SIZE;

/// Inclusive dirty rectangle in canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}

impl DirtyRect {
    pub const FULL: Self = Self {
        x0: 0,
        y0: 0,
        x1: (CANVAS_SIZE - 1) as u16,
        y1: (CANVAS_SIZE - 1) as u16,
    };

    pub const EMPTY: Self = Self {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };

    pub const fn is_empty(self) -> bool {
        self.x1 < self.x0 || self.y1 < self.y0
    }
}

/// Source SVG viewBox minimum (`-15 -15 259 259`).
const VIEWBOX_MIN: f32 = -15.0;

/// Source SVG viewBox span in both axes.
const VIEWBOX_SPAN: f32 = 259.0;

/// Upstream body center (`EB_RINGS.HEAD_C`).
const HEAD_CENTER: f32 = 114.2705;

/// Edge sampling quality.
///
/// The default [`render`] path keeps the original 2x2 supersampling. Callers
/// with a tighter frame budget can request fewer sub-samples; interior pixels
/// (the majority of the ball) are shaded identically in every mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// One sample at the pixel center. Fastest; silhouette and eye edges are
    /// aliased.
    OneSample,
    /// Two horizontal samples in one row. Halves the scanline work of 2x2 and
    /// keeps horizontal antialiasing, which is what the slanted eye edges
    /// need.
    TwoByOne,
    /// Two samples per axis (the original G0.3 quality).
    TwoByTwo,
}

impl Quality {
    const fn sample_counts(self) -> (u16, u16) {
        match self {
            Self::OneSample => (1, 1),
            Self::TwoByOne => (2, 1),
            Self::TwoByTwo => (2, 2),
        }
    }

    const fn coverage_max(self) -> u8 {
        let (x, y) = self.sample_counts();
        (x * y) as u8
    }
}

/// Upstream eye fill is `#1A1A1A`; G0.3 uses the requested near-black value.
const EYE_COLOR: Rgb = Rgb::from_hex(0x1B1B1B);

/// Number of entries in the body gradient LUT.
///
/// The LUT is indexed by squared distance, so the first entries span the
/// widest radius steps; 512 entries keep the step at the centre below half
/// a green level (2 KiB on the stack while rendering).
const GRADIENT_LUT_LEN: usize = 512;

/// RGB565 level with 4 fractional bits per channel, packed into one `u32`:
/// red at bits 19..28, green at 9..19, blue at 0..9. Each field has one
/// spare bit, so adding a 0..15 threshold to every field never carries
/// into its neighbour.
type PackedLevel = u32;

const R_SHIFT: u32 = 19;
const G_SHIFT: u32 = 9;

/// 4x4 Bayer thresholds (0..16), replicated into every packed field.
///
/// RGB565 has only 32 red/blue and 64 green levels; the large, slow body
/// gradient shows contour bands. Ordered dithering spreads each fractional
/// level over a 4x4 tile, is deterministic per pixel (a still ball does not
/// shimmer) and costs one add plus the field extraction per pixel.
const BAYER4: [[PackedLevel; 4]; 4] = {
    const M: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let mut out = [[0; 4]; 4];
    let mut y = 0;
    while y < 4 {
        let mut x = 0;
        while x < 4 {
            let t = M[y][x];
            out[y][x] = (t << R_SHIFT) | (t << G_SHIFT) | t;
            x += 1;
        }
        y += 1;
    }
    out
};

/// Outer gradient color factor (`72 / 100`).
const GRADIENT_OUTER_NUM: u32 = 72;
const GRADIENT_OUTER_DEN: u32 = 100;

/// Upper bound applied to transformed coordinates. Anything beyond this is
/// far outside the canvas; clamping keeps the scanline arithmetic bounded
/// without affecting visible geometry.
const COORD_LIMIT: f32 = 1_000_000.0;

/// Upper bound applied to body/eye scales to keep absurd frames finite.
const SCALE_LIMIT: f32 = 1_000.0;

/// Anisotropic canvas-space decoration applied after the engine pose.
///
/// Used for the landing squash, the held-ball squirm and the "hop" offsets.
/// The transform is applied around the canvas center, so the identity pose is
/// a no-op and a scale above `1.0` grows the ball symmetrically (the widget
/// clamps it so the blob cannot be clipped by its own canvas).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasPose {
    pub scale_x: f32,
    pub scale_y: f32,
    /// Extra canvas-space translation in pixels, applied after scaling.
    pub offset_x: f32,
    pub offset_y: f32,
    /// Rotation in degrees around the canvas center.
    pub rotate_deg: f32,
}

impl CanvasPose {
    /// The identity pose.
    pub const IDENTITY: Self = Self {
        scale_x: 1.0,
        scale_y: 1.0,
        offset_x: 0.0,
        offset_y: 0.0,
        rotate_deg: 0.0,
    };

    /// Whether the pose changes nothing.
    pub const fn is_identity(&self) -> bool {
        self.scale_x == 1.0
            && self.scale_y == 1.0
            && self.offset_x == 0.0
            && self.offset_y == 0.0
            && self.rotate_deg == 0.0
    }

    /// Clamp and sanitize every field into the range the fill can serve.
    pub fn sanitized(self) -> Self {
        Self {
            scale_x: clamp_scale(finite_or(self.scale_x, 1.0)),
            scale_y: clamp_scale(finite_or(self.scale_y, 1.0)),
            offset_x: finite_or(self.offset_x, 0.0).clamp(-COORD_LIMIT, COORD_LIMIT),
            offset_y: finite_or(self.offset_y, 0.0).clamp(-COORD_LIMIT, COORD_LIMIT),
            rotate_deg: finite_or(self.rotate_deg, 0.0).clamp(-3600.0, 3600.0),
        }
    }
}

impl Default for CanvasPose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Render one frame into `out` and return the updated region.
///
/// This is the original 320x320 / 2x2-supersampling entry point. The full
/// canvas is repainted (black background, body, then eyes) and
/// [`DirtyRect::FULL`] is returned for the first release. The function never
/// allocates and never writes outside `out`.
pub fn render(frame: &BallFrame, out: &mut [u16; CANVAS_PIXELS]) -> DirtyRect {
    render_sized_with_quality(frame, out, CANVAS_SIZE, Quality::TwoByTwo)
}

/// Render one frame into a `size * size` canvas at the default 2x2 quality.
///
/// `size` selects the coordinate mapping (`size / 259`) and the dirty
/// rectangle. The buffer must contain at least `size * size` pixels.
pub fn render_sized(frame: &BallFrame, out: &mut [u16], size: usize) -> DirtyRect {
    render_sized_with_quality(frame, out, size, Quality::TwoByTwo)
}

/// Render one frame into a `size * size` canvas with explicit edge sampling.
///
/// The full canvas is repainted and the returned rectangle covers exactly the
/// rendered canvas. `out` may be longer than needed; extra pixels are left
/// untouched.
pub fn render_sized_with_quality(
    frame: &BallFrame,
    out: &mut [u16],
    size: usize,
    quality: Quality,
) -> DirtyRect {
    render_sized_with_pose(frame, out, size, quality, CanvasPose::IDENTITY)
}

/// Render one frame with an anisotropic canvas-space pose.
///
/// The pose is applied after the engine's source-space transform, around the
/// canvas center: `p' = center + rotate(scale * (p - center)) + offset`. It is
/// the hook for landing squash, held-ball squirm and hop offsets; the gradient
/// center and radius move with the body so the shading stays attached.
pub fn render_sized_with_pose(
    frame: &BallFrame,
    out: &mut [u16],
    size: usize,
    quality: Quality,
    pose: CanvasPose,
) -> DirtyRect {
    render_sized_with_pose_and_skin(frame, out, size, quality, pose, &crate::skin::BLOB)
}

/// Render one frame with a pose and a character skin.
///
/// Accessories are polygons in the same source space as the body, so they get
/// the body transform and the canvas pose for free. `Layer::Behind` shapes are
/// filled before the body (ear bases disappear under the head) and `Front`
/// shapes after the eyes.
pub fn render_sized_with_pose_and_skin(
    frame: &BallFrame,
    out: &mut [u16],
    size: usize,
    quality: Quality,
    pose: CanvasPose,
    skin: &crate::skin::Skin,
) -> DirtyRect {
    assert!(size >= 3, "canvas size must be at least 3");
    assert!(
        size <= u16::MAX as usize,
        "canvas size does not fit DirtyRect"
    );
    let pixels = size.checked_mul(size).expect("canvas size overflow");
    assert!(out.len() >= pixels, "canvas buffer too short");
    let out = &mut out[..pixels];

    // The background is a black field; this also clears stale pixels so a
    // full-canvas dirty rectangle is always truthful.
    out.fill(0);

    let canvas = size as f32;
    let scale = canvas / VIEWBOX_SPAN;
    let body_xform = BodyTransform::from_frame(frame);
    let pose = pose.sanitized();
    // The decoration is the identity for the resting ball, which is the
    // overwhelming majority of frames. Routing every point through a second
    // transform there is pure overhead, so the identity case keeps the
    // original single-transform path.
    let decorate = CanvasDecorator::new(pose, canvas);
    let identity = pose.is_identity();

    let draw_accessories =
        |layer: crate::skin::Layer, out: &mut [u16], decorate: &CanvasDecorator| {
            for accessory in skin.accessories {
                if accessory.layer != layer {
                    continue;
                }
                let ring = accessory.ring();
                let mut points = [Point::ZERO; crate::skin::ACCESSORY_POINTS];
                for (dst, src) in points.iter_mut().zip(ring.iter()) {
                    let point = to_canvas(body_xform.apply(*src), scale);
                    *dst = if identity { point } else { decorate.apply(point) };
                }
                fill_polygon(
                    &points[..ring.len()],
                    &SolidShader {
                        value: accessory.color.to_rgb565(),
                    },
                    out,
                    size,
                    quality,
                );
            }
        };

    draw_accessories(crate::skin::Layer::Behind, out, &decorate);

    // Transform the body ring into canvas coordinates once.
    let mut body_points = [Point::ZERO; BODY_POINTS];
    for (dst, src) in body_points.iter_mut().zip(frame.body_ring.iter()) {
        let point = to_canvas(body_xform.apply(*src), scale);
        *dst = if identity { point } else { decorate.apply(point) };
    }

    let body_color = skin.body_color.unwrap_or(frame.body.color);
    let radius = 0.75 * canvas * (pose.scale_x.max(pose.scale_y));
    let gradient_center = decorate.apply(Point {
        x: 0.38 * canvas,
        y: 0.32 * canvas,
    });
    let gradient = RadialShader {
        lut: body_gradient_lut(body_color),
        cx: gradient_center.x,
        cy: gradient_center.y,
        inv_r2: 1.0 / (radius * radius),
    };
    fill_polygon(&body_points, &gradient, out, size, quality);

    // Eyes are drawn after the body and move with the body transform.
    let eyes = [
        (&frame.left_ring, &frame.left),
        (&frame.right_ring, &frame.right),
    ];
    let eye_shader = SolidShader {
        value: EYE_COLOR.to_rgb565(),
    };
    for (ring, pose) in eyes {
        let eye_points = transform_eye(ring, pose, &body_xform, scale);
        if identity {
            fill_polygon(&eye_points, &eye_shader, out, size, quality);
        } else {
            let mut decorated = [Point::ZERO; RING_POINTS];
            for (dst, src) in decorated.iter_mut().zip(eye_points.iter()) {
                *dst = decorate.apply(*src);
            }
            fill_polygon(&decorated, &eye_shader, out, size, quality);
        }
    }

    draw_accessories(crate::skin::Layer::Front, out, &decorate);

    DirtyRect {
        x0: 0,
        y0: 0,
        x1: (size - 1) as u16,
        y1: (size - 1) as u16,
    }
}

// ---- transforms ----------------------------------------------------------

/// Canvas-space pose applied around the canvas center.
#[derive(Clone, Copy)]
struct CanvasDecorator {
    scale_x: f32,
    scale_y: f32,
    offset_x: f32,
    offset_y: f32,
    sin: f32,
    cos: f32,
    center: f32,
}

impl CanvasDecorator {
    fn new(pose: CanvasPose, canvas: f32) -> Self {
        let (sin, cos) = sin_cos_deg(pose.rotate_deg);
        Self {
            scale_x: pose.scale_x,
            scale_y: pose.scale_y,
            offset_x: pose.offset_x,
            offset_y: pose.offset_y,
            sin,
            cos,
            center: canvas * 0.5,
        }
    }

    #[inline]
    fn apply(&self, p: Point) -> Point {
        let dx = (p.x - self.center) * self.scale_x;
        let dy = (p.y - self.center) * self.scale_y;
        Point {
            x: sanitize_coord(self.center + dx * self.cos - dy * self.sin + self.offset_x),
            y: sanitize_coord(self.center + dx * self.sin + dy * self.cos + self.offset_y),
        }
    }
}

/// Body rotation/scale around the source center, followed by the `body.y`
/// translation. The canvas mapping is applied separately.
#[derive(Clone, Copy)]
struct BodyTransform {
    scale: f32,
    sin: f32,
    cos: f32,
    translate_y: f32,
}

impl BodyTransform {
    fn from_frame(frame: &BallFrame) -> Self {
        let scale = finite_or(frame.body.scale, 1.0) * (1.0 + finite_or(frame.body.breathe, 0.0));
        let (sin, cos) = sin_cos_deg(finite_or(frame.body.rotate, 0.0));
        Self {
            scale: clamp_scale(scale),
            sin,
            cos,
            translate_y: finite_or(frame.body.y, 0.0),
        }
    }

    /// `p' = center + rotate(scale * (p - center)) + (0, translate_y)`.
    #[inline]
    fn apply(&self, p: Point) -> Point {
        let dx = (p.x - HEAD_CENTER) * self.scale;
        let dy = (p.y - HEAD_CENTER) * self.scale;
        Point {
            x: HEAD_CENTER + dx * self.cos - dy * self.sin,
            y: HEAD_CENTER + dx * self.sin + dy * self.cos + self.translate_y,
        }
    }
}

/// Apply one eye pose around the ring's own centroid, then the body transform,
/// and map the result to canvas coordinates.
fn transform_eye(
    ring: &[Point; RING_POINTS],
    pose: &crate::types::EyePose,
    body: &BodyTransform,
    scale: f32,
) -> [Point; RING_POINTS] {
    // Upstream anchors eye scaling at the centroid of the current ring.
    let mut centroid_x = 0.0f32;
    let mut centroid_y = 0.0f32;
    for p in ring.iter() {
        centroid_x += finite_or(p.x, HEAD_CENTER);
        centroid_y += finite_or(p.y, HEAD_CENTER);
    }
    let inv_len = 1.0 / RING_POINTS as f32;
    let centroid_x = centroid_x * inv_len;
    let centroid_y = centroid_y * inv_len;

    let open = finite_or(pose.open, 1.0).clamp(0.0, 1.5);
    let scale_x = clamp_scale(finite_or(pose.scale_x, 1.0));
    let scale_y = clamp_scale(finite_or(pose.scale_y, 1.0)) * open;
    let shift_x = finite_or(pose.x, 0.0) + finite_or(pose.look_x, 0.0);
    let shift_y = finite_or(pose.y, 0.0) + finite_or(pose.look_y, 0.0);

    let mut out = [Point::ZERO; RING_POINTS];
    for (dst, src) in out.iter_mut().zip(ring.iter()) {
        let px = finite_or(src.x, centroid_x);
        let py = finite_or(src.y, centroid_y);
        let posed = Point {
            x: centroid_x + (px - centroid_x) * scale_x + shift_x,
            y: centroid_y + (py - centroid_y) * scale_y + shift_y,
        };
        *dst = to_canvas(body.apply(posed), scale);
    }
    out
}

/// Map one source-space point to canvas space, sanitizing non-finite values.
#[inline]
fn to_canvas(p: Point, scale: f32) -> Point {
    Point {
        x: sanitize_coord((p.x - VIEWBOX_MIN) * scale),
        y: sanitize_coord((p.y - VIEWBOX_MIN) * scale),
    }
}

#[inline]
fn sanitize_coord(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-COORD_LIMIT, COORD_LIMIT)
    } else {
        0.0
    }
}

// ---- polygon fill --------------------------------------------------------

/// Pixel color source used by [`fill_polygon`].
trait Shader {
    /// Color for one fully covered pixel `(px, py)`, sampled at its centre.
    fn color_at(&self, px: usize, py: usize) -> u16;
}

struct SolidShader {
    value: u16,
}

impl Shader for SolidShader {
    #[inline]
    fn color_at(&self, _px: usize, _py: usize) -> u16 {
        self.value
    }
}

struct RadialShader {
    lut: [PackedLevel; GRADIENT_LUT_LEN],
    cx: f32,
    cy: f32,
    inv_r2: f32,
}

impl Shader for RadialShader {
    #[inline]
    fn color_at(&self, px: usize, py: usize) -> u16 {
        let dx = px as f32 + 0.5 - self.cx;
        let dy = py as f32 + 0.5 - self.cy;
        // Index the LUT by normalized squared distance. The LUT itself
        // compensates with sqrt, so the visible ramp is linear in radius.
        let scaled = (dx * dx + dy * dy) * self.inv_r2 * (GRADIENT_LUT_LEN - 1) as f32;
        let index = if scaled.partial_cmp(&0.0) != Some(Ordering::Greater) {
            0
        } else {
            let i = scaled as usize;
            if i >= GRADIENT_LUT_LEN {
                GRADIENT_LUT_LEN - 1
            } else {
                i
            }
        };
        let level = self.lut[index];
        dither565(level, px, py)
    }
}

/// Quantize a packed fractional RGB565 level with 4x4 ordered dithering.
///
/// Per channel this is `floor((level16 + t) / 16)` for the Bayer threshold
/// `t`: over one 4x4 tile the mean equals `level16 / 16` exactly, and whole
/// levels stay flat.
#[inline]
fn dither565(level: PackedLevel, px: usize, py: usize) -> u16 {
    let p = level + BAYER4[py & 3][px & 3];
    (((p >> 12) & 0xf800) | ((p >> 8) & 0x07e0) | ((p >> 4) & 0x001f)) as u16
}

/// Fill a closed polygon with the requested edge sampling.
///
/// The even-odd rule matches the upstream SVG default fill rule. Coverage is
/// accumulated per pixel row in a fixed 320-byte buffer (the maximum canvas
/// size), then composited over the existing canvas (which is black for the
/// body pass).
fn fill_polygon<S: Shader>(
    points: &[Point],
    shader: &S,
    out: &mut [u16],
    size: usize,
    quality: Quality,
) {
    let n = points.len();
    if !(3..=BODY_POINTS).contains(&n) {
        return;
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for p in points {
        if p.x < min_x {
            min_x = p.x;
        }
        if p.x > max_x {
            max_x = p.x;
        }
        if p.y < min_y {
            min_y = p.y;
        }
        if p.y > max_y {
            max_y = p.y;
        }
    }

    let canvas = size as f32;
    if !(min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite()) {
        return;
    }
    if max_x <= 0.0 || max_y <= 0.0 || min_x >= canvas || min_y >= canvas {
        return;
    }

    let x_start = if min_x <= 0.0 { 0 } else { min_x as usize };
    let x_end = if max_x >= canvas {
        size - 1
    } else {
        max_x as usize
    };
    let y_start = if min_y <= 0.0 { 0 } else { min_y as usize };
    let y_end = if max_y >= canvas {
        size - 1
    } else {
        max_y as usize
    };
    if x_start > x_end || y_start > y_end {
        return;
    }

    let mut coverage_storage = [0u8; CANVAS_SIZE];
    let coverage = &mut coverage_storage[..size];
    let mut crossings = [0.0f32; BODY_POINTS];
    let (samples_x, samples_y) = quality.sample_counts();
    let coverage_max = quality.coverage_max();
    let inv_samples_x = 1.0 / samples_x as f32;
    let inv_samples_y = 1.0 / samples_y as f32;

    let mut py = y_start;
    while py <= y_end {
        coverage[x_start..=x_end].fill(0);

        let mut sub_y = 0u16;
        while sub_y < samples_y {
            // Centers of the vertical sub-samples (0.25/0.75 for two).
            let sy = py as f32 + (sub_y as f32 + 0.5) * inv_samples_y;
            let mut count = 0usize;

            let mut prev_x = points[n - 1].x;
            let mut prev_y = points[n - 1].y;
            for p in points {
                let y0 = prev_y;
                let y1 = p.y;
                // Half-open vertical test: horizontal edges never cross and
                // shared vertices are counted exactly once.
                if (y0 <= sy) != (y1 <= sy) {
                    let t = (sy - y0) / (y1 - y0);
                    crossings[count] = prev_x + (p.x - prev_x) * t;
                    count += 1;
                }
                prev_x = p.x;
                prev_y = p.y;
            }

            // Insertion sort; the crossing count is tiny for these rings.
            let mut i = 1;
            while i < count {
                let value = crossings[i];
                let mut j = i;
                while j > 0 && crossings[j - 1] > value {
                    crossings[j] = crossings[j - 1];
                    j -= 1;
                }
                crossings[j] = value;
                i += 1;
            }

            // Even-odd spans.
            let mut i = 0;
            while i + 1 < count {
                let xa = crossings[i];
                let xb = crossings[i + 1];
                i += 2;

                if xb.partial_cmp(&xa) != Some(Ordering::Greater) || xb <= 0.0 || xa >= canvas {
                    continue;
                }

                let ia = if xa <= 0.0 { 0 } else { xa as usize };
                let ib = if xb >= canvas { size - 1 } else { xb as usize };
                let ia = ia.max(x_start);
                let ib = ib.min(x_end);
                if ia > ib {
                    continue;
                }

                for (offset, cover) in coverage[ia..=ib].iter_mut().enumerate() {
                    let px = ia + offset;
                    let px_f = px as f32;
                    let mut c = 0u8;
                    let mut sub_x = 0u16;
                    while sub_x < samples_x {
                        // Centers of the horizontal sub-samples.
                        let sx = (sub_x as f32 + 0.5) * inv_samples_x;
                        if px_f + sx >= xa && px_f + sx < xb {
                            c += 1;
                        }
                        sub_x += 1;
                    }
                    *cover += c;
                }
            }

            sub_y += 1;
        }

        let row = py * size;
        for (offset, &cov) in coverage[x_start..=x_end].iter().enumerate() {
            if cov == 0 {
                continue;
            }
            let px = x_start + offset;
            let src = shader.color_at(px, py);
            let index = row + px;
            out[index] = blend565(out[index], src, cov, coverage_max);
        }

        py += 1;
    }
}

/// Composite `src` over `dst` using 1..=`coverage_max` sub-sample coverage.
#[inline]
fn blend565(dst: u16, src: u16, coverage: u8, coverage_max: u8) -> u16 {
    if coverage >= coverage_max {
        return src;
    }
    // Division by the power-of-two sample count with round-to-nearest.
    let shift = coverage_max.trailing_zeros();
    let round = if shift == 0 { 0 } else { 1 << (shift - 1) };
    let c = coverage as u16;
    let inv = coverage_max as u16 - c;
    let r = (((dst >> 11) & 0x1f) * inv + ((src >> 11) & 0x1f) * c + round) >> shift;
    let g = (((dst >> 5) & 0x3f) * inv + ((src >> 5) & 0x3f) * c + round) >> shift;
    let b = ((dst & 0x1f) * inv + (src & 0x1f) * c + round) >> shift;
    (r << 11) | (g << 5) | b
}

/// Build the two-stop body gradient LUT.
///
/// Entry `i` covers squared distances `i..i + 1` (normalized by `n - 1`) and
/// stores the color at the bucket's mean radius, so the ramp is linear in
/// radius although it is indexed by squared distance. Each entry keeps four
/// fractional bits per channel for [`dither565`].
fn body_gradient_lut(color: Rgb) -> [PackedLevel; GRADIENT_LUT_LEN] {
    let center = [color.r as u32, color.g as u32, color.b as u32];
    let outer = [
        (center[0] * GRADIENT_OUTER_NUM + GRADIENT_OUTER_DEN / 2) / GRADIENT_OUTER_DEN,
        (center[1] * GRADIENT_OUTER_NUM + GRADIENT_OUTER_DEN / 2) / GRADIENT_OUTER_DEN,
        (center[2] * GRADIENT_OUTER_NUM + GRADIENT_OUTER_DEN / 2) / GRADIENT_OUTER_DEN,
    ];

    // Per channel: level16(t) = start + delta * t, with four fractional bits.
    let ramp = |c: u32, o: u32, max: u32| {
        let k = (max * 16) as f32 / 255.0;
        (c as f32 * k, (o as f32 - c as f32) * k, (max * 16) as f32)
    };
    let channels = [
        ramp(center[0], outer[0], 31),
        ramp(center[1], outer[1], 63),
        ramp(center[2], outer[2], 31),
    ];
    let level = |(start, delta, top): (f32, f32, f32), t: f32| {
        // `as` saturates on overflow and truncates toward zero.
        ((start + delta * t + 0.5) as u32).min(top as u32)
    };

    let mut lut = [0; GRADIENT_LUT_LEN];
    let inv_steps = 1.0 / (GRADIENT_LUT_LEN - 1) as f32;
    let mut i = 0usize;
    while i < GRADIENT_LUT_LEN {
        // Mean radius over the bucket, clamped to the outer stop.
        let t = sqrt_approx(((i as f32 + 0.5) * inv_steps).min(1.0));
        lut[i] = (level(channels[0], t) << R_SHIFT)
            | (level(channels[1], t) << G_SHIFT)
            | level(channels[2], t);
        i += 1;
    }
    lut
}

// ---- math helpers --------------------------------------------------------

/// Fast square root used only while building the body gradient LUT.
///
/// One bit-level initial guess plus two Newton steps gives well under 0.1%
/// relative error over the normalized `0..=1` input range.
fn sqrt_approx(v: f32) -> f32 {
    if !v.is_finite() || v <= 0.0 {
        return 0.0;
    }
    let bits = v.to_bits();
    // Initial guess for 1/sqrt(v) (0x5f3759df), refined twice.
    let mut inv = f32::from_bits(0x5f37_59dfu32.wrapping_sub(bits >> 1));
    inv *= 1.5 - 0.5 * v * inv * inv;
    inv *= 1.5 - 0.5 * v * inv * inv;
    v * inv
}

/// Small polynomial sine with range reduction, matching the engine's
/// approximation. Maximum absolute error is about `1.6e-4`, far below one
/// source unit.
fn sin_approx(x: f32) -> f32 {
    use core::f32::consts::{PI, TAU};

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

/// Sine and cosine of an angle in degrees.
fn sin_cos_deg(deg: f32) -> (f32, f32) {
    use core::f32::consts::PI;

    let reduced = if deg.is_finite() { deg % 360.0 } else { 0.0 };
    let radians = reduced * (PI / 180.0);
    (sin_approx(radians), sin_approx(radians + PI * 0.5))
}

#[inline]
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

#[inline]
fn clamp_scale(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, SCALE_LIMIT)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(r: u32, g: u32, b: u32) -> PackedLevel {
        (r << R_SHIFT) | (g << G_SHIFT) | b
    }

    fn channels(p: u16) -> [u32; 3] {
        [(p >> 11) as u32, ((p >> 5) & 0x3f) as u32, (p & 0x1f) as u32]
    }

    #[test]
    fn ordered_dither_keeps_whole_levels_flat() {
        let level = pack(10 * 16, 40 * 16, 31 * 16);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(dither565(level, x, y), (10 << 11) | (40 << 5) | 31);
            }
        }
    }

    #[test]
    fn ordered_dither_preserves_the_mean_level() {
        // 10.5, 20.25 and 3.75 levels.
        let level = pack(10 * 16 + 8, 20 * 16 + 4, 3 * 16 + 12);
        let mut sum = [0u32; 3];
        for y in 0..4 {
            for x in 0..4 {
                let c = channels(dither565(level, x, y));
                for i in 0..3 {
                    sum[i] += c[i];
                }
            }
        }
        // Means times 16: 10.5, 20.25, 3.75.
        assert_eq!(sum, [168, 324, 60]);
    }

    #[test]
    fn ordered_dither_never_carries_between_channels() {
        let top = pack(31 * 16, 63 * 16, 31 * 16);
        let low_g = pack(31 * 16, 0, 31 * 16);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(dither565(top, x, y), 0xffff);
                assert_eq!(dither565(low_g, x, y), 0xf81f);
            }
        }
    }

    #[test]
    fn gradient_lut_darkens_from_center_to_outer_color() {
        let lut = body_gradient_lut(Rgb::from_hex(0xf3f0ea));
        let unpack = |p: PackedLevel| [p >> R_SHIFT, (p >> G_SHIFT) & 0x3ff, p & 0x1ff];
        let expect = |v: f32, max: f32| (v * max * 16.0 / 255.0 + 0.5) as u32;
        // The first bucket is within one level (16) of the centre color.
        let first = unpack(lut[0]);
        let centre = [expect(243.0, 31.0), expect(240.0, 63.0), expect(234.0, 31.0)];
        for c in 0..3 {
            assert!(centre[c] - first[c] < 16, "{first:?} vs {centre:?}");
        }
        // The last bucket is the outer stop: 72 % of the centre, rounded.
        let last = unpack(lut[GRADIENT_LUT_LEN - 1]);
        assert_eq!(last, [expect(175.0, 31.0), expect(173.0, 63.0), expect(168.0, 31.0)]);
        for pair in lut.windows(2) {
            let (a, b) = (unpack(pair[0]), unpack(pair[1]));
            for c in 0..3 {
                assert!(b[c] <= a[c]);
            }
        }
    }
}
