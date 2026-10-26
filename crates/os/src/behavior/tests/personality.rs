//! Personality ladder, boot hold-off and battery baseline.
//!
//! These tests pin the "human feel" rules from the H3 work package:
//!
//! - a tap never changes the face in the same millisecond;
//! - one tap is curiosity, two are puzzlement, three are annoyance, and only a
//!   frantic burst makes the fox angry;
//! - an angry fox is not re-angered during its cooldown;
//! - a finger that rests or strokes is petting, never a tap;
//! - the boot hold-off discards pointer samples completely;
//! - the battery face is a baseline that reactions still override.

use crate::behavior::affect::HapticPattern;
use crate::behavior::affect::{AffectEngine, EventKind, EventSource, ReactionReason, SensorEvent};
use crate::behavior::personality::{
    PersonaReaction, PersonalityEngine, PointerSample, PET_AFTER_MS, PET_HOLD_MS, PET_LEAN_MAX_PX,
    SCREEN_CENTER_X, SCREEN_CENTER_Y,
};
use crate::behavior::Expression;

const CENTER: (i32, i32) = (SCREEN_CENTER_X, SCREEN_CENTER_Y);

fn tap(engine: &mut PersonalityEngine, at_ms: u64) {
    let down = PointerSample {
        at_ms,
        x: CENTER.0,
        y: CENTER.1,
        pressed: true,
    };
    engine.on_pointer(down);
    let up = PointerSample {
        at_ms: at_ms + 40,
        x: CENTER.0,
        y: CENTER.1,
        pressed: false,
    };
    engine.on_pointer(up);
}

#[test]
fn a_single_tap_is_not_answered_in_the_same_frame() {
    let mut engine = PersonalityEngine::new(0);
    tap(&mut engine, 1_000);

    // The release itself must not switch the face.
    let output = engine.tick(1_040);
    assert_eq!(output.expression, None, "no reaction before the delay");
    assert_eq!(output.reaction, None);

    // The reaction appears after the first-reaction delay.
    let output = engine.tick(1_000 + 40 + 220);
    assert_eq!(output.reaction, Some(PersonaReaction::Curious));
    assert_eq!(output.expression, Some(Expression::Curious));
    assert_eq!(
        output.haptic, None,
        "a single curious tap does not buzz; a real object nudged by a finger does not vibrate"
    );
}

#[test]
fn the_tap_ladder_escalates_one_two_three() {
    let mut engine = PersonalityEngine::new(0);

    tap(&mut engine, 1_000);
    let _ = engine.tick(1_100);
    tap(&mut engine, 1_400);
    let output = engine.tick(1_400 + 40 + 380);
    assert_eq!(
        output.reaction,
        Some(PersonaReaction::Puzzled),
        "two taps inside the window are puzzlement, not two separate reactions"
    );

    tap(&mut engine, 2_400);
    let output = engine.tick(2_440 + 380);
    assert_eq!(
        output.reaction,
        Some(PersonaReaction::Annoyed),
        "three taps are annoyance, not anger"
    );
    assert_ne!(
        output.expression,
        Some(Expression::Angry),
        "three taps must never show the angry face"
    );
}

#[test]
fn only_a_frantic_burst_makes_the_fox_angry() {
    let mut engine = PersonalityEngine::new(0);
    let mut at = 5_000;
    for _ in 0..4 {
        tap(&mut engine, at);
        at += 150;
    }
    let calm = engine.tick(at + 380);
    assert_ne!(
        calm.reaction,
        Some(PersonaReaction::Angry),
        "four taps are still only annoyance"
    );

    // The fifth tap lands inside the frantic window measured from the first.
    tap(&mut engine, at + 500);
    let angry = engine.tick(at + 500 + 40 + 380);
    assert_eq!(
        angry.reaction,
        Some(PersonaReaction::Angry),
        "the fifth tap inside the frantic window is the angry one"
    );
    assert_eq!(angry.expression, Some(Expression::Angry));
    assert_eq!(
        angry.haptic,
        Some(crate::behavior::affect::HapticPattern::Angry),
        "anger is the moment that buzzes"
    );
}

#[test]
fn anger_has_a_cooldown_so_more_taps_do_not_restart_it() {
    let mut engine = PersonalityEngine::new(0);
    let mut at = 10_000;
    for _ in 0..5 {
        tap(&mut engine, at);
        at += 120;
    }
    assert_eq!(engine.tick(at + 380).reaction, Some(PersonaReaction::Angry));

    // Hammering during the angry hold does not extend or restart the reaction.
    let mut after = at;
    for _ in 0..5 {
        tap(&mut engine, after);
        after += 120;
    }
    let still_angry = engine.tick(after);
    assert_eq!(still_angry.reaction, Some(PersonaReaction::Angry));

    // After the hold expires, even a fresh burst is only annoyance while the
    // cooldown runs.
    let cooldown_sample = after + 4_000;
    let mut burst = cooldown_sample;
    for _ in 0..5 {
        tap(&mut engine, burst);
        burst += 120;
    }
    let muted = engine.tick(burst + 380);
    assert_eq!(
        muted.reaction,
        Some(PersonaReaction::Annoyed),
        "inside the cooldown a burst only annoys the fox"
    );
}

#[test]
fn the_boot_hold_off_discards_pointer_samples() {
    let mut engine = PersonalityEngine::new(0);
    engine.ignore_until(3_000);

    tap(&mut engine, 1_000);
    let output = engine.tick(2_000);
    assert_eq!(output.expression, None);
    assert_eq!(output.reaction, None);

    // Samples after the hold-off work normally.
    tap(&mut engine, 3_100);
    let output = engine.tick(3_100 + 40 + 220);
    assert_eq!(output.reaction, Some(PersonaReaction::Curious));
}

#[test]
fn a_press_in_progress_is_cancelled_by_the_boot_hold_off() {
    let mut engine = PersonalityEngine::new(0);
    engine.on_pointer(PointerSample {
        at_ms: 100,
        x: CENTER.0,
        y: CENTER.1,
        pressed: true,
    });
    engine.ignore_until(5_000);

    let output = engine.tick(1_000);
    assert!(
        output.reaction.is_none(),
        "the entrance cannot start a reaction"
    );
    assert!(output.expression.is_none());

    // The stale release is also ignored, so it cannot queue a tap.
    engine.on_pointer(PointerSample {
        at_ms: 1_100,
        x: CENTER.0,
        y: CENTER.1,
        pressed: false,
    });
    let output = engine.tick(5_500);
    assert_eq!(output.reaction, None);
}

#[test]
fn the_tap_pulse_decays() {
    let mut engine = PersonalityEngine::new(0);
    engine.on_pointer(PointerSample {
        at_ms: 1_000,
        x: CENTER.0,
        y: CENTER.1,
        pressed: true,
    });
    let pressed_frame = engine.tick(1_020);
    assert!(
        pressed_frame.pulse > 0.0,
        "the touch-down produces a visible pulse before any reaction"
    );
    engine.on_pointer(PointerSample {
        at_ms: 1_040,
        x: CENTER.0,
        y: CENTER.1,
        pressed: false,
    });
    let later = engine.tick(1_400);
    assert_eq!(later.pulse, 0.0, "the pulse decays inside 280 ms");
}

fn press(engine: &mut PersonalityEngine, at_ms: u64, x: i32, pressed: bool) {
    engine.on_pointer(PointerSample {
        at_ms,
        x,
        y: CENTER.1,
        pressed,
    });
}

#[test]
fn a_resting_finger_is_petting() {
    let mut engine = PersonalityEngine::new(0);
    press(&mut engine, 1_000, CENTER.0, true);
    assert_eq!(engine.tick(1_000 + PET_HOLD_MS - 10).reaction, None);

    let petted = engine.tick(1_000 + PET_HOLD_MS);
    assert_eq!(petted.reaction, Some(PersonaReaction::Petted));
    assert_eq!(petted.expression, Some(Expression::Satisfied));
    assert_eq!(petted.haptic, Some(HapticPattern::Satisfied));

    // The hand squashes her a little while it rests.
    let held = engine.tick(1_000 + PET_HOLD_MS + 300);
    assert!(held.pet > 0.2, "pet squash {}", held.pet);
    assert_eq!(held.haptic, None, "the purr buzzes once");

    // Long after the start she is still being petted.
    assert_eq!(engine.tick(10_000).reaction, Some(PersonaReaction::Petted));

    // Lift: contented a little longer, then back; no tap was counted.
    press(&mut engine, 10_000, CENTER.0, false);
    assert_eq!(
        engine.tick(10_000 + PET_AFTER_MS - 1).reaction,
        Some(PersonaReaction::Petted)
    );
    let after = engine.tick(10_000 + PET_AFTER_MS);
    assert_eq!(after.reaction, None);
    assert_eq!(engine.tick(10_000 + PET_AFTER_MS + 500).reaction, None);
    assert!(engine.tick(10_000 + PET_AFTER_MS + 600).pet < 0.01);
}

#[test]
fn a_stroke_is_petting_at_once_and_she_leans_with_it() {
    let mut engine = PersonalityEngine::new(0);
    press(&mut engine, 1_000, CENTER.0 - 40, true);
    // A quick stroke to the right, well inside the tap time.
    let mut at = 1_000;
    let mut x = CENTER.0 - 40;
    for _ in 0..6 {
        at += 20;
        x += 15;
        press(&mut engine, at, x, true);
    }
    let out = engine.tick(at);
    assert_eq!(out.reaction, Some(PersonaReaction::Petted));
    let mut lean = 0.0;
    let mut squash = 0.0;
    for _ in 0..10 {
        at += 20;
        let out = engine.tick(at);
        lean = out.lean_x;
        squash = out.pet;
    }
    assert!(lean > 2.0 && lean <= PET_LEAN_MAX_PX, "lean {lean}");
    assert!(
        squash > 0.35,
        "a stroke presses harder than a rest: {squash}"
    );

    // A quick release after a stroke is not a tap either.
    press(&mut engine, at, x, false);
    let later = engine.tick(at + PET_AFTER_MS + 400);
    assert_eq!(later.reaction, None, "no Curious from the stroke");
}

#[test]
fn petting_soothes_a_pending_tap_reaction() {
    let mut engine = PersonalityEngine::new(0);
    tap(&mut engine, 1_000);
    tap(&mut engine, 1_200);
    // Before the puzzled face appears, the finger comes back and stays.
    press(&mut engine, 1_300, CENTER.0, true);
    let petted = engine.tick(1_300 + PET_HOLD_MS);
    assert_eq!(petted.reaction, Some(PersonaReaction::Petted));
    press(&mut engine, 3_000, CENTER.0, false);

    // The next tap starts a fresh ladder: curious, not annoyed.
    tap(&mut engine, 6_000);
    assert_eq!(
        engine.tick(6_000 + 40 + 220).reaction,
        Some(PersonaReaction::Curious)
    );
}

// ---------------------------------------------------------------------------
// Battery baseline
// ---------------------------------------------------------------------------

fn battery(at_ms: u64, percent: u8) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Battery,
        kind: EventKind::BatteryLevel(percent),
    }
}

#[test]
fn a_low_battery_changes_the_resting_face() {
    let mut engine = AffectEngine::new(0);
    assert_eq!(engine.tick(100).state.expression, Expression::Idle);

    engine.on_event(battery(1_000, 40));
    assert_eq!(
        engine.tick(1_010).state.expression,
        Expression::Idle,
        "40% is not low yet"
    );

    engine.on_event(battery(2_000, 14));
    let tick = engine.tick(2_010);
    assert_eq!(tick.state.expression, Expression::Tired);
    assert_eq!(tick.state.reason, ReactionReason::Battery);
    assert!(tick.changed);

    engine.on_event(battery(3_000, 4));
    assert_eq!(
        engine.tick(3_010).state.expression,
        Expression::Dying,
        "4% is the dying face"
    );
}

#[test]
fn battery_thresholds_have_hysteresis() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(battery(1_000, 14));
    assert_eq!(engine.tick(1_010).state.expression, Expression::Tired);

    // 16% is above the enter threshold but below the clear threshold: the
    // gauge flickering around 15% must not flap the face.
    engine.on_event(battery(1_100, 16));
    assert_eq!(
        engine.tick(1_110).state.expression,
        Expression::Tired,
        "hysteresis holds the tired face at 16%"
    );

    engine.on_event(battery(1_200, 18));
    assert_eq!(
        engine.tick(1_210).state.expression,
        Expression::Idle,
        "18% clears the tired face"
    );

    engine.on_event(battery(1_300, 5));
    assert_eq!(engine.tick(1_310).state.expression, Expression::Dying);
    engine.on_event(battery(1_400, 6));
    assert_eq!(
        engine.tick(1_410).state.expression,
        Expression::Dying,
        "6% keeps the dying face"
    );
    engine.on_event(battery(1_500, 9));
    assert_eq!(
        engine.tick(1_510).state.expression,
        Expression::Tired,
        "9% leaves dying but is still tired"
    );
}

#[test]
fn reactions_still_play_over_the_battery_face() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(battery(1_000, 10));
    assert_eq!(engine.tick(1_010).state.expression, Expression::Tired);

    engine.on_event(SensorEvent {
        at_ms: 2_000,
        source: EventSource::Touch,
        kind: EventKind::TouchPoke,
    });
    assert_eq!(
        engine.tick(2_010).state.expression,
        Expression::Puzzled,
        "a poke is answered even on low battery"
    );

    // After the reaction expires the tired face returns, not Idle.
    let after = engine.tick(2_000 + 1_500 + 10);
    assert_eq!(after.state.expression, Expression::Tired);
}

#[test]
fn the_battery_face_does_not_override_activity() {
    let mut engine = AffectEngine::new(0);
    engine.on_event(battery(1_000, 12));
    assert_eq!(engine.tick(1_010).state.expression, Expression::Tired);

    engine.on_event(SensorEvent {
        at_ms: 2_000,
        source: EventSource::System,
        kind: EventKind::Activity(Expression::Listening),
    });
    assert_eq!(
        engine.tick(2_010).state.expression,
        Expression::Listening,
        "the assistant state wins over the low-battery resting face"
    );
}
