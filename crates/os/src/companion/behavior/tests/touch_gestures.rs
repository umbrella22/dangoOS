//! H0 touch timelines: petting, poking, rapid pokes, Angry precedence, and
//! haptic rate limiting.

use crate::companion::behavior::Expression;
use {
    crate::companion::behavior::affect::AffectEngine,
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::EventSource,
    crate::companion::behavior::affect::HapticPattern,
    crate::companion::behavior::affect::ReactionReason,
    crate::companion::behavior::affect::SensorEvent,
};

fn touch(at_ms: u64, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Touch,
        kind,
    }
}

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

fn triple_key_r(engine: &mut AffectEngine) {
    // Three KEY_R presses inside the 1.5 s triple window.
    for at in [0u64, 100, 200] {
        engine.on_event(key(at, EventSource::KeyRight, true));
        engine.tick(at);
        engine.on_event(key(at + 50, EventSource::KeyRight, false));
    }
}

#[test]
fn petting_is_happy_while_petting_and_1500_ms_after() {
    let mut engine = AffectEngine::new(0);

    for at in [100u64, 200, 300] {
        engine.on_event(touch(at, EventKind::TouchPet));
        let tick = engine.tick(at);
        assert_eq!(tick.state.expression, Expression::Happy);
        assert_eq!(tick.state.reason, ReactionReason::TouchPet);
    }

    // Last pet at 300, so Happy lasts until 1_800 (exclusive).
    assert_eq!(engine.tick(1_000).state.expression, Expression::Happy);
    assert_eq!(engine.tick(1_799).state.expression, Expression::Happy);
    let back = engine.tick(1_800);
    assert_eq!(back.state.expression, Expression::Idle);
    assert!(back.changed);
}

#[test]
fn poke_is_puzzled_for_1500_ms() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(touch(100, EventKind::TouchPoke));

    let poke = engine.tick(100);
    assert_eq!(poke.state.expression, Expression::Puzzled);
    assert_eq!(poke.state.reason, ReactionReason::TouchPoke);
    assert_eq!(poke.haptic, Some(HapticPattern::Puzzled));

    assert_eq!(engine.tick(1_599).state.expression, Expression::Puzzled);
    assert_eq!(engine.tick(1_600).state.expression, Expression::Idle);
}

#[test]
fn petting_and_poking_do_not_stack() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(touch(100, EventKind::TouchPet));
    assert_eq!(engine.tick(100).state.expression, Expression::Happy);

    engine.on_event(touch(200, EventKind::TouchPoke));
    let poke = engine.tick(200);
    assert_eq!(poke.state.expression, Expression::Puzzled);
    assert!(poke.changed);

    // One reaction at a time: the poke replaced petting, and the original
    // petting deadline at 1_600 does not matter.
    assert_eq!(engine.tick(400).state.expression, Expression::Puzzled);
}

#[test]
fn poke_while_angry_does_not_override_angry() {
    let mut engine = AffectEngine::new(0);
    triple_key_r(&mut engine);
    assert_eq!(engine.tick(200).state.expression, Expression::Angry);

    engine.on_event(touch(300, EventKind::TouchPoke));
    let poke = engine.tick(300);
    assert_eq!(poke.state.expression, Expression::Angry);
    assert_eq!(poke.state.reason, ReactionReason::KeyRight);

    engine.on_event(touch(400, EventKind::TouchPet));
    assert_eq!(engine.tick(400).state.expression, Expression::Angry);

    assert_eq!(engine.tick(10_199).state.expression, Expression::Angry);
    assert_eq!(engine.tick(10_200).state.expression, Expression::Idle);
}

#[test]
fn three_pokes_within_1500_ms_trigger_angry() {
    let mut engine = AffectEngine::new(0);

    for (index, at) in [100u64, 400, 700].iter().enumerate() {
        engine.on_event(touch(*at, EventKind::TouchPoke));
        let expression = engine.tick(*at).state.expression;
        if index < 2 {
            assert_eq!(expression, Expression::Puzzled);
        } else {
            assert_eq!(expression, Expression::Angry);
        }
    }

    assert_eq!(engine.tick(10_699).state.expression, Expression::Angry);
    assert_eq!(engine.tick(10_700).state.expression, Expression::Idle);
}

#[test]
fn petting_haptic_is_rate_limited() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(touch(0, EventKind::TouchPet));
    assert_eq!(engine.tick(0).haptic, Some(HapticPattern::Happy));

    engine.on_event(touch(100, EventKind::TouchPoke));
    let limited = engine.tick(100);
    assert_eq!(limited.state.expression, Expression::Puzzled);
    assert_eq!(limited.haptic, None);

    let deferred = engine.tick(150);
    assert_eq!(deferred.haptic, Some(HapticPattern::Puzzled));
}
