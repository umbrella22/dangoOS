//! Sleep and waking: every interaction wakes her, holds sleep off for
//! `AWAKE_HOLD_MS`, and shows a drowsy or grumpy face depending on how she
//! was woken. A low battery shows through sleep.

use crate::behavior::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent,
    AWAKE_HOLD_MS, WAKE_DROWSY_MS, WAKE_GRUMPY_MS,
};
use crate::behavior::Expression;

fn light(at_ms: u64, level: u8) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Light,
        kind: EventKind::LightLevel(level),
    }
}

fn touch(at_ms: u64, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Touch,
        kind,
    }
}

fn accel(at_ms: u64, az_mg: i16) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Imu,
        kind: EventKind::Accel {
            ax_mg: 0,
            ay_mg: 0,
            az_mg,
        },
    }
}

fn battery(at_ms: u64, percent: u8) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Battery,
        kind: EventKind::BatteryLevel(percent),
    }
}

/// A dark room from t = 0: asleep at 60 s.
fn asleep_engine() -> (AffectEngine, u64) {
    let mut engine = AffectEngine::new(0);
    engine.on_event(light(0, 5));
    engine.tick(0);
    assert_eq!(engine.tick(60_000).state.expression, Expression::Sleep);
    (engine, 61_000)
}

#[test]
fn a_tap_wakes_her_drowsy_and_she_stays_up_for_the_hold() {
    let (mut engine, at) = asleep_engine();

    assert!(engine.on_screen_touch(at), "the tap is spent on waking up");
    let woken = engine.tick(at).state;
    assert_eq!(woken.expression, Expression::Tired);
    assert_eq!(woken.reason, ReactionReason::Wake);
    assert_eq!(
        engine.tick(at + WAKE_DROWSY_MS).state.expression,
        Expression::Idle
    );

    // The room is still dark (the LDR keeps reporting), but the hold keeps
    // her awake.
    engine.on_event(light(at + 100, 5));
    assert_eq!(
        engine.tick(at + AWAKE_HOLD_MS - 1).state.expression,
        Expression::Idle
    );
    assert_eq!(
        engine.tick(at + AWAKE_HOLD_MS).state.expression,
        Expression::Sleep
    );
}

#[test]
fn every_interaction_extends_the_hold() {
    let (mut engine, at) = asleep_engine();
    engine.on_screen_touch(at);
    let later = at + 4 * 60_000;
    engine.on_event(light(at + 100, 5));
    assert!(!engine.on_screen_touch(later), "awake: an ordinary tap");
    engine.on_event(light(later + 100, 5));
    assert_eq!(
        engine.tick(at + AWAKE_HOLD_MS).state.expression,
        Expression::Idle
    );
    assert_eq!(
        engine.tick(later + AWAKE_HOLD_MS).state.expression,
        Expression::Sleep
    );
}

#[test]
fn poking_her_again_while_drowsy_makes_her_grumpy() {
    let (mut engine, at) = asleep_engine();
    engine.on_screen_touch(at);
    assert!(engine.on_screen_touch(at + 800));
    let tick = engine.tick(at + 800);
    assert_eq!(tick.state.expression, Expression::Angry);
    assert_eq!(tick.state.reason, ReactionReason::Wake);
    assert_eq!(tick.haptic, Some(HapticPattern::Angry));
    assert_eq!(
        engine.tick(at + 800 + WAKE_GRUMPY_MS).state.expression,
        Expression::Idle
    );
}

#[test]
fn a_stroke_wakes_her_drowsy_and_is_not_a_pet() {
    let (mut engine, at) = asleep_engine();
    engine.on_event(touch(at, EventKind::TouchPet));
    assert_eq!(engine.tick(at).state.expression, Expression::Tired);
}

#[test]
fn a_board_poke_also_wakes_her_drowsy() {
    let (mut engine, at) = asleep_engine();
    engine.on_event(touch(at, EventKind::TouchPoke));
    assert_eq!(engine.tick(at).state.expression, Expression::Tired);
}

#[test]
fn a_knock_wakes_her_grumpy_and_lifting_her_drowsy() {
    let (mut engine, at) = asleep_engine();
    engine.on_event(accel(at, 1_000 + 1_200));
    let tick = engine.tick(at);
    assert_eq!(tick.state.expression, Expression::Angry);
    assert_eq!(tick.state.reason, ReactionReason::Wake);

    let (mut engine, at) = asleep_engine();
    engine.on_event(accel(at, 1_000 + 150));
    assert_eq!(engine.tick(at).state.expression, Expression::Tired);
}

#[test]
fn using_an_app_keeps_her_awake_without_a_face() {
    let (mut engine, at) = asleep_engine();
    engine.on_interaction(at);
    assert_eq!(engine.tick(at).state.expression, Expression::Idle);
    engine.on_event(light(at + 100, 5));
    assert!(!engine.is_asleep());
    assert_eq!(
        engine.tick(at + AWAKE_HOLD_MS).state.expression,
        Expression::Sleep
    );
}

#[test]
fn a_low_battery_wakes_her_up_tired() {
    let (mut engine, at) = asleep_engine();
    engine.on_event(battery(at, 12));
    let tick = engine.tick(at).state;
    assert_eq!(tick.expression, Expression::Tired);
    assert_eq!(tick.reason, ReactionReason::Battery);
}

#[test]
fn removing_the_board_still_lets_her_fall_asleep() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(SensorEvent {
        at_ms: 100,
        source: EventSource::ModuleRight,
        kind: EventKind::ModuleRemoved(crate::behavior::affect::ModuleKind::Interaction),
    });
    engine.on_event(light(100, 5));
    engine.tick(100);
    assert_eq!(engine.tick(60_100).state.expression, Expression::Sleep);
}
