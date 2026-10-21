//! H0 key and button timelines: KEY_L, KEY_R, triple R, both keys, power
//! short/long/double, and the debug-only boot button.

use crate::behavior::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent, SystemCommand,
};
use crate::behavior::Expression;

fn key(at_ms: u64, source: EventSource, down: bool) -> SensorEvent {
    SensorEvent {
        at_ms,
        source,
        kind: if down {
            EventKind::KeyDown
        } else {
            EventKind::KeyUp
        },
    }
}

fn button(at_ms: u64, source: EventSource, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source,
        kind,
    }
}

#[test]
fn key_left_soothes_happy_while_held_and_after_release() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(key(100, EventSource::KeyLeft, true));
    let pressed = engine.tick(100);
    assert_eq!(pressed.state.expression, Expression::Happy);
    assert_eq!(pressed.state.reason, ReactionReason::KeyLeft);
    assert_eq!(pressed.haptic, Some(HapticPattern::Happy));

    // Still held after 1.5 s: the reaction is extended.
    assert_eq!(engine.tick(1_000).state.expression, Expression::Happy);

    engine.on_event(key(1_200, EventSource::KeyLeft, false));
    assert_eq!(engine.tick(1_200).state.expression, Expression::Happy);

    // Release + 1.5 s.
    assert_eq!(engine.tick(2_699).state.expression, Expression::Happy);
    assert_eq!(engine.tick(2_700).state.expression, Expression::Idle);
}

#[test]
fn key_right_pokes_puzzled_for_1500_ms() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(key(100, EventSource::KeyRight, true));
    let pressed = engine.tick(100);
    assert_eq!(pressed.state.expression, Expression::Puzzled);
    assert_eq!(pressed.state.reason, ReactionReason::KeyRight);

    engine.on_event(key(200, EventSource::KeyRight, false));
    assert_eq!(engine.tick(1_599).state.expression, Expression::Puzzled);
    assert_eq!(engine.tick(1_600).state.expression, Expression::Idle);
}

#[test]
fn triple_key_right_is_angry_with_cooldown() {
    let mut engine = AffectEngine::new(0);

    for (index, at) in [100u64, 300, 500].iter().enumerate() {
        engine.on_event(key(*at, EventSource::KeyRight, true));
        let expression = engine.tick(*at).state.expression;
        if index < 2 {
            assert_eq!(expression, Expression::Puzzled);
        } else {
            assert_eq!(expression, Expression::Angry);
        }
        engine.on_event(key(*at + 50, EventSource::KeyRight, false));
    }

    assert_eq!(engine.tick(10_499).state.expression, Expression::Angry);
    assert_eq!(engine.tick(10_500).state.expression, Expression::Idle);

    // A second triple inside the 30 s cooldown falls back to Puzzled.
    for at in [11_000u64, 11_200, 11_400] {
        engine.on_event(key(at, EventSource::KeyRight, true));
        engine.tick(at);
        engine.on_event(key(at + 50, EventSource::KeyRight, false));
    }
    let cooled = engine.tick(11_400);
    assert_eq!(cooled.state.expression, Expression::Puzzled);
    assert_ne!(cooled.state.expression, Expression::Angry);
}

#[test]
fn both_keys_together_are_satisfied() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(key(100, EventSource::KeyLeft, true));
    assert_eq!(engine.tick(100).state.expression, Expression::Happy);

    // Within 300 ms: hug overrides the individual Happy.
    engine.on_event(key(250, EventSource::KeyRight, true));
    let hug = engine.tick(250);
    assert_eq!(hug.state.expression, Expression::Satisfied);
    assert_eq!(hug.state.reason, ReactionReason::BothKeys);
    assert_eq!(hug.haptic, Some(HapticPattern::Satisfied));

    engine.on_event(key(300, EventSource::KeyLeft, false));
    engine.on_event(key(320, EventSource::KeyRight, false));

    assert_eq!(
        engine.tick(2_249).state.expression,
        Expression::Satisfied
    );
    assert_eq!(engine.tick(2_250).state.expression, Expression::Idle);
}

#[test]
fn keys_outside_the_hug_window_do_not_hug() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(key(100, EventSource::KeyLeft, true));
    engine.on_event(key(600, EventSource::KeyRight, true));
    let tick = engine.tick(600);
    assert_eq!(tick.state.expression, Expression::Puzzled);
    assert_eq!(tick.state.reason, ReactionReason::KeyRight);
}

#[test]
fn power_short_long_and_double_map_to_commands() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(button(
        100,
        EventSource::ButtonPower,
        EventKind::ButtonShort,
    ));
    let short = engine.tick(100);
    assert_eq!(short.state.expression, Expression::Listening);
    assert_eq!(short.state.reason, ReactionReason::Activity);
    assert_eq!(short.command, Some(SystemCommand::StartListening));
    assert_eq!(short.haptic, Some(HapticPattern::Listening));

    engine.on_event(button(200, EventSource::ButtonPower, EventKind::ButtonLong));
    let long = engine.tick(200);
    assert_eq!(long.state.expression, Expression::Listening);
    assert!(!long.changed);
    assert_eq!(long.command, Some(SystemCommand::Hub));
    assert_eq!(long.haptic, None);

    engine.on_event(button(
        300,
        EventSource::ButtonPower,
        EventKind::ButtonDouble,
    ));
    let double = engine.tick(300);
    assert_eq!(double.state.expression, Expression::Idle);
    assert_eq!(double.command, Some(SystemCommand::StopActivity));
    assert!(double.changed);
    assert_eq!(double.haptic, None);
}

#[test]
fn key_left_happy_returns_to_listening() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(button(
        100,
        EventSource::ButtonPower,
        EventKind::ButtonShort,
    ));
    assert_eq!(engine.tick(100).state.expression, Expression::Listening);

    engine.on_event(key(200, EventSource::KeyLeft, true));
    assert_eq!(engine.tick(200).state.expression, Expression::Happy);
    engine.on_event(key(250, EventSource::KeyLeft, false));

    assert_eq!(engine.tick(1_749).state.expression, Expression::Happy);
    assert_eq!(
        engine.tick(1_750).state.expression,
        Expression::Listening
    );
}

#[test]
fn boot_button_is_debug_only() {
    let mut engine = AffectEngine::new(0);

    for (at, kind) in [
        (100u64, EventKind::ButtonShort),
        (200, EventKind::ButtonLong),
        (300, EventKind::ButtonDouble),
    ] {
        engine.on_event(button(at, EventSource::ButtonBoot, kind));
        let tick = engine.tick(at);
        assert_eq!(tick.state.expression, Expression::Idle);
        assert!(!tick.changed);
        assert_eq!(tick.command, None);
        assert_eq!(tick.haptic, None);
    }
}
