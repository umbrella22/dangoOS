//! H0 event-contract tests: defaults, priority resolution, one reaction at a
//! time, and return to the prior activity.

use crate::behavior::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent, SystemCommand,
};
use crate::behavior::Expression;

fn event(at_ms: u64, source: EventSource, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source,
        kind,
    }
}

fn accel(at_ms: u64, ax_mg: i16, ay_mg: i16, az_mg: i16) -> SensorEvent {
    event(
        at_ms,
        EventSource::Imu,
        EventKind::Accel {
            ax_mg,
            ay_mg,
            az_mg,
        },
    )
}

fn gyro(at_ms: u64, gx_dps: i16, gy_dps: i16, gz_dps: i16) -> SensorEvent {
    event(
        at_ms,
        EventSource::Imu,
        EventKind::Gyro {
            gx_dps,
            gy_dps,
            gz_dps,
        },
    )
}

fn key(at_ms: u64, source: EventSource, down: bool) -> SensorEvent {
    event(
        at_ms,
        source,
        if down {
            EventKind::KeyDown
        } else {
            EventKind::KeyUp
        },
    )
}

fn touch(at_ms: u64, kind: EventKind) -> SensorEvent {
    event(at_ms, EventSource::Touch, kind)
}

fn magnetic(at_ms: u64, kind: EventKind) -> SensorEvent {
    event(at_ms, EventSource::Bmm150, kind)
}

fn light(at_ms: u64, level: u8) -> SensorEvent {
    event(at_ms, EventSource::Light, EventKind::LightLevel(level))
}

fn button(at_ms: u64, kind: EventKind) -> SensorEvent {
    event(at_ms, EventSource::ButtonPower, kind)
}

fn activity(at_ms: u64, expression: Expression) -> SensorEvent {
    event(at_ms, EventSource::System, EventKind::Activity(expression))
}

/// Three KEY_R presses inside the 1.5 s triple window.
fn triple_key_r(engine: &mut AffectEngine) {
    for at in [100u64, 300, 500] {
        engine.on_event(key(at, EventSource::KeyRight, true));
        engine.tick(at);
        engine.on_event(key(at + 50, EventSource::KeyRight, false));
    }
}

#[test]
fn default_is_idle() {
    let mut engine = AffectEngine::new(0);
    let first = engine.tick(0);
    assert_eq!(first.state.expression, Expression::Idle);
    assert_eq!(first.state.reason, ReactionReason::Idle);
    assert_eq!(first.state.intensity, 0);
    assert!(!first.changed);
    assert_eq!(first.haptic, None);
    assert_eq!(first.command, None);

    let later = engine.tick(10_000);
    assert_eq!(later.state.expression, Expression::Idle);
    assert!(!later.changed);
    assert_eq!(later.haptic, None);
    assert_eq!(later.command, None);
}

#[test]
fn panic_overrides_angry_and_lower_reactions() {
    let mut engine = AffectEngine::new(0);
    triple_key_r(&mut engine);

    let angry = engine.tick(500);
    assert_eq!(angry.state.expression, Expression::Angry);
    assert_eq!(angry.state.reason, ReactionReason::KeyRight);

    // A single gyro sample above 200 dps is a large shake.
    engine.on_event(gyro(600, 0, 0, 250));
    let panic = engine.tick(600);
    assert_eq!(panic.state.expression, Expression::Panic);
    assert_eq!(panic.state.reason, ReactionReason::ShakeLarge);
    // Angry's haptic was emitted at 500, so Panic is deferred 150 ms.
    assert_eq!(panic.haptic, None);
    let deferred = engine.tick(650);
    assert_eq!(deferred.state.expression, Expression::Panic);
    assert_eq!(deferred.haptic, Some(HapticPattern::Panic));

    // Physical reactions cannot override Panic.
    engine.on_event(touch(700, EventKind::TouchPoke));
    let poke = engine.tick(700);
    assert_eq!(poke.state.expression, Expression::Panic);

    engine.on_event(magnetic(800, EventKind::MagneticContact));
    let contact = engine.tick(800);
    assert_eq!(contact.state.expression, Expression::Panic);
}

#[test]
fn short_reaction_overlays_and_returns_to_activity() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(button(100, EventKind::ButtonShort));
    let listening = engine.tick(100);
    assert_eq!(listening.state.expression, Expression::Listening);
    assert_eq!(listening.state.reason, ReactionReason::Activity);
    assert_eq!(listening.command, Some(SystemCommand::StartListening));
    assert_eq!(listening.haptic, Some(HapticPattern::Listening));

    engine.on_event(magnetic(200, EventKind::MagneticApproach));
    let curious = engine.tick(200);
    assert_eq!(curious.state.expression, Expression::Curious);
    assert_eq!(curious.state.reason, ReactionReason::Magnetic);
    assert!(curious.changed);

    let still_curious = engine.tick(2_199);
    assert_eq!(still_curious.state.expression, Expression::Curious);

    let back = engine.tick(2_200);
    assert_eq!(back.state.expression, Expression::Listening);
    assert_eq!(back.state.reason, ReactionReason::Activity);
    assert!(back.changed);

    // The activity itself survives the overlay.
    let later = engine.tick(9_000);
    assert_eq!(later.state.expression, Expression::Listening);
    assert!(!later.changed);
}

#[test]
fn only_one_reaction_renders_at_a_time() {
    let mut engine = AffectEngine::new(0);
    triple_key_r(&mut engine);
    assert_eq!(engine.tick(500).state.expression, Expression::Angry);

    // Ambient and physical events are ignored while Angry holds.
    engine.on_event(touch(600, EventKind::TouchPoke));
    assert_eq!(engine.tick(600).state.expression, Expression::Angry);
    engine.on_event(touch(700, EventKind::TouchPet));
    assert_eq!(engine.tick(700).state.expression, Expression::Angry);
    engine.on_event(light(750, 80));
    engine.on_event(light(850, 40));
    assert_eq!(engine.tick(850).state.expression, Expression::Angry);

    // Angry started at 500 and holds for 10 s.
    assert_eq!(engine.tick(10_499).state.expression, Expression::Angry);
    let done = engine.tick(10_500);
    assert_eq!(done.state.expression, Expression::Idle);
    assert!(done.changed);
}

#[test]
fn activity_input_selects_listening_thinking_and_speaking() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(activity(100, Expression::Listening));
    let listening = engine.tick(100);
    assert_eq!(listening.state.expression, Expression::Listening);
    assert_eq!(listening.state.reason, ReactionReason::Activity);

    engine.on_event(activity(200, Expression::Thinking));
    let thinking = engine.tick(200);
    assert_eq!(thinking.state.expression, Expression::Thinking);
    assert_eq!(thinking.state.reason, ReactionReason::Activity);
    assert!(thinking.changed);

    engine.on_event(activity(300, Expression::Speaking));
    let speaking = engine.tick(300);
    assert_eq!(speaking.state.expression, Expression::Speaking);
    assert_eq!(speaking.state.reason, ReactionReason::Activity);

    engine.on_event(activity(400, Expression::Idle));
    let idle = engine.tick(400);
    assert_eq!(idle.state.expression, Expression::Idle);
    assert_eq!(idle.state.reason, ReactionReason::Idle);
    assert!(idle.changed);

    // Non-Activity expressions are ignored.
    engine.on_event(activity(500, Expression::Happy));
    let ignored = engine.tick(500);
    assert_eq!(ignored.state.expression, Expression::Idle);
    assert!(!ignored.changed);

    engine.on_event(activity(600, Expression::Panic));
    assert_eq!(engine.tick(600).state.expression, Expression::Idle);
}

#[test]
fn short_physical_and_ambient_reactions_overlay_activity() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(activity(100, Expression::Listening));
    assert_eq!(engine.tick(100).state.expression, Expression::Listening);

    // Physical overlay.
    engine.on_event(touch(200, EventKind::TouchPoke));
    assert_eq!(engine.tick(200).state.expression, Expression::Puzzled);
    assert_eq!(engine.tick(1_699).state.expression, Expression::Puzzled);
    assert_eq!(engine.tick(1_700).state.expression, Expression::Listening);

    // Ambient overlay.
    engine.on_event(activity(2_000, Expression::Thinking));
    assert_eq!(engine.tick(2_000).state.expression, Expression::Thinking);
    engine.on_event(light(2_100, 80));
    engine.on_event(light(2_200, 50));
    assert_eq!(engine.tick(2_200).state.expression, Expression::Confused);
    assert_eq!(engine.tick(4_199).state.expression, Expression::Confused);
    assert_eq!(engine.tick(4_200).state.expression, Expression::Thinking);
}

#[test]
fn panic_interrupts_activity_and_it_resumes_after_the_sequence() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(activity(100, Expression::Thinking));
    assert_eq!(engine.tick(100).state.expression, Expression::Thinking);

    engine.on_event(gyro(200, 0, 0, 300));
    assert_eq!(engine.tick(200).state.expression, Expression::Panic);

    // The full panic sequence masks Activity: Panic hold, still wait,
    // post-panic Confused, then the Activity resumes.
    let after_panic = engine.tick(1_700);
    assert_eq!(after_panic.state.expression, Expression::Idle);
    assert_eq!(after_panic.state.reason, ReactionReason::Idle);

    assert_eq!(engine.tick(6_699).state.expression, Expression::Idle);
    assert_eq!(engine.tick(6_700).state.expression, Expression::Confused);
    assert_eq!(engine.tick(9_699).state.expression, Expression::Confused);
    let resumed = engine.tick(9_700);
    assert_eq!(resumed.state.expression, Expression::Thinking);
    assert_eq!(resumed.state.reason, ReactionReason::Activity);
    assert!(resumed.changed);
}

#[test]
fn error_interrupts_panic_angry_and_activity_until_idle() {
    let mut engine = AffectEngine::new(0);

    // Error interrupts an active Activity and an active Panic.
    engine.on_event(activity(100, Expression::Speaking));
    assert_eq!(engine.tick(100).state.expression, Expression::Speaking);

    engine.on_event(gyro(200, 0, 0, 300));
    assert_eq!(engine.tick(200).state.expression, Expression::Panic);

    engine.on_event(activity(300, Expression::Error));
    let error = engine.tick(300);
    assert_eq!(error.state.expression, Expression::Error);
    assert_eq!(error.state.reason, ReactionReason::Error);
    assert!(error.changed);

    // Error clears Panic and its follow-up; no post-panic Confused appears.
    assert_eq!(engine.tick(10_000).state.expression, Expression::Error);

    // Other Activity values and physical/ambient events do not clear Error.
    engine.on_event(activity(10_100, Expression::Listening));
    assert_eq!(engine.tick(10_100).state.expression, Expression::Error);
    engine.on_event(touch(10_200, EventKind::TouchPoke));
    assert_eq!(engine.tick(10_200).state.expression, Expression::Error);
    engine.on_event(light(10_300, 80));
    engine.on_event(light(10_400, 50));
    assert_eq!(engine.tick(10_400).state.expression, Expression::Error);

    // Only Activity(Idle) clears Error.
    engine.on_event(activity(10_500, Expression::Idle));
    let cleared = engine.tick(10_500);
    assert_eq!(cleared.state.expression, Expression::Idle);
    assert_eq!(cleared.state.reason, ReactionReason::Idle);
    assert!(cleared.changed);
    assert_eq!(engine.tick(10_600).state.expression, Expression::Idle);

    // Error also interrupts Angry and holds until Activity(Idle).
    for at in [11_000u64, 11_200, 11_400] {
        engine.on_event(key(at, EventSource::KeyRight, true));
        engine.tick(at);
        engine.on_event(key(at + 50, EventSource::KeyRight, false));
    }
    assert_eq!(engine.tick(11_400).state.expression, Expression::Angry);

    engine.on_event(activity(11_500, Expression::Error));
    let angry_error = engine.tick(11_500);
    assert_eq!(angry_error.state.expression, Expression::Error);
    assert_eq!(angry_error.state.reason, ReactionReason::Error);

    assert_eq!(engine.tick(30_000).state.expression, Expression::Error);
    engine.on_event(activity(30_100, Expression::Idle));
    assert_eq!(engine.tick(30_100).state.expression, Expression::Idle);
}

#[test]
fn physical_reaction_blocks_ambient_reaction() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(touch(100, EventKind::TouchPoke));
    let puzzled = engine.tick(100);
    assert_eq!(puzzled.state.expression, Expression::Puzzled);

    // Sudden dark while Puzzled is active: ambient must not replace physical.
    engine.on_event(light(200, 80));
    engine.on_event(light(300, 50));
    assert_eq!(engine.tick(300).state.expression, Expression::Puzzled);

    // Puzzled ends at 1600; the blocked dark reaction never appears.
    assert_eq!(engine.tick(1_600).state.expression, Expression::Idle);

    // With no physical reaction, the same light delta does react.
    engine.on_event(light(2_000, 80));
    engine.on_event(light(2_100, 50));
    let confused = engine.tick(2_100);
    assert_eq!(confused.state.expression, Expression::Confused);
    assert_eq!(confused.state.reason, ReactionReason::Light);
    assert_eq!(confused.haptic, Some(HapticPattern::Confused));
}

#[test]
fn angry_holds_over_activity_then_returns_to_it() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(button(100, EventKind::ButtonShort));
    assert_eq!(engine.tick(100).state.expression, Expression::Listening);

    // Angry interrupts the activity for its 10 s hold.
    for at in [200u64, 400, 600] {
        engine.on_event(key(at, EventSource::KeyRight, true));
        engine.tick(at);
        engine.on_event(key(at + 50, EventSource::KeyRight, false));
    }
    let angry = engine.tick(600);
    assert_eq!(angry.state.expression, Expression::Angry);
    assert_eq!(angry.state.reason, ReactionReason::KeyRight);

    assert_eq!(engine.tick(10_599).state.expression, Expression::Angry);
    let resumed = engine.tick(10_600);
    assert_eq!(resumed.state.expression, Expression::Listening);
    assert_eq!(resumed.state.reason, ReactionReason::Activity);
}

#[test]
fn haptic_rate_limit_defers_the_second_pattern() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(touch(0, EventKind::TouchPet));
    let first = engine.tick(0);
    assert_eq!(first.haptic, Some(HapticPattern::Happy));

    engine.on_event(touch(100, EventKind::TouchPoke));
    let limited = engine.tick(100);
    assert_eq!(limited.state.expression, Expression::Puzzled);
    assert_eq!(limited.haptic, None);

    let released = engine.tick(150);
    assert_eq!(released.haptic, Some(HapticPattern::Puzzled));
    assert_eq!(released.state.expression, Expression::Puzzled);
}

#[test]
fn panic_replaces_a_pending_lower_haptic() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(button(0, EventKind::ButtonShort));
    assert_eq!(engine.tick(0).haptic, Some(HapticPattern::Listening));

    // Happy is queued but rate-limited.
    engine.on_event(touch(100, EventKind::TouchPet));
    let limited = engine.tick(100);
    assert_eq!(limited.state.expression, Expression::Happy);
    assert_eq!(limited.haptic, None);

    // Panic replaces the pending Happy pattern.
    engine.on_event(gyro(110, 0, 0, 300));
    let panic = engine.tick(150);
    assert_eq!(panic.state.expression, Expression::Panic);
    assert_eq!(panic.haptic, Some(HapticPattern::Panic));
}

#[test]
fn tick_is_idempotent_at_the_same_timestamp() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(accel(100, 0, 0, 1_500));

    let first = engine.tick(100);
    let second = engine.tick(100);
    assert_eq!(first.state, second.state);
    assert!(first.changed);
    assert!(!second.changed);
    assert_eq!(second.haptic, None);
    assert_eq!(second.command, None);
}
