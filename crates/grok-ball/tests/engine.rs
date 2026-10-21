//! G0.2 engine acceptance tests.
//!
//! These tests use their own static ring/seed tables through
//! [`BallEngine::with_tables`] so they do not depend on the G0.1 generator.

use grok_ball::engine::{BallEngine, BallFrame};
use grok_ball::types::{
    AiState, BallExpression, BodyParams, ExpressionSeed, EyePair, EyePose, EyeRings, Point, Rgb,
    BODY_POINTS, RING_POINTS, RING_SETS,
};

// ---- synthetic tables ----------------------------------------------------

const fn make_rings() -> [EyeRings; RING_SETS] {
    let mut out = [EyeRings::EMPTY; RING_SETS];
    let mut i = 0;
    while i < RING_SETS {
        let mut p = 0;
        while p < RING_POINTS {
            let index = i as f32;
            let point = p as f32;
            out[i].left[p] = Point {
                x: 20.0 + index * 37.0 + point * 0.5,
                y: 40.0 + index * 23.0 + point * 0.25,
            };
            out[i].right[p] = Point {
                x: 180.0 + index * 37.0 + point * 0.5,
                y: 40.0 + index * 23.0 + point * 0.25,
            };
            p += 1;
        }
        i += 1;
    }
    out
}

static TEST_RINGS: [EyeRings; RING_SETS] = make_rings();

const fn body(y: f32, scale: f32, rotate: f32, breathe: f32, color: u32) -> BodyParams {
    BodyParams {
        y,
        scale,
        rotate,
        breathe,
        color: Rgb::from_hex(color),
    }
}

const fn eye(
    open: f32,
    scale_x: f32,
    scale_y: f32,
    x: f32,
    y: f32,
    look_x: f32,
    look_y: f32,
) -> EyePose {
    EyePose {
        open,
        scale_x,
        scale_y,
        x,
        y,
        look_x,
        look_y,
    }
}

const fn eyes(left: EyePose, right: EyePose) -> EyePair {
    EyePair { left, right }
}

#[allow(clippy::too_many_arguments)]
const fn seed(
    id: u8,
    pool: &'static [u8],
    pool_min_ms: u16,
    pool_max_ms: u16,
    transition_ms: u16,
    blink_min_ms: u16,
    blink_max_ms: u16,
    openness: f32,
    body: BodyParams,
    eyes: EyePair,
) -> ExpressionSeed {
    ExpressionSeed {
        id,
        pool,
        pool_min_ms,
        pool_max_ms,
        transition_ms,
        blink_min_ms,
        blink_max_ms,
        openness,
        body,
        eyes,
    }
}

/// Six seeds whose ids match the six [`AiState`] expressions, each with a
/// distinct ring set and a distinct pose.
static AI_SEEDS: [ExpressionSeed; 6] = [
    seed(
        2,
        &[0],
        0,
        0,
        0,
        0,
        0,
        1.00,
        body(0.0, 1.00, 0.0, 0.010, 0xF3F0EA),
        eyes(
            eye(1.00, 1.00, 1.00, 0.0, 0.0, 0.0, 0.0),
            eye(1.00, 1.00, 1.00, 0.0, 0.0, 0.0, 0.0),
        ),
    ),
    seed(
        35,
        &[1],
        0,
        0,
        0,
        0,
        0,
        0.95,
        body(1.5, 1.01, 1.0, 0.020, 0xF4C34E),
        eyes(
            eye(0.90, 1.05, 1.05, 1.0, -1.0, -2.0, 1.0),
            eye(0.90, 1.05, 1.05, -1.0, 2.0, 2.0, -1.0),
        ),
    ),
    seed(
        30,
        &[2],
        0,
        0,
        0,
        0,
        0,
        1.00,
        body(2.0, 1.02, -2.0, 0.015, 0xF1EEE8),
        eyes(
            eye(1.00, 0.95, 0.95, 0.0, 2.0, 4.0, -3.0),
            eye(1.00, 0.95, 0.95, 0.0, -2.0, -4.0, 3.0),
        ),
    ),
    seed(
        39,
        &[3],
        0,
        0,
        0,
        0,
        0,
        0.98,
        body(-1.0, 0.99, 3.0, 0.012, 0xF6EFE4),
        eyes(
            eye(1.05, 1.10, 0.90, 2.0, 0.0, 3.0, 0.0),
            eye(1.05, 0.90, 1.10, -2.0, 0.0, -3.0, 0.0),
        ),
    ),
    seed(
        10,
        &[4],
        0,
        0,
        0,
        0,
        0,
        1.00,
        body(-3.0, 1.03, 2.0, 0.025, 0xF4C34E),
        eyes(
            eye(1.10, 1.12, 1.12, 0.0, -3.0, 0.0, -4.0),
            eye(1.10, 1.12, 1.12, 0.0, -3.0, 0.0, -4.0),
        ),
    ),
    seed(
        34,
        &[5],
        0,
        0,
        0,
        0,
        0,
        1.00,
        body(1.0, 1.00, -6.0, 0.008, 0xE25B5B),
        eyes(
            eye(1.00, 1.00, 1.00, 0.0, -3.0, 0.0, 0.0),
            eye(1.00, 1.20, 1.20, 0.0, 4.0, 0.0, 0.0),
        ),
    ),
];

static TRANSITION_FROM: ExpressionSeed = seed(
    0x80,
    &[0],
    0,
    0,
    0,
    0,
    0,
    1.0,
    BodyParams::DEFAULT,
    EyePair::DEFAULT,
);

static TRANSITION_TO: ExpressionSeed = seed(
    0x81,
    &[1],
    0,
    0,
    1_000,
    0,
    0,
    1.0,
    BodyParams::DEFAULT,
    EyePair::DEFAULT,
);

static ROTATE_SEED: ExpressionSeed = seed(
    0x82,
    &[0, 1],
    100,
    100,
    0,
    0,
    0,
    1.0,
    BodyParams::DEFAULT,
    EyePair::DEFAULT,
);

static NO_ROTATE_MIN_ZERO: ExpressionSeed = seed(
    0x83,
    &[0, 1],
    0,
    500,
    0,
    0,
    0,
    1.0,
    BodyParams::DEFAULT,
    EyePair::DEFAULT,
);

static BLINK_SEED: ExpressionSeed = seed(
    0x84,
    &[0],
    0,
    0,
    0,
    1_000,
    1_000,
    1.0,
    BodyParams::DEFAULT,
    EyePair::DEFAULT,
);

static BUSY_SEED: ExpressionSeed = seed(
    0x85,
    &[0, 1, 2],
    200,
    500,
    300,
    150,
    400,
    0.92,
    body(2.0, 1.01, 4.0, 0.030, 0xF3F0EA),
    eyes(
        eye(1.10, 1.05, 0.95, 1.0, -2.0, 3.0, -2.0),
        eye(0.85, 0.95, 1.05, -1.0, 2.0, -3.0, 2.0),
    ),
);

// ---- helpers -------------------------------------------------------------

fn signature(frame: &BallFrame) -> Vec<u32> {
    let mut out = Vec::with_capacity(2 * (BODY_POINTS + 2 * RING_POINTS) + 24);
    for point in frame
        .body_ring
        .iter()
        .chain(frame.left_ring.iter())
        .chain(frame.right_ring.iter())
    {
        out.push(point.x.to_bits());
        out.push(point.y.to_bits());
    }

    for value in [
        frame.body.y,
        frame.body.scale,
        frame.body.rotate,
        frame.body.breathe,
        frame.openness,
    ] {
        out.push(value.to_bits());
    }
    out.push(
        ((frame.body.color.r as u32) << 16)
            | ((frame.body.color.g as u32) << 8)
            | frame.body.color.b as u32,
    );

    for pose in [frame.left, frame.right] {
        for value in [
            pose.open,
            pose.scale_x,
            pose.scale_y,
            pose.x,
            pose.y,
            pose.look_x,
            pose.look_y,
        ] {
            out.push(value.to_bits());
        }
    }

    out
}

fn rings_match(actual: &[Point; RING_POINTS], expected: &[Point; RING_POINTS]) -> bool {
    actual
        .iter()
        .zip(expected.iter())
        .all(|(a, b)| a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits())
}

fn assert_finite_frame(frame: &BallFrame, step: usize) {
    for (index, point) in frame.body_ring.iter().enumerate() {
        assert!(
            point.x.is_finite() && point.y.is_finite(),
            "body_ring[{index}] non-finite at step {step}"
        );
    }
    for (ring_index, ring) in [frame.left_ring, frame.right_ring].iter().enumerate() {
        for (index, point) in ring.iter().enumerate() {
            assert!(
                point.x.is_finite() && point.y.is_finite(),
                "ring {ring_index}[{index}] non-finite at step {step}"
            );
        }
    }

    let body = frame.body;
    assert!(
        body.y.is_finite()
            && body.scale.is_finite()
            && body.rotate.is_finite()
            && body.breathe.is_finite(),
        "body non-finite at step {step}: {body:?}"
    );
    assert!(
        frame.openness.is_finite(),
        "openness non-finite at step {step}"
    );

    for (index, pose) in [frame.left, frame.right].iter().enumerate() {
        assert!(
            pose.open.is_finite()
                && pose.scale_x.is_finite()
                && pose.scale_y.is_finite()
                && pose.x.is_finite()
                && pose.y.is_finite()
                && pose.look_x.is_finite()
                && pose.look_y.is_finite(),
            "eye pose {index} non-finite at step {step}: {pose:?}"
        );
    }
}

fn engine() -> BallEngine {
    BallEngine::with_tables(&TEST_RINGS, &AI_SEEDS)
}

// ---- tests ---------------------------------------------------------------

#[test]
fn six_ai_states_produce_distinct_frames() {
    let states = [
        AiState::Idle,
        AiState::Listening,
        AiState::Thinking,
        AiState::Speaking,
        AiState::Happy,
        AiState::Error,
    ];

    let mut signatures = Vec::with_capacity(states.len());
    for state in states {
        let mut engine = engine();
        engine.set_expression(state.expression());
        let frame = engine.advance(0);
        signatures.push(signature(&frame));
    }

    for i in 0..signatures.len() {
        for j in (i + 1)..signatures.len() {
            assert_ne!(
                signatures[i], signatures[j],
                "AI states {i} and {j} produced identical frame signatures"
            );
        }
    }
}

#[test]
fn transition_midpoint_lies_between_source_and_target_rings() {
    let mut engine = engine();
    engine.set_seed(&TRANSITION_FROM);
    let frame = engine.advance(0);
    assert!(
        rings_match(&frame.left_ring, &TEST_RINGS[0].left),
        "initial seed did not snap to its pool ring"
    );

    engine.set_seed(&TRANSITION_TO);
    let midpoint = engine.advance(500);

    for index in 0..RING_POINTS {
        let expected_left = (TEST_RINGS[0].left[index].x + TEST_RINGS[1].left[index].x) * 0.5;
        let expected_right = (TEST_RINGS[0].right[index].y + TEST_RINGS[1].right[index].y) * 0.5;
        assert!(
            (midpoint.left_ring[index].x - expected_left).abs() < 1.0e-3,
            "left x midpoint mismatch at point {index}: {} vs {expected_left}",
            midpoint.left_ring[index].x
        );
        assert!(
            (midpoint.right_ring[index].y - expected_right).abs() < 1.0e-3,
            "right y midpoint mismatch at point {index}: {} vs {expected_right}",
            midpoint.right_ring[index].y
        );
    }

    let settled = engine.advance(500);
    assert!(
        rings_match(&settled.left_ring, &TEST_RINGS[1].left),
        "transition did not settle on the target ring"
    );
    assert!(rings_match(&settled.right_ring, &TEST_RINGS[1].right));
}

#[test]
fn pool_rotation_occurs_after_the_hold_and_wraps() {
    let mut engine = engine();
    engine.set_seed(&ROTATE_SEED);

    let frame = engine.advance(99);
    assert!(rings_match(&frame.left_ring, &TEST_RINGS[0].left));

    let frame = engine.advance(1);
    assert!(
        rings_match(&frame.left_ring, &TEST_RINGS[1].left),
        "pool did not advance at the hold boundary"
    );

    let frame = engine.advance(99);
    assert!(rings_match(&frame.left_ring, &TEST_RINGS[1].left));

    let frame = engine.advance(1);
    assert!(
        rings_match(&frame.left_ring, &TEST_RINGS[0].left),
        "pool rotation did not wrap"
    );
}

#[test]
fn pool_rotation_is_disabled_when_min_hold_is_zero() {
    let mut engine = engine();
    engine.set_seed(&NO_ROTATE_MIN_ZERO);

    for step in 0..100 {
        let frame = engine.advance(16);
        assert!(
            rings_match(&frame.left_ring, &TEST_RINGS[0].left),
            "pool rotated despite pool_min_ms == 0 at step {step}"
        );
    }
}

#[test]
fn blink_occurs_within_the_configured_window() {
    let mut engine = engine();
    engine.set_seed(&BLINK_SEED);

    let mut elapsed = 0u32;
    let mut first_dip = None;
    for _ in 0..110 {
        elapsed += 16;
        let frame = engine.advance(16);
        if elapsed < 1_000 {
            assert!(
                frame.openness > 0.95,
                "blink started before the configured window at {elapsed} ms: {}",
                frame.openness
            );
        }
        if frame.openness < 0.6 {
            first_dip = Some(elapsed);
            break;
        }
    }

    let dip = first_dip.expect("no blink occurred");
    assert!(
        (1_000..=1_400).contains(&dip),
        "blink dip at {dip} ms outside the configured window"
    );

    let mut recovered = false;
    while elapsed < 1_900 {
        elapsed += 16;
        let frame = engine.advance(16);
        if frame.openness > 0.95 {
            recovered = true;
            break;
        }
    }
    assert!(recovered, "eyes did not reopen after the blink");
}

#[test]
fn identical_timelines_are_byte_identical() {
    let mut first = engine();
    let mut second = engine();
    first.set_seed(&BUSY_SEED);
    second.set_seed(&BUSY_SEED);

    for step in 0..2_000 {
        let a = first.advance(16);
        let b = second.advance(16);
        assert_eq!(
            signature(&a),
            signature(&b),
            "frame mismatch between identical runs at step {step}"
        );
    }
}

#[test]
fn frames_are_finite_over_sixty_seconds() {
    let mut engine = engine();
    engine.set_seed(&BUSY_SEED);

    for step in 0..(60_000 / 16) {
        let frame = engine.advance(16);
        assert_finite_frame(&frame, step);
    }

    // Huge and zero steps must not panic or leak non-finite values.
    let frame = engine.advance(u32::MAX);
    assert_finite_frame(&frame, usize::MAX);
    let frame = engine.advance(0);
    assert_finite_frame(&frame, usize::MAX);
}

#[test]
fn advance_zero_and_duplicate_expression_are_noops() {
    let mut first = engine();
    let mut second = engine();
    first.set_expression(BallExpression::Idle);
    second.set_expression(BallExpression::Idle);
    // The second selection must not restart timers or transitions.
    second.set_expression(BallExpression::Idle);

    for step in 0..300 {
        let a = first.advance(16);
        let b = second.advance(16);
        assert_eq!(
            signature(&a),
            signature(&b),
            "duplicate set_expression changed behavior at step {step}"
        );

        let before = signature(&a);
        let zero = first.advance(0);
        assert_eq!(
            before,
            signature(&zero),
            "advance(0) changed the frame at step {step}"
        );
    }
}

#[test]
fn missing_expression_seed_falls_back_without_panicking() {
    let mut engine = engine();
    engine.set_expression(BallExpression::Idle);
    let _ = engine.advance(64);

    // The synthetic table has no seed 0; this must fall back to EMPTY.
    engine.set_expression(BallExpression::Sleep);
    let frame = engine.advance(500);
    assert_finite_frame(&frame, 0);
}

#[test]
fn real_data_resolves_every_reachable_expression() {
    for expression in BallExpression::ALL {
        let id = expression.seed_id();
        let seed = grok_ball::data::seed(id)
            .unwrap_or_else(|| panic!("missing upstream seed {id} for {expression:?}"));
        assert_eq!(seed.id, id, "seed id mismatch for {expression:?}");
        assert!(!seed.pool.is_empty(), "empty pool for {expression:?}");
        assert!(
            (seed.pool[0] as usize) < RING_SETS,
            "pool index out of range for {expression:?}"
        );

        let mut engine = BallEngine::new();
        engine.set_expression(expression);
        let frame = engine.advance(250);
        assert_finite_frame(&frame, 0);
    }
}
