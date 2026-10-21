//! H0 shake timelines: panic, small-shake confusion, repeated-burst anger,
//! cooldown, and continuous-shake segmentation.

use crate::behavior::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent,
};
use crate::behavior::Expression;

fn accel(at_ms: u64, ax_mg: i16, ay_mg: i16, az_mg: i16) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Imu,
        kind: EventKind::Accel {
            ax_mg,
            ay_mg,
            az_mg,
        },
    }
}

/// One small-shake peak: z = 1500 mg gives 500 mg dynamic.
fn small_peak(at_ms: u64) -> SensorEvent {
    accel(at_ms, 0, 0, 1_500)
}

fn tick_expression(engine: &mut AffectEngine, at_ms: u64) -> Expression {
    engine.tick(at_ms).state.expression
}

#[test]
fn large_shake_panics_then_confuses_after_five_still_seconds() {
    let t0 = 1_000u64;
    let mut engine = AffectEngine::new(0);

    // One large peak alone is not enough.
    engine.on_event(accel(t0 - 50, 0, 0, 2_500));
    assert_eq!(tick_expression(&mut engine, t0 - 50), Expression::Idle);

    // The second peak within 200 ms triggers Panic at t0.
    engine.on_event(accel(t0, 0, 0, 2_500));
    let panic = engine.tick(t0);
    assert_eq!(panic.state.expression, Expression::Panic);
    assert_eq!(panic.state.reason, ReactionReason::ShakeLarge);
    assert_eq!(panic.haptic, Some(HapticPattern::Panic));

    // Panic is [t0, t0 + 1.5 s).
    assert_eq!(tick_expression(&mut engine, t0 + 1_499), Expression::Panic);
    assert_eq!(tick_expression(&mut engine, t0 + 1_500), Expression::Idle);

    // Device is still: no motion until t0 + 6.5 s.
    assert_eq!(tick_expression(&mut engine, t0 + 6_499), Expression::Idle);
    assert_eq!(
        tick_expression(&mut engine, t0 + 6_500),
        Expression::Confused
    );

    // Confused holds 3 s, then returns.
    assert_eq!(
        tick_expression(&mut engine, t0 + 9_499),
        Expression::Confused
    );
    assert_eq!(tick_expression(&mut engine, t0 + 9_500), Expression::Idle);
}

#[test]
fn small_shake_confuses_for_2500_ms() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(small_peak(1_000));

    let confused = engine.tick(1_000);
    assert_eq!(confused.state.expression, Expression::Confused);
    assert_eq!(confused.state.reason, ReactionReason::ShakeSmall);
    assert_eq!(confused.haptic, Some(HapticPattern::Confused));

    assert_eq!(tick_expression(&mut engine, 3_499), Expression::Confused);
    assert_eq!(tick_expression(&mut engine, 3_500), Expression::Idle);
}

#[test]
fn six_small_bursts_trigger_angry_and_cooldown() {
    let mut engine = AffectEngine::new(0);

    // Five bursts, each separated by a 500 ms rest gap (inside 300..=800 ms).
    for burst in 0..5u64 {
        let at = 1_000 + burst * 500;
        engine.on_event(small_peak(at));
        assert_eq!(tick_expression(&mut engine, at), Expression::Confused);
    }

    // The fifth burst closes 300 ms after its last motion: Angry starts.
    let angry = engine.tick(3_300);
    assert_eq!(angry.state.expression, Expression::Angry);
    assert_eq!(angry.state.reason, ReactionReason::ShakeBursts);
    assert_eq!(angry.haptic, Some(HapticPattern::Angry));

    // A sixth burst while Angry holds does not restart or stack anything.
    engine.on_event(small_peak(3_500));
    assert_eq!(tick_expression(&mut engine, 3_500), Expression::Angry);

    assert_eq!(tick_expression(&mut engine, 13_299), Expression::Angry);
    assert_eq!(tick_expression(&mut engine, 13_300), Expression::Idle);

    // The 30 s cooldown runs after the 10 s hold (until 43_300). Five more
    // bursts inside the cooldown must not produce Angry.
    for burst in 0..5u64 {
        let at = 14_000 + burst * 500;
        engine.on_event(small_peak(at));
        let expression = tick_expression(&mut engine, at);
        assert_ne!(expression, Expression::Angry);
    }
    let after_close = engine.tick(16_300);
    assert_ne!(after_close.state.expression, Expression::Angry);
    assert_eq!(after_close.state.expression, Expression::Confused);

    // The latest small-shake Confused ends 2.5 s after its last peak.
    assert_eq!(tick_expression(&mut engine, 18_500), Expression::Idle);
}

#[test]
fn four_small_bursts_do_not_trigger_angry() {
    let mut engine = AffectEngine::new(0);
    for burst in 0..4u64 {
        let at = 1_000 + burst * 600;
        engine.on_event(small_peak(at));
        tick_expression(&mut engine, at);
    }

    let after_close = engine.tick(3_100);
    assert_eq!(after_close.state.expression, Expression::Confused);
    assert_ne!(after_close.state.expression, Expression::Angry);

    assert_eq!(tick_expression(&mut engine, 5_300), Expression::Idle);
}

#[test]
fn continuous_shaking_counts_as_one_burst() {
    let mut engine = AffectEngine::new(0);

    // 3 s of continuous small motion with no 300 ms rest gap.
    let mut at = 1_000u64;
    while at <= 4_000 {
        engine.on_event(small_peak(at));
        engine.tick(at);
        at += 50;
    }

    assert_eq!(engine.tick(4_000).state.expression, Expression::Confused);

    // The burst closes 300 ms after the last motion; the count is one.
    let closed = engine.tick(4_300);
    assert_eq!(closed.state.expression, Expression::Confused);
    assert_ne!(closed.state.expression, Expression::Angry);

    assert_eq!(tick_expression(&mut engine, 6_499), Expression::Confused);
    assert_eq!(tick_expression(&mut engine, 6_500), Expression::Idle);
}

#[test]
fn large_shake_clears_the_small_burst_counter() {
    let mut engine = AffectEngine::new(0);

    // Four small bursts.
    for burst in 0..4u64 {
        let at = 1_000 + burst * 600;
        engine.on_event(small_peak(at));
        tick_expression(&mut engine, at);
    }

    // Large shake at 4_000/4_050 clears the counter.
    engine.on_event(accel(4_000, 0, 0, 2_500));
    engine.on_event(accel(4_050, 0, 0, 2_500));
    assert_eq!(engine.tick(4_050).state.expression, Expression::Panic);

    // After Panic, four small bursts start from a fresh count; without the
    // reset the earlier four bursts would make this the eighth and trigger
    // Angry.
    for burst in 0..4u64 {
        let at = 20_000 + burst * 600;
        engine.on_event(small_peak(at));
        engine.tick(at);
    }
    let expression = engine.tick(22_100).state.expression;
    assert_ne!(expression, Expression::Angry);
    assert_eq!(expression, Expression::Confused);
}
