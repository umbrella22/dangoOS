//! H0 ambient timelines: relative LDR deltas, gradual dawn, night sleep, PIR
//! debounce, sustained presence, and absence.

use crate::behavior::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent,
    DAWN_CURIOUS_MS, LIGHT_BRIGHT_MS, LIGHT_COVER_MAX_MS, LIGHT_DARK_MS, LIGHT_FAST_WINDOW_MS,
    MIN_DWELL_MS, PIR_CURIOUS_MS, PIR_SATISFIED_MS, PIR_VISIT_GAP_MS,
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
fn covering_the_sensor_makes_her_peer_until_it_is_uncovered() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 30));
    assert_eq!(engine.tick(0).state.expression, Expression::Idle);

    // A finger over the LDR: 30 -> 4 well inside the window.
    engine.on_event(light(100, 4));
    let dark = engine.tick(100);
    assert_eq!(dark.state.expression, Expression::Peering);
    assert_eq!(dark.state.reason, ReactionReason::Light);
    assert_eq!(dark.haptic, None, "peering into the dark is silent");

    // Still covered: the wide eyes stay past the plain hold time.
    let mut at = 100;
    while at < 100 + 2 * LIGHT_DARK_MS {
        at += 40;
        engine.on_event(light(at, 4));
        assert_eq!(engine.tick(at).state.expression, Expression::Peering);
    }

    // The finger lifts: the light floods back and she is dazzled.
    at += 40;
    engine.on_event(light(at, 28));
    let flash = engine.tick(at);
    assert_eq!(flash.state.expression, Expression::Dazzled);
    assert_eq!(flash.state.reason, ReactionReason::Light);
    assert_eq!(flash.haptic, Some(HapticPattern::Surprised));

    // Later samples at the same level do not restart or re-buzz it.
    engine.on_event(light(at + 40, 30));
    let next = engine.tick(at + 40);
    assert_eq!(next.state.expression, Expression::Dazzled);
    assert_eq!(next.haptic, None);

    let end = at + LIGHT_BRIGHT_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Dazzled);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);
}

#[test]
fn a_long_cover_lets_her_eyes_adjust() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(light(0, 30));
    engine.on_event(light(100, 4));
    assert_eq!(engine.tick(100).state.expression, Expression::Peering);

    let mut at = 100;
    while at < 100 + LIGHT_COVER_MAX_MS + 1_000 {
        at += 40;
        engine.on_event(light(at, 4));
        engine.tick(at);
    }
    assert_eq!(engine.tick(at).state.expression, Expression::Idle);
}

#[test]
fn sudden_bright_light_dazzles_for_the_hold_time() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 30));
    engine.on_event(light(100, 80));
    let bright = engine.tick(100);
    assert_eq!(bright.state.expression, Expression::Dazzled);
    assert_eq!(bright.state.reason, ReactionReason::Light);
    assert_eq!(bright.haptic, Some(HapticPattern::Surprised));

    let end = 100 + LIGHT_BRIGHT_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Dazzled);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);
}

#[test]
fn small_light_changes_are_ignored() {
    let mut engine = AffectEngine::new(0);

    // A shadow passing by (30 -> 22) and a lamp (60 -> 75) are no reaction.
    engine.on_event(light(0, 30));
    engine.on_event(light(100, 22));
    assert_eq!(engine.tick(100).state.expression, Expression::Idle);
    engine.on_event(light(2_000, 60));
    engine.on_event(light(2_100, 75));
    assert_eq!(engine.tick(2_100).state.expression, Expression::Idle);
}

#[test]
fn bright_delta_is_measured_across_the_window() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(light(0, 30));
    engine.on_event(light(100, 40));
    assert_eq!(engine.tick(100).state.expression, Expression::Idle);

    // 30 -> 60 across 300 ms: the reference is the oldest sample inside the
    // window, not only the previous sample.
    engine.on_event(light(300, 60));
    let bright = engine.tick(300);
    assert_eq!(bright.state.expression, Expression::Dazzled);
    assert_eq!(bright.state.reason, ReactionReason::Light);
    assert!(LIGHT_FAST_WINDOW_MS >= 300);
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
        assert_ne!(expression, Expression::Dazzled);
        assert_ne!(expression, Expression::Peering);
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
    assert_eq!(woken.state.expression, Expression::Dazzled);

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
fn a_pulsing_pir_greets_once_per_visit() {
    let mut engine = AffectEngine::new(0);
    let mut curious = 0;
    let mut last = Expression::Idle;
    let mut satisfied = false;

    // The module pulses: 2 s high, 3 s low, for a minute.
    let mut at = 0u64;
    while at < 60_000 {
        let high = at % 5_000 < 2_000;
        if at % 1_000 == 0 {
            engine.on_event(presence(at, high));
        }
        let expression = engine.tick(at).state.expression;
        if expression == Expression::Curious && last != Expression::Curious {
            curious += 1;
        }
        satisfied |= expression == Expression::Satisfied;
        last = expression;
        at += 50;
    }
    assert_eq!(curious, 1, "one greeting for the whole visit");
    assert!(satisfied, "a long pulsing visit still counts as sustained");

    // A long quiet gap ends the visit; the next arrival greets again.
    engine.on_event(presence(at, false));
    at += PIR_VISIT_GAP_MS + 1_000;
    engine.tick(at);
    engine.on_event(presence(at, true));
    at += 200;
    assert_eq!(engine.tick(at).state.expression, Expression::Curious);
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
    engine.on_event(light(100, 30));
    assert_eq!(engine.tick(300).state.expression, Expression::Curious);

    // A light jump right after does not replace it.
    engine.on_event(light(400, 80));
    let held = engine.tick(400);
    assert_eq!(held.state.expression, Expression::Curious);
    assert_eq!(held.state.reason, ReactionReason::Presence);

    // After the minimum stay a new light jump does.
    let later = 300 + MIN_DWELL_MS;
    engine.on_event(light(later - 100, 30));
    engine.on_event(light(later, 80));
    let replaced = engine.tick(later);
    assert_eq!(replaced.state.expression, Expression::Dazzled);
    assert_eq!(replaced.state.reason, ReactionReason::Light);
}
