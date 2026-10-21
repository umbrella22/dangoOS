//! H0 ambient timelines: relative LDR deltas, gradual dawn, night sleep, PIR
//! debounce, sustained presence, and absence.

use crate::behavior::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent,
    DAWN_CURIOUS_MS, LIGHT_BRIGHT_MS, LIGHT_DARK_MS, MIN_DWELL_MS, PIR_CURIOUS_MS,
    PIR_SATISFIED_MS,
};
use crate::behavior::Expression;

fn light(at_ms: u64, level: u8) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Light,
        kind: EventKind::LightLevel(level),
    }
}

fn presence(at_ms: u64, present: bool) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Pir,
        kind: EventKind::Presence(present),
    }
}

fn touch(at_ms: u64, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Touch,
        kind,
    }
}

#[test]
fn sudden_dark_is_confused_for_the_hold_time() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 80));
    assert_eq!(engine.tick(0).state.expression, Expression::Idle);

    engine.on_event(light(100, 50));
    let dark = engine.tick(100);
    assert_eq!(dark.state.expression, Expression::Confused);
    assert_eq!(dark.state.reason, ReactionReason::Light);
    assert_eq!(dark.haptic, Some(HapticPattern::Confused));

    let end = 100 + LIGHT_DARK_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Confused);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);
}

#[test]
fn sudden_bright_is_surprised_for_the_hold_time() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 50));
    engine.on_event(light(100, 80));
    let bright = engine.tick(100);
    assert_eq!(bright.state.expression, Expression::Surprised);
    assert_eq!(bright.state.reason, ReactionReason::Light);
    assert_eq!(bright.haptic, Some(HapticPattern::Surprised));

    let end = 100 + LIGHT_BRIGHT_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Surprised);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);
}

#[test]
fn dark_then_bright_recovery_replaces_confused() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 80));
    engine.on_event(light(100, 50));
    assert_eq!(engine.tick(100).state.expression, Expression::Confused);

    // Keep a recent dark reference and then jump +30 points.
    engine.on_event(light(400, 50));
    engine.on_event(light(500, 80));
    let recovered = engine.tick(500);
    assert_eq!(recovered.state.expression, Expression::Surprised);
    assert_eq!(recovered.state.reason, ReactionReason::Light);
    assert!(recovered.changed);

    let end = 500 + LIGHT_BRIGHT_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Surprised);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);
}

#[test]
fn bright_delta_is_measured_across_the_300_ms_window() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 50));
    engine.on_event(light(100, 60));
    assert_eq!(engine.tick(100).state.expression, Expression::Idle);

    // 75 - 50 = +25 across 200 ms: the reference is the oldest sample inside
    // the 300 ms window, not only the previous sample.
    engine.on_event(light(200, 75));
    let bright = engine.tick(200);
    assert_eq!(bright.state.expression, Expression::Surprised);
    assert_eq!(bright.state.reason, ReactionReason::Light);
}

#[test]
fn gradual_dawn_is_curious_not_surprised() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 10));
    assert_eq!(engine.tick(0).state.expression, Expression::Idle);

    // +1 point per second for 30 s: no single sample is a fast +25 delta.
    for second in 1..=30u64 {
        let level = (10 + second) as u8;
        engine.on_event(light(second * 1_000, level));
        let expression = engine.tick(second * 1_000).state.expression;
        assert_ne!(expression, Expression::Surprised);
        assert_ne!(expression, Expression::Confused);
    }

    // The 30 s rise reaches +30 and produces Curious.
    let end = 30_000 + DAWN_CURIOUS_MS;
    assert_eq!(engine.tick(30_000).state.expression, Expression::Curious);
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Curious);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);
}

#[test]
fn ninety_seconds_of_darkness_sleeps_and_touch_wakes() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 5));
    assert_eq!(engine.tick(0).state.expression, Expression::Idle);

    assert_eq!(engine.tick(59_999).state.expression, Expression::Idle);
    assert_eq!(engine.tick(60_000).state.expression, Expression::Sleep);
    assert_eq!(engine.tick(70_000).state.expression, Expression::Sleep);

    engine.on_event(touch(70_100, EventKind::TouchPoke));
    let woken = engine.tick(70_100);
    assert_eq!(woken.state.expression, Expression::Puzzled);

    // The poke itself ends 1.5 s after the wake.
    assert_eq!(engine.tick(71_600).state.expression, Expression::Idle);
}

#[test]
fn bright_light_wakes_from_sleep() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 5));
    assert_eq!(engine.tick(60_000).state.expression, Expression::Sleep);

    // Refresh a dark sample, then jump +25 points.
    engine.on_event(light(60_000, 5));
    engine.on_event(light(60_100, 30));
    let woken = engine.tick(60_100);
    assert_eq!(woken.state.expression, Expression::Surprised);

    assert_eq!(
        engine.tick(60_100 + LIGHT_BRIGHT_MS).state.expression,
        Expression::Idle
    );
}

#[test]
fn pir_presence_rising_sustained_and_absence() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(presence(100, true));
    assert_eq!(engine.tick(100).state.expression, Expression::Idle);

    // Debounce: the confirmed rising edge is at 300 ms.
    assert_eq!(engine.tick(299).state.expression, Expression::Idle);
    let curious = engine.tick(300);
    assert_eq!(curious.state.expression, Expression::Curious);
    assert_eq!(curious.state.reason, ReactionReason::Presence);
    assert_eq!(curious.haptic, None);

    let end = 300 + PIR_CURIOUS_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Curious);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);

    // Ten seconds of continuous presence, measured from the confirmed edge.
    assert_eq!(engine.tick(10_299).state.expression, Expression::Idle);
    let sustained = engine.tick(10_300);
    assert_eq!(sustained.state.expression, Expression::Satisfied);
    assert_eq!(sustained.haptic, Some(HapticPattern::Satisfied));

    let end = 10_300 + PIR_SATISFIED_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Satisfied);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);

    // Falling edge: no immediate reaction.
    let gone = end + 200;
    engine.on_event(presence(gone, false));
    assert_eq!(engine.tick(gone).state.expression, Expression::Idle);
    assert_eq!(engine.tick(gone + 200).state.expression, Expression::Idle);

    // Absence timer: 60 s after the confirmed falling edge.
    let asleep = gone + 200 + 60_000;
    assert_eq!(engine.tick(asleep - 1).state.expression, Expression::Idle);
    assert_eq!(engine.tick(asleep).state.expression, Expression::Sleep);
}

#[test]
fn short_pir_pulse_is_ignored_by_debounce() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(presence(100, true));
    engine.on_event(presence(200, false));

    for at in [300u64, 1_000, 10_000] {
        let expression = engine.tick(at).state.expression;
        assert_ne!(expression, Expression::Curious);
        assert_ne!(expression, Expression::Satisfied);
    }
}

#[test]
fn a_fresh_face_is_not_replaced_by_another_passive_sensor() {
    let mut engine = AffectEngine::new(0);

    // PIR Curious at 300 ms.
    engine.on_event(presence(100, true));
    engine.on_event(light(100, 50));
    assert_eq!(engine.tick(300).state.expression, Expression::Curious);

    // A light jump right after does not replace it.
    engine.on_event(light(400, 80));
    let held = engine.tick(400);
    assert_eq!(held.state.expression, Expression::Curious);
    assert_eq!(held.state.reason, ReactionReason::Presence);

    // After the minimum stay a new light jump does.
    let later = 300 + MIN_DWELL_MS;
    engine.on_event(light(later - 100, 50));
    engine.on_event(light(later, 80));
    let replaced = engine.tick(later);
    assert_eq!(replaced.state.expression, Expression::Surprised);
    assert_eq!(replaced.state.reason, ReactionReason::Light);
}
