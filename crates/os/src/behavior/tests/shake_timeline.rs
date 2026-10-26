//! Shake timelines: a pickup is nothing, sustained swaying makes the fox
//! dizzy, only a violent shake panics, and repeated short nudges anger her.

use crate::behavior::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent, DIZZY_MS,
    VIOLENT_PEAKS, VIOLENT_WINDOW_MS,
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

fn gyro(at_ms: u64, gz_dps: i16) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Imu,
        kind: EventKind::Gyro {
            gx_dps: 0,
            gy_dps: 0,
            gz_dps,
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

/// 50 Hz swaying: accel alternates between `peak_mg` dynamic and rest, the
/// gyro reads `dps`. Returns the time after the last sample.
fn sway(engine: &mut AffectEngine, from_ms: u64, ms: u64, peak_mg: i16, dps: i16) -> u64 {
    let mut at = from_ms;
    let mut n = 0u64;
    while at < from_ms + ms {
        let z = if n % 4 < 2 { 1_000 + peak_mg } else { 1_000 };
        engine.on_event(accel(at, 0, 0, z));
        engine.on_event(gyro(at, dps));
        engine.tick(at);
        at += 20;
        n += 1;
    }
    at
}

/// A violent shake: hard peaks every 60 ms with a rest sample between.
fn violent(engine: &mut AffectEngine, from_ms: u64, peaks: u64) -> u64 {
    let mut at = from_ms;
    for _ in 0..peaks {
        engine.on_event(accel(at, 0, 0, 2_500));
        engine.on_event(gyro(at, 600));
        engine.tick(at);
        engine.on_event(accel(at + 30, 0, 0, 1_000));
        at += 60;
    }
    at - 60
}

#[test]
fn picking_the_fox_up_is_no_reaction() {
    let mut engine = AffectEngine::new(0);
    // Half a second of lift: a few 400 mg samples and a quick 250 dps turn.
    let end = sway(&mut engine, 1_000, 500, 400, 250);
    assert_eq!(tick_expression(&mut engine, end), Expression::Idle);
    assert_eq!(tick_expression(&mut engine, end + 3_000), Expression::Idle);
}

#[test]
fn sustained_swaying_makes_her_dizzy() {
    let mut engine = AffectEngine::new(0);
    let mut seen = None;
    let mut at = 1_000u64;
    while at < 4_000 && seen.is_none() {
        let z = if (at / 20) % 4 < 2 { 1_700 } else { 1_000 };
        engine.on_event(accel(at, 0, 0, z));
        engine.on_event(gyro(at, 220));
        let tick = engine.tick(at);
        if tick.state.expression == Expression::Confused {
            assert_eq!(tick.state.reason, ReactionReason::ShakeSmall);
            assert_eq!(tick.haptic, Some(HapticPattern::Confused));
            seen = Some(at);
        }
        at += 20;
    }
    let dizzy_at = seen.expect("swaying for a while makes her dizzy");
    assert!(dizzy_at >= 1_500, "not on the first wobble: {dizzy_at}");

    // Still swaying: the dizzy face stays, and never turns into Panic.
    let end = sway(&mut engine, at, 2_000, 700, 220);
    assert_eq!(tick_expression(&mut engine, end), Expression::Confused);

    // It ends DIZZY_MS after the last motion.
    let last = end - 20;
    assert_eq!(
        tick_expression(&mut engine, last + DIZZY_MS - 1),
        Expression::Confused
    );
    assert_eq!(
        tick_expression(&mut engine, last + DIZZY_MS),
        Expression::Idle
    );
}

#[test]
fn a_fast_turn_alone_never_panics() {
    let mut engine = AffectEngine::new(0);
    for n in 0..10u64 {
        engine.on_event(gyro(1_000 + n * 20, 1_500));
        assert_ne!(
            tick_expression(&mut engine, 1_000 + n * 20),
            Expression::Panic
        );
    }
}

#[test]
fn a_few_hard_bumps_do_not_panic() {
    let mut engine = AffectEngine::new(0);
    // Put down hard twice: two hard peaks.
    let end = violent(&mut engine, 1_000, 2);
    assert_ne!(tick_expression(&mut engine, end), Expression::Panic);
    // The same peaks spread too far apart do not add up either.
    let mut at = 5_000;
    for _ in 0..VIOLENT_PEAKS {
        violent(&mut engine, at, 1);
        at += VIOLENT_WINDOW_MS / 2 + 100;
    }
    assert_ne!(tick_expression(&mut engine, at), Expression::Panic);
}

#[test]
fn violent_shake_panics_then_confuses_after_five_still_seconds() {
    let mut engine = AffectEngine::new(0);

    let before = violent(&mut engine, 1_000, VIOLENT_PEAKS as u64 - 1);
    assert_ne!(tick_expression(&mut engine, before), Expression::Panic);

    let t0 = violent(&mut engine, before + 60, 1);
    let panic = engine.tick(t0);
    assert_eq!(panic.state.expression, Expression::Panic);
    assert_eq!(panic.state.reason, ReactionReason::ShakeLarge);

    // Panic is [t0, t0 + 1.5 s); the last motion was the rest sample at t0 + 30
    // (a rest sample is no motion), so the still wait is measured from t0 + 1.5 s.
    assert_eq!(tick_expression(&mut engine, t0 + 1_499), Expression::Panic);
    assert_eq!(tick_expression(&mut engine, t0 + 1_500), Expression::Idle);
    assert_eq!(tick_expression(&mut engine, t0 + 6_499), Expression::Idle);
    assert_eq!(
        tick_expression(&mut engine, t0 + 6_500),
        Expression::Confused
    );
    assert_eq!(
        tick_expression(&mut engine, t0 + 9_499),
        Expression::Confused
    );
    assert_eq!(tick_expression(&mut engine, t0 + 9_500), Expression::Idle);
}

#[test]
fn five_small_bursts_trigger_angry_and_cooldown() {
    let mut engine = AffectEngine::new(0);

    // Five short nudges, each separated by a 500 ms rest gap (inside
    // 300..=800 ms). One nudge is too short to make her dizzy.
    for burst in 0..5u64 {
        let at = 1_000 + burst * 500;
        engine.on_event(small_peak(at));
        assert_eq!(tick_expression(&mut engine, at), Expression::Idle);
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
        assert_ne!(tick_expression(&mut engine, at), Expression::Angry);
    }
    assert_eq!(tick_expression(&mut engine, 16_300), Expression::Idle);
}

#[test]
fn four_small_bursts_do_not_trigger_angry() {
    let mut engine = AffectEngine::new(0);
    for burst in 0..4u64 {
        let at = 1_000 + burst * 600;
        engine.on_event(small_peak(at));
        tick_expression(&mut engine, at);
    }
    assert_eq!(tick_expression(&mut engine, 3_100), Expression::Idle);
    assert_eq!(tick_expression(&mut engine, 5_300), Expression::Idle);
}

#[test]
fn continuous_shaking_counts_as_one_burst() {
    let mut engine = AffectEngine::new(0);

    // 3 s of continuous small motion with no 300 ms rest gap: dizzy, not angry.
    let mut at = 1_000u64;
    while at <= 4_000 {
        engine.on_event(small_peak(at));
        engine.tick(at);
        at += 20;
    }
    assert_eq!(engine.tick(4_000).state.expression, Expression::Confused);

    let closed = engine.tick(4_300);
    assert_eq!(closed.state.expression, Expression::Confused);

    assert_eq!(
        tick_expression(&mut engine, 4_000 + DIZZY_MS - 1),
        Expression::Confused
    );
    assert_eq!(
        tick_expression(&mut engine, 4_000 + DIZZY_MS),
        Expression::Idle
    );
}

#[test]
fn violent_shake_clears_the_small_burst_counter() {
    let mut engine = AffectEngine::new(0);

    for burst in 0..4u64 {
        let at = 1_000 + burst * 600;
        engine.on_event(small_peak(at));
        tick_expression(&mut engine, at);
    }

    let t0 = violent(&mut engine, 4_000, VIOLENT_PEAKS as u64);
    assert_eq!(engine.tick(t0).state.expression, Expression::Panic);

    // After Panic, four small bursts start from a fresh count; without the
    // reset the earlier four bursts would make this the eighth and trigger
    // Angry.
    for burst in 0..4u64 {
        let at = 20_000 + burst * 600;
        engine.on_event(small_peak(at));
        engine.tick(at);
    }
    assert_ne!(engine.tick(22_100).state.expression, Expression::Angry);
}
