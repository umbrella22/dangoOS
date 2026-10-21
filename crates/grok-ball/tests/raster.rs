//! G0.3 rasterizer acceptance tests.
//!
//! These tests build synthetic frames in the upstream source space, so they do
//! not depend on the G0.1 data tables or the G0.2 expression catalog. A final
//! timeline test drives the real engine for 60 simulated seconds.

use grok_ball::engine::{BallEngine, BallFrame};
use grok_ball::raster::{render, DirtyRect, CANVAS_PIXELS};
use grok_ball::types::{AiState, BodyParams, EyePose, Point, Rgb, BODY_POINTS, RING_POINTS};

/// Exact RGB565 value written for a fully covered eye pixel.
const EYE_565: u16 = 0x18C3;

// ---- synthetic frames ----------------------------------------------------

/// Distribute `N` points evenly along the closed polyline through `vertices`.
fn ring_from_vertices<const N: usize>(vertices: &[Point]) -> [Point; N] {
    assert!(!vertices.is_empty());
    let mut ring = [Point::ZERO; N];
    let per_edge = N / vertices.len();
    assert!(per_edge > 0);
    let mut index = 0;
    for i in 0..vertices.len() {
        let a = vertices[i];
        let b = vertices[(i + 1) % vertices.len()];
        for j in 0..per_edge {
            let t = j as f32 / per_edge as f32;
            ring[index] = Point {
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t,
            };
            index += 1;
        }
    }
    while index < N {
        ring[index] = vertices[0];
        index += 1;
    }
    ring
}

fn square_vertices(cx: f32, cy: f32, half: f32) -> [Point; 4] {
    [
        Point {
            x: cx - half,
            y: cy - half,
        },
        Point {
            x: cx + half,
            y: cy - half,
        },
        Point {
            x: cx + half,
            y: cy + half,
        },
        Point {
            x: cx - half,
            y: cy + half,
        },
    ]
}

fn leak_body(ring: [Point; BODY_POINTS]) -> &'static [Point; BODY_POINTS] {
    Box::leak(Box::new(ring))
}

fn known_frame() -> BallFrame {
    let body = ring_from_vertices::<BODY_POINTS>(&square_vertices(114.2705, 114.2705, 80.0));
    BallFrame {
        body_ring: leak_body(body),
        left_ring: ring_from_vertices::<RING_POINTS>(&square_vertices(80.0, 114.2705, 12.0)),
        right_ring: ring_from_vertices::<RING_POINTS>(&square_vertices(148.541, 114.2705, 12.0)),
        body: BodyParams {
            color: Rgb::from_hex(0xF1EEE8),
            ..BodyParams::DEFAULT
        },
        left: EyePose::DEFAULT,
        right: EyePose::DEFAULT,
        openness: 1.0,
    }
}

fn triangular_frame() -> BallFrame {
    let body = ring_from_vertices::<BODY_POINTS>(&square_vertices(114.2705, 114.2705, 80.0));
    let triangle = [
        Point { x: 60.0, y: 60.0 },
        Point { x: 170.0, y: 60.0 },
        Point { x: 60.0, y: 170.0 },
    ];
    let degenerate_eye = [Point::ZERO; RING_POINTS];
    BallFrame {
        body_ring: leak_body(body),
        left_ring: ring_from_vertices::<RING_POINTS>(&triangle),
        right_ring: degenerate_eye,
        body: BodyParams {
            color: Rgb::from_hex(0x000000),
            ..BodyParams::DEFAULT
        },
        left: EyePose::DEFAULT,
        right: EyePose::DEFAULT,
        openness: 1.0,
    }
}

fn hash_pixels(pixels: &[u16; CANVAS_PIXELS]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &value in pixels {
        hash ^= value as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn pixel(pixels: &[u16; CANVAS_PIXELS], x: usize, y: usize) -> u16 {
    pixels[y * 320 + x]
}

fn red5(value: u16) -> u16 {
    (value >> 11) & 0x1f
}

fn frame_is_finite(frame: &BallFrame) -> bool {
    let body = &frame.body;
    let poses = [&frame.left, &frame.right];
    body.y.is_finite()
        && body.scale.is_finite()
        && body.rotate.is_finite()
        && body.breathe.is_finite()
        && frame.openness.is_finite()
        && frame
            .left_ring
            .iter()
            .all(|p| p.x.is_finite() && p.y.is_finite())
        && frame
            .right_ring
            .iter()
            .all(|p| p.x.is_finite() && p.y.is_finite())
        && poses.iter().all(|pose| {
            pose.open.is_finite()
                && pose.scale_x.is_finite()
                && pose.scale_y.is_finite()
                && pose.x.is_finite()
                && pose.y.is_finite()
                && pose.look_x.is_finite()
                && pose.look_y.is_finite()
        })
}

// ---- tests ---------------------------------------------------------------

#[test]
fn known_frame_is_deterministic() {
    let frame = known_frame();
    let mut first = [0u16; CANVAS_PIXELS];
    let mut second = [0u16; CANVAS_PIXELS];

    assert_eq!(render(&frame, &mut first), DirtyRect::FULL);
    assert_eq!(render(&frame, &mut second), DirtyRect::FULL);

    assert_eq!(first.as_slice(), second.as_slice());
    assert_ne!(hash_pixels(&first), 0);
}

#[test]
fn render_clears_stale_pixels() {
    let frame = known_frame();
    let mut fresh = [0u16; CANVAS_PIXELS];
    let mut stale = [0xffffu16; CANVAS_PIXELS];

    render(&frame, &mut fresh);
    render(&frame, &mut stale);
    assert_eq!(fresh.as_slice(), stale.as_slice());
}

#[test]
fn body_is_visible_and_eye_interior_is_dark() {
    let frame = known_frame();
    let mut pixels = [0u16; CANVAS_PIXELS];
    render(&frame, &mut pixels);

    // Body-only pixel above both eyes (source ~= 114.09, 59.87).
    let body_pixel = pixel(&pixels, 159, 92);
    assert_ne!(body_pixel, 0, "body pixel must not be background");
    assert!(
        red5(body_pixel) >= 25,
        "body pixel should be near the bright body color, got {body_pixel:#06x}"
    );
    assert_ne!(body_pixel, EYE_565);

    // Gradient center pixel (canvas 38% / 32%) versus an outer body pixel.
    let gradient_center = pixel(&pixels, 121, 102);
    let gradient_outer = pixel(&pixels, 67, 67);
    // The body gradient is ordered-dithered, so the centre may sit one
    // RGB565 step from the truncated body color in any channel.
    let body = Rgb::from_hex(0xF1EEE8).to_rgb565();
    let channels = |v: u16| [(v >> 11) as i32, ((v >> 5) & 0x3f) as i32, (v & 0x1f) as i32];
    for (got, want) in channels(gradient_center).into_iter().zip(channels(body)) {
        assert!(
            (got - want).abs() <= 1,
            "the gradient center should be the body color within one step: \
             {gradient_center:#06x} vs {body:#06x}"
        );
    }
    assert!(
        red5(gradient_center) > red5(gradient_outer),
        "gradient should darken away from 38%/32%: {gradient_center:#06x} vs {gradient_outer:#06x}"
    );

    // Left and right eye centers are fully covered and use the dark fill.
    let left_eye = pixel(&pixels, 117, 159);
    let right_eye = pixel(&pixels, 202, 159);
    assert_eq!(left_eye, EYE_565);
    assert_eq!(right_eye, EYE_565);
    for value in [left_eye, right_eye] {
        assert!(red5(value) <= 4, "eye interior must be dark");
        assert!(((value >> 5) & 0x3f) <= 8, "eye green must be dark");
        assert!((value & 0x1f) <= 4, "eye blue must be dark");
    }
}

#[test]
fn diagonal_edge_produces_intermediate_coverage() {
    let frame = triangular_frame();
    let mut pixels = [0u16; CANVAS_PIXELS];
    render(&frame, &mut pixels);

    let mut seen = [0u16; 16];
    let mut distinct = 0usize;
    let mut intermediate = 0usize;
    for &value in pixels.iter() {
        if distinct < seen.len() && !seen[..distinct].contains(&value) {
            seen[distinct] = value;
            distinct += 1;
        }
        if value != 0 && value != EYE_565 {
            intermediate += 1;
        }
    }

    assert!(
        distinct >= 4,
        "2x2 supersampling should produce several coverage levels, got {distinct} values"
    );
    assert!(
        intermediate >= 50,
        "expected many partially covered edge pixels, got {intermediate}"
    );
}

#[test]
fn extreme_poses_do_not_panic() {
    let base = known_frame();
    let mut pixels = [0u16; CANVAS_PIXELS];

    // Huge but finite body/eye transforms.
    let mut huge = base;
    huge.body.scale = 1_000_000.0;
    huge.body.rotate = 1_000_000_000.0;
    huge.body.y = -1_000_000.0;
    huge.body.breathe = 1_000_000.0;
    huge.left.open = 1_000_000.0;
    huge.left.scale_x = 1_000_000.0;
    huge.left.scale_y = -1_000_000.0;
    huge.left.x = 1_000_000_000.0;
    huge.left.y = -1_000_000_000.0;
    huge.left.look_x = f32::INFINITY;
    huge.right.open = 0.0;
    huge.right.scale_x = 0.0;
    huge.right.scale_y = 0.0;
    assert_eq!(render(&huge, &mut pixels), DirtyRect::FULL);

    // Fully collapsed body and eyes.
    let mut collapsed = base;
    collapsed.body.scale = 0.0;
    collapsed.body.breathe = -1.0;
    collapsed.body.rotate = 0.0;
    collapsed.left.open = 0.0;
    collapsed.left.scale_x = 0.0;
    collapsed.left.scale_y = 0.0;
    collapsed.right.open = 0.0;
    assert_eq!(render(&collapsed, &mut pixels), DirtyRect::FULL);

    // Non-finite pose fields.
    let mut poisoned = base;
    poisoned.body.scale = f32::NAN;
    poisoned.body.rotate = f32::NAN;
    poisoned.body.y = f32::INFINITY;
    poisoned.body.breathe = f32::NAN;
    poisoned.left.open = f32::NAN;
    poisoned.left.scale_x = f32::NEG_INFINITY;
    poisoned.left.scale_y = f32::NAN;
    poisoned.left.x = f32::NAN;
    poisoned.left.look_y = f32::INFINITY;
    poisoned.right = poisoned.left;
    assert_eq!(render(&poisoned, &mut pixels), DirtyRect::FULL);

    // Non-finite ring points.
    let mut nan_points = [Point::ZERO; BODY_POINTS];
    for (i, point) in nan_points.iter_mut().enumerate() {
        if i % 2 == 0 {
            point.x = f32::NAN;
        } else {
            point.y = f32::INFINITY;
        }
    }
    let mut nan_eye = [Point::ZERO; RING_POINTS];
    for point in nan_eye.iter_mut() {
        point.x = f32::NAN;
        point.y = f32::NEG_INFINITY;
    }
    let mut poisoned_rings = base;
    poisoned_rings.body_ring = leak_body(nan_points);
    poisoned_rings.left_ring = nan_eye;
    poisoned_rings.right_ring = nan_eye;
    assert_eq!(render(&poisoned_rings, &mut pixels), DirtyRect::FULL);
}

#[test]
fn dirty_rect_full_is_non_empty() {
    let full = DirtyRect::FULL;
    assert_eq!((full.x0, full.y0, full.x1, full.y1), (0, 0, 319, 319));
    assert!(!full.is_empty());

    let frame = known_frame();
    let mut pixels = [0u16; CANVAS_PIXELS];
    assert_eq!(render(&frame, &mut pixels), DirtyRect::FULL);
}

#[test]
fn sixty_second_timeline_is_stable() {
    use std::time::Instant;

    let states = [
        AiState::Idle,
        AiState::Listening,
        AiState::Thinking,
        AiState::Speaking,
        AiState::Happy,
        AiState::Error,
    ];

    let mut engine = BallEngine::new();
    let mut pixels = [0u16; CANVAS_PIXELS];
    let mut frames = 0u32;
    let started = Instant::now();

    // 60_000 ms / 16 ms = 3_750 frames.
    for step in 0..3_750u32 {
        if step % 500 == 0 {
            engine.set_expression(states[(step / 500) as usize % states.len()].expression());
        }
        let frame = engine.advance(16);
        assert!(frame_is_finite(&frame), "non-finite frame at step {step}");
        assert_eq!(render(&frame, &mut pixels), DirtyRect::FULL);
        assert!(
            pixels.iter().any(|&value| value != 0),
            "frame {step} has no visible pixels"
        );
        frames += 1;
    }

    let elapsed = started.elapsed();
    assert_eq!(frames, 3_750);
    eprintln!(
        "timeline: {frames} frames in {elapsed:?} ({:?}/frame)",
        elapsed / frames
    );
}

// ---- skins ---------------------------------------------------------------

#[test]
fn a_cat_skin_adds_ears_above_the_plain_body() {
    use grok_ball::raster::{render_sized_with_pose_and_skin, CanvasPose};
    use grok_ball::skin;

    let frame = known_frame();
    let mut plain = [0u16; CANVAS_PIXELS];
    let mut cat = [0u16; CANVAS_PIXELS];
    render_sized_with_pose_and_skin(
        &frame,
        &mut plain,
        320,
        grok_ball::raster::Quality::TwoByOne,
        CanvasPose::IDENTITY,
        &skin::BLOB,
    );
    render_sized_with_pose_and_skin(
        &frame,
        &mut cat,
        320,
        grok_ball::raster::Quality::TwoByOne,
        CanvasPose::IDENTITY,
        &skin::CAT,
    );

    let plain_lit = plain.iter().filter(|&&value| value != 0).count();
    let cat_lit = cat.iter().filter(|&&value| value != 0).count();
    assert!(
        cat_lit > plain_lit,
        "cat ears add pixels: plain={plain_lit} cat={cat_lit}"
    );

    // The ears sit above the body: a row inside the ear band must be lit by the
    // cat and dark for the plain blob. The synthetic body starts at source
    // y=34, which maps to canvas row ~60 at this size.
    let row = 40usize;
    let plain_row: usize = (0..320)
        .filter(|&x| plain[row * 320 + x] != 0)
        .count();
    let cat_row: usize = (0..320).filter(|&x| cat[row * 320 + x] != 0).count();
    assert_eq!(plain_row, 0, "the plain body does not reach this row");
    assert!(cat_row > 0, "the cat ears reach this row");
}

#[test]
fn a_skin_body_color_replaces_the_expression_color() {
    use grok_ball::raster::{render_sized_with_pose_and_skin, CanvasPose, Quality};
    use grok_ball::skin;

    let frame = known_frame();
    let mut pixels = [0u16; CANVAS_PIXELS];
    render_sized_with_pose_and_skin(
        &frame,
        &mut pixels,
        320,
        Quality::TwoByOne,
        CanvasPose::IDENTITY,
        &skin::FOX,
    );
    // Sample the body center, which is fully covered and therefore uses the
    // skin's base color before the radial ramp. RGB565: 5/6/5 bits.
    let center = pixels[160 * 320 + 160];
    let r = (center >> 11) & 0x1f;
    let g = (center >> 5) & 0x3f;
    let b = center & 0x1f;
    assert!(
        r > 22 && (24..48).contains(&g) && b < 18,
        "the fox body is orange, got ({r}, {g}, {b})"
    );
}
