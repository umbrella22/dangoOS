//! Independent parity checks for the generated upstream tables.
//!
//! Every expected value below is transcribed from the pinned upstream
//! `grok-ball.js` data block (lines 12-546), not read back from `data.rs`.
//! The normalization rules mirror the G0.1 contract: absent `poolMs` -> `0/0`,
//! `blinkMs: null` -> `0/0`, absent `openness` -> `1.0`, absent body fields ->
//! `0/1/0/0`, absent `body.color` -> the upstream default `#F3F0EA`, and
//! `eyes.both` is applied before per-side overrides.

use std::collections::BTreeSet;
use std::path::Path;

use grok_ball::data::{BODY_RING, LICENSE, RINGS, SEEDS, SOURCE_COMMIT, SOURCE_SHA256};
use grok_ball::types::{
    BodyParams, ExpressionSeed, EyePair, EyePose, Point, Rgb, BODY_POINTS, RING_POINTS, RING_SETS,
    SEED_COUNT,
};

/// Pinned upstream checkout used for the optional live hash comparison.
const UPSTREAM: &str = "/Volumes/project_home/github/grok-ball/src/grok-ball.js";

fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

fn pose(
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

fn pair(left: EyePose, right: EyePose) -> EyePair {
    EyePair { left, right }
}

fn body(y: f32, scale: f32, rotate: f32, breathe: f32, color: u32) -> BodyParams {
    BodyParams {
        y,
        scale,
        rotate,
        breathe,
        color: Rgb::from_hex(color),
    }
}

fn seed_value(id: u8) -> &'static ExpressionSeed {
    SEEDS
        .iter()
        .find(|seed| seed.id == id)
        .unwrap_or_else(|| panic!("missing upstream seed {id:02}"))
}

#[allow(clippy::too_many_arguments)]
fn assert_seed(
    id: u8,
    pool: &[u8],
    pool_ms: [u16; 2],
    transition_ms: u16,
    blink_ms: [u16; 2],
    openness: f32,
    expected_body: BodyParams,
    expected_eyes: EyePair,
) {
    let seed = seed_value(id);
    assert_eq!(seed.id, id, "seed {id:02}: id");
    assert_eq!(seed.pool, pool, "seed {id:02}: pool");
    assert_eq!(
        [seed.pool_min_ms, seed.pool_max_ms],
        pool_ms,
        "seed {id:02}: poolMs"
    );
    assert_eq!(
        seed.transition_ms, transition_ms,
        "seed {id:02}: transition"
    );
    assert_eq!(
        [seed.blink_min_ms, seed.blink_max_ms],
        blink_ms,
        "seed {id:02}: blinkMs"
    );
    assert_eq!(seed.openness, openness, "seed {id:02}: openness");
    assert_eq!(seed.body, expected_body, "seed {id:02}: body");
    assert_eq!(seed.eyes, expected_eyes, "seed {id:02}: eyes");
}

#[test]
fn ring_and_seed_counts_are_complete() {
    assert_eq!(RING_SETS, 25);
    assert_eq!(RING_POINTS, 48);
    assert_eq!(BODY_POINTS, 96);
    assert_eq!(SEED_COUNT, 32);

    assert_eq!(RINGS.len(), RING_SETS);
    assert_eq!(SEEDS.len(), SEED_COUNT);

    for (index, set) in RINGS.iter().enumerate() {
        assert_eq!(set.left.len(), RING_POINTS, "ring set {index}: left length");
        assert_eq!(
            set.right.len(),
            RING_POINTS,
            "ring set {index}: right length"
        );
    }

    let ids: BTreeSet<u8> = SEEDS.iter().map(|seed| seed.id).collect();
    assert_eq!(ids.len(), SEED_COUNT, "seed ids are not unique");

    for seed in SEEDS {
        assert!(!seed.pool.is_empty(), "seed {:02}: empty pool", seed.id);
        for &index in seed.pool {
            assert!(
                (index as usize) < RING_SETS,
                "seed {:02}: pool index {index} out of range",
                seed.id
            );
        }
    }
}

#[test]
fn body_ring_endpoints_match_upstream() {
    assert_eq!(BODY_RING.len(), BODY_POINTS);
    // SHAPES.blob.ring[0] and [95].
    assert_eq!(BODY_RING[0], point(228.54, 114.27));
    assert_eq!(BODY_RING[BODY_POINTS - 1], point(228.29, 106.8));
}

#[test]
fn eye_ring_endpoints_match_upstream() {
    // EXPRESSIONS[0] left/right first and last points.
    assert_eq!(RINGS[0].left[0], point(130.36, 45.98));
    assert_eq!(RINGS[0].left[RING_POINTS - 1], point(128.01, 46.19));
    assert_eq!(RINGS[0].right[0], point(176.61, 37.08));
    assert_eq!(RINGS[0].right[RING_POINTS - 1], point(174.45, 37.19));

    // One point from EXPRESSIONS[24], the last ring set.
    assert_eq!(RINGS[24].left[0], point(98.62, 121.53));
}

#[test]
fn six_ai_state_seeds_match_upstream() {
    let neutral_pose = pose(1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0);
    let neutral_eyes = pair(neutral_pose, neutral_pose);

    // 02 待机放空 / Idle: no eyes/body pose overrides; body color default.
    assert_seed(
        2,
        &[0, 8],
        [9000, 16000],
        700,
        [6000, 14000],
        1.0,
        body(0.0, 1.0, 0.0, 0.012, 0xf3f0ea),
        neutral_eyes,
    );

    // 35 等待输入 / Listening.
    assert_seed(
        35,
        &[10, 1, 19],
        [2800, 5000],
        480,
        [3000, 7000],
        1.0,
        body(0.0, 1.0, 0.0, 0.01, 0xf3f0ea),
        neutral_eyes,
    );

    // 30 思考中 / Thinking: eyes.both {lookY: -6, y: -2}.
    assert_seed(
        30,
        &[8, 16, 14, 17, 5],
        [2000, 3600],
        480,
        [3500, 7000],
        1.0,
        body(0.0, 1.0, -3.0, 0.01, 0xf3f0ea),
        pair(
            pose(1.0, 1.0, 1.0, 0.0, -2.0, 0.0, -6.0),
            pose(1.0, 1.0, 1.0, 0.0, -2.0, 0.0, -6.0),
        ),
    );

    // 39 输出回复 / Speaking: blinkMs is null -> 0/0.
    assert_seed(
        39,
        &[15, 9],
        [4000, 8000],
        360,
        [0, 0],
        1.0,
        body(0.0, 1.0, 0.0, 0.008, 0xf3f0ea),
        pair(
            pose(1.0, 1.0, 1.0, 0.0, -2.0, 0.0, 0.0),
            pose(1.0, 1.0, 1.0, 0.0, -2.0, 0.0, 0.0),
        ),
    );

    // 10 开心 / Happy: eyes.both {y: -3}, warm body color.
    assert_seed(
        10,
        &[2, 11, 17, 19],
        [2500, 4500],
        380,
        [2500, 5000],
        1.0,
        body(-3.0, 1.0, 0.0, 0.014, 0xf6efe4),
        pair(
            pose(1.0, 1.0, 1.0, 0.0, -3.0, 0.0, 0.0),
            pose(1.0, 1.0, 1.0, 0.0, -3.0, 0.0, 0.0),
        ),
    );

    // 34 出错 / Error: per-side eyes, left {y: -3}, right {y: 4}.
    assert_seed(
        34,
        &[3, 21],
        [2000, 3600],
        220,
        [0, 0],
        1.0,
        body(0.0, 1.0, -6.0, 0.0, 0xe25b5b),
        pair(
            pose(1.0, 1.0, 1.0, 0.0, -3.0, 0.0, 0.0),
            pose(1.0, 1.0, 1.0, 0.0, 4.0, 0.0, 0.0),
        ),
    );
}

#[test]
fn remaining_reachable_seeds_match_upstream() {
    let neutral_pose = pose(1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0);
    let neutral_eyes = pair(neutral_pose, neutral_pose);

    // 00 睡眠 / Sleep: openness 0.08, no blinking.
    assert_seed(
        0,
        &[13, 22, 4],
        [6000, 10000],
        900,
        [0, 0],
        0.08,
        body(4.0, 1.0, -2.0, 0.018, 0xeeebe4),
        pair(
            pose(1.0, 1.0, 1.0, 0.0, 4.0, 0.0, 2.0),
            pose(1.0, 1.0, 1.0, 0.0, 4.0, 0.0, 2.0),
        ),
    );

    // 03 好奇 / Curious.
    assert_seed(
        3,
        &[3, 21, 0, 15],
        [1800, 3200],
        420,
        [2500, 5500],
        1.0,
        body(0.0, 1.0, 4.0, 0.01, 0xf3f0ea),
        pair(
            pose(1.0, 1.0, 1.0, 0.0, 0.0, 0.0, -1.0),
            pose(1.0, 1.0, 1.0, 0.0, 0.0, 0.0, -1.0),
        ),
    );

    // 11 困惑 / Puzzled: asymmetric eye scale and position.
    assert_seed(
        11,
        &[14, 5, 8],
        [2200, 3800],
        420,
        [2800, 5500],
        1.0,
        body(0.0, 1.0, -8.0, 0.008, 0xf3f0ea),
        pair(
            pose(1.0, 1.1, 1.1, 0.0, -4.0, 0.0, 0.0),
            pose(1.0, 0.9, 0.9, 0.0, 3.0, 3.0, 0.0),
        ),
    );

    // 13 惊讶 / Surprised: body scale 1.03, enlarged eyes.
    assert_seed(
        13,
        &[3, 21],
        [2500, 4000],
        180,
        [1800, 3500],
        1.0,
        body(-4.0, 1.03, 0.0, 0.006, 0xf3f0ea),
        pair(
            pose(1.0, 1.14, 1.14, 0.0, -2.0, 0.0, -2.0),
            pose(1.0, 1.14, 1.14, 0.0, -2.0, 0.0, -2.0),
        ),
    );

    // 17 惊慌 / Panic.
    assert_seed(
        17,
        &[3, 21],
        [900, 1800],
        200,
        [1200, 3000],
        1.0,
        body(0.0, 1.0, 0.0, 0.006, 0xf3f0ea),
        neutral_eyes,
    );

    // 19 满足 / Satisfied.
    assert_seed(
        19,
        &[15, 8, 2],
        [3500, 6000],
        580,
        [3500, 7000],
        1.0,
        body(0.0, 1.0, 0.0, 0.012, 0xf5efe6),
        neutral_eyes,
    );

    // 20 疑惑 / Confused: per-side eyes.
    assert_seed(
        20,
        &[14, 5, 8],
        [2200, 3800],
        480,
        [2800, 5500],
        1.0,
        body(0.0, 1.0, -5.0, 0.008, 0xf3f0ea),
        pair(
            pose(1.0, 1.16, 1.16, 0.0, -3.0, -3.0, 0.0),
            pose(1.0, 0.8, 0.8, 0.0, 4.0, 5.0, 0.0),
        ),
    );

    // 21 生气 / Angry.
    assert_seed(
        21,
        &[7, 16],
        [2200, 3800],
        260,
        [3500, 7000],
        1.0,
        body(1.0, 1.0, 0.0, 0.004, 0xe4574a),
        neutral_eyes,
    );
}

#[test]
fn source_metadata_is_well_formed() {
    assert_eq!(LICENSE, "MIT");

    assert_eq!(SOURCE_SHA256.len(), 64, "SHA-256 must be 64 hex characters");
    assert!(
        SOURCE_SHA256
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "SHA-256 must be lowercase hex: {SOURCE_SHA256}"
    );

    assert!(
        SOURCE_COMMIT.len() == 40 || SOURCE_COMMIT.len() == 64,
        "unexpected commit id length: {}",
        SOURCE_COMMIT.len()
    );
    assert!(
        SOURCE_COMMIT
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "source commit must be lowercase hex: {SOURCE_COMMIT}"
    );
}

#[test]
fn source_sha256_matches_upstream_when_present() {
    match std::fs::read(Path::new(UPSTREAM)) {
        Ok(bytes) => {
            assert_eq!(
                sha256_hex(&bytes),
                SOURCE_SHA256,
                "generated SHA-256 does not match {UPSTREAM}"
            );
        }
        Err(error) => {
            eprintln!(
                "skipping upstream hash comparison: cannot read {UPSTREAM}: {error} \
                 (generated SOURCE_SHA256 = {SOURCE_SHA256})"
            );
        }
    }
}

#[test]
fn sha256_helper_matches_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

/// Minimal read-only SHA-256, used only to verify the recorded source digest.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);
        let (mut e, mut f, mut g, mut h) = (state[4], state[5], state[6], state[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    state.iter().map(|word| format!("{word:08x}")).collect()
}
