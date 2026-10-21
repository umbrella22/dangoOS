//! Host microbenchmark: compare the pre-skin and skin raster entry points.
//!
//! The two paths are interleaved and each round is timed separately, so
//! frequency scaling and cache warm-up cannot favour whichever runs first.

use std::time::Instant;

use grok_ball::engine::BallEngine;
use grok_ball::raster::{
    render_sized, render_sized_with_pose_and_skin, CanvasPose, Quality,
};
use grok_ball::skin;
use grok_ball::types::BallExpression;

fn main() {
    let mut engine = BallEngine::new();
    engine.set_expression(BallExpression::Idle);
    let frame = engine.advance(500);
    let mut buffer = vec![0u16; 240 * 240];

    const ROUNDS: usize = 12;
    const PER_ROUND: usize = 60;
    let mut old_ns = 0u128;
    let mut new_ns = 0u128;

    for round in 0..ROUNDS {
        if round % 2 == 0 {
            let start = Instant::now();
            for _ in 0..PER_ROUND {
                render_sized(&frame, &mut buffer, 240);
            }
            old_ns += start.elapsed().as_nanos();
            let start = Instant::now();
            for _ in 0..PER_ROUND {
                render_sized_with_pose_and_skin(
                    &frame, &mut buffer, 240, Quality::TwoByTwo, CanvasPose::IDENTITY, &skin::BLOB,
                );
            }
            new_ns += start.elapsed().as_nanos();
        } else {
            let start = Instant::now();
            for _ in 0..PER_ROUND {
                render_sized_with_pose_and_skin(
                    &frame, &mut buffer, 240, Quality::TwoByTwo, CanvasPose::IDENTITY, &skin::BLOB,
                );
            }
            new_ns += start.elapsed().as_nanos();
            let start = Instant::now();
            for _ in 0..PER_ROUND {
                render_sized(&frame, &mut buffer, 240);
            }
            old_ns += start.elapsed().as_nanos();
        }
    }

    let total = (ROUNDS * PER_ROUND) as f64;
    let old = old_ns as f64 / total / 1000.0;
    let new = new_ns as f64 / total / 1000.0;
    println!("pre-skin render_sized:            {old:.0} us/frame");
    println!("skin render_sized_with_pose:      {new:.0} us/frame");
    println!("skin overhead: {:+.1}%", 100.0 * (new - old) / old);
}
