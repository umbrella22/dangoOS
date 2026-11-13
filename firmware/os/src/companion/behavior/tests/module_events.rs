//! H0 module timelines: Interaction insertion/removal sequences on either
//! slot, camera reactions, duplicate suppression, and overlay return.

use crate::companion::behavior::Expression;
use {
    crate::companion::behavior::affect::AffectEngine,
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::EventSource,
    crate::companion::behavior::affect::HapticPattern,
    crate::companion::behavior::affect::ModuleKind,
    crate::companion::behavior::affect::ReactionReason,
    crate::companion::behavior::affect::SensorEvent,
    crate::companion::behavior::affect::CAMERA_INSERT_CURIOUS_MS,
    crate::companion::behavior::affect::CAMERA_REMOVE_CONFUSED_MS,
    crate::companion::behavior::affect::INTERACTION_INSERT_CURIOUS_MS,
    crate::companion::behavior::affect::INTERACTION_INSERT_HAPPY_MS,
    crate::companion::behavior::affect::INTERACTION_INSERT_SURPRISED_MS,
    crate::companion::behavior::affect::INTERACTION_REMOVE_CONFUSED_MS,
    crate::companion::behavior::affect::INTERACTION_REMOVE_SURPRISED_MS,
};

/// Stage boundaries of the Interaction insert sequence started at `at`:
/// Surprised -> Curious -> Happy -> back.
fn insert_stages(at: u64) -> (u64, u64, u64) {
    let curious = at + INTERACTION_INSERT_SURPRISED_MS;
    let happy = curious + INTERACTION_INSERT_CURIOUS_MS;
    (curious, happy, happy + INTERACTION_INSERT_HAPPY_MS)
}

/// Stage boundaries of the Interaction remove sequence started at `at`:
/// Surprised -> Confused -> back.
fn remove_stages(at: u64) -> (u64, u64) {
    let confused = at + INTERACTION_REMOVE_SURPRISED_MS;
    (confused, confused + INTERACTION_REMOVE_CONFUSED_MS)
}

fn module(at_ms: u64, source: EventSource, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source,
        kind,
    }
}

/// The usual layout: Interaction on the right, camera on the left.
fn usual_kind(source: EventSource) -> ModuleKind {
    match source {
        EventSource::ModuleLeft => ModuleKind::Camera,
        _ => ModuleKind::Interaction,
    }
}

fn inserted(at_ms: u64, source: EventSource) -> SensorEvent {
    module(at_ms, source, EventKind::ModuleInserted(usual_kind(source)))
}

fn removed(at_ms: u64, source: EventSource) -> SensorEvent {
    module(at_ms, source, EventKind::ModuleRemoved(usual_kind(source)))
}

fn button(at_ms: u64, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::ButtonPower,
        kind,
    }
}

#[test]
fn right_slot_insert_plays_the_three_stage_sequence() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(inserted(100, EventSource::ModuleRight));
    let surprised = engine.tick(100);
    assert_eq!(surprised.state.expression, Expression::Surprised);
    assert_eq!(surprised.state.reason, ReactionReason::ModuleInsert);
    assert_eq!(surprised.haptic, Some(HapticPattern::ModuleInsert));

    let (curious, happy, done) = insert_stages(100);
    assert_eq!(
        engine.tick(curious - 1).state.expression,
        Expression::Surprised
    );
    assert_eq!(engine.tick(curious).state.expression, Expression::Curious);
    assert_eq!(engine.tick(happy - 1).state.expression, Expression::Curious);
    assert_eq!(engine.tick(happy).state.expression, Expression::Happy);
    assert_eq!(engine.tick(done - 1).state.expression, Expression::Happy);
    let back = engine.tick(done);
    assert_eq!(back.state.expression, Expression::Idle);
    assert!(back.changed);

    // Only the insertion transition produced a haptic.
    assert_eq!(engine.tick(done + 700).haptic, None);
}

#[test]
fn right_slot_remove_plays_surprised_then_confused() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(inserted(100, EventSource::ModuleRight));
    assert_eq!(
        engine.tick(insert_stages(100).2).state.expression,
        Expression::Idle
    );

    engine.on_event(removed(10_000, EventSource::ModuleRight));
    let surprised = engine.tick(10_000);
    assert_eq!(surprised.state.expression, Expression::Surprised);
    assert_eq!(surprised.state.reason, ReactionReason::ModuleRemove);
    assert_eq!(surprised.haptic, Some(HapticPattern::ModuleRemove));

    let (confused, done) = remove_stages(10_000);
    assert_eq!(
        engine.tick(confused - 1).state.expression,
        Expression::Surprised
    );
    assert_eq!(engine.tick(confused).state.expression, Expression::Confused);
    assert_eq!(engine.tick(done - 1).state.expression, Expression::Confused);
    assert_eq!(engine.tick(done).state.expression, Expression::Idle);
}

#[test]
fn removal_without_presence_is_ignored() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(removed(100, EventSource::ModuleRight));
    let tick = engine.tick(100);
    assert_eq!(tick.state.expression, Expression::Idle);
    assert_eq!(tick.haptic, None);

    engine.on_event(removed(200, EventSource::ModuleLeft));
    let tick = engine.tick(200);
    assert_eq!(tick.state.expression, Expression::Idle);
    assert_eq!(tick.haptic, None);
}

#[test]
fn left_camera_insert_and_remove_use_short_reactions() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(inserted(100, EventSource::ModuleLeft));
    let insert = engine.tick(100);
    assert_eq!(insert.state.expression, Expression::Curious);
    assert_eq!(insert.state.reason, ReactionReason::ModuleInsert);
    assert_eq!(insert.haptic, Some(HapticPattern::ModuleInsert));

    let end = 100 + CAMERA_INSERT_CURIOUS_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Curious);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);

    engine.on_event(removed(5_000, EventSource::ModuleLeft));
    let remove = engine.tick(5_000);
    assert_eq!(remove.state.expression, Expression::Confused);
    assert_eq!(remove.state.reason, ReactionReason::ModuleRemove);
    assert_eq!(remove.haptic, Some(HapticPattern::ModuleRemove));

    let end = 5_000 + CAMERA_REMOVE_CONFUSED_MS;
    assert_eq!(engine.tick(end - 1).state.expression, Expression::Confused);
    assert_eq!(engine.tick(end).state.expression, Expression::Idle);
}

#[test]
fn insertion_overlays_activity_and_returns_to_it() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(button(0, EventKind::ButtonShort));
    assert_eq!(engine.tick(0).state.expression, Expression::Listening);

    engine.on_event(inserted(100, EventSource::ModuleRight));
    assert_eq!(engine.tick(100).state.expression, Expression::Surprised);

    let done = insert_stages(100).2;
    assert_eq!(engine.tick(done - 1).state.expression, Expression::Happy);
    let back_to_activity = engine.tick(done);
    assert_eq!(back_to_activity.state.expression, Expression::Listening);
    assert_eq!(back_to_activity.state.reason, ReactionReason::Activity);

    // Removal also overlays and returns.
    engine.on_event(removed(10_000, EventSource::ModuleRight));
    assert_eq!(engine.tick(10_000).state.expression, Expression::Surprised);
    assert_eq!(
        engine.tick(remove_stages(10_000).1).state.expression,
        Expression::Listening
    );
}

#[test]
fn duplicate_insert_does_not_restart_the_sequence() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(inserted(100, EventSource::ModuleRight));
    assert_eq!(engine.tick(100).haptic, Some(HapticPattern::ModuleInsert));

    engine.on_event(inserted(300, EventSource::ModuleRight));
    let duplicate = engine.tick(300);
    assert_eq!(duplicate.state.expression, Expression::Surprised);
    assert_eq!(duplicate.haptic, None);

    // The original Surprised stage still ends on time.
    let (curious, _, _) = insert_stages(100);
    assert_eq!(engine.tick(curious).state.expression, Expression::Curious);
}

#[test]
fn insertion_while_angry_is_ignored() {
    let mut engine = AffectEngine::new(0);

    for at in [0u64, 100, 200] {
        engine.on_event(SensorEvent {
            at_ms: at,
            source: EventSource::KeyRight,
            kind: EventKind::KeyDown,
        });
        engine.tick(at);
        engine.on_event(SensorEvent {
            at_ms: at + 50,
            source: EventSource::KeyRight,
            kind: EventKind::KeyUp,
        });
    }
    assert_eq!(engine.tick(200).state.expression, Expression::Angry);

    engine.on_event(inserted(300, EventSource::ModuleRight));
    let during_angry = engine.tick(300);
    assert_eq!(during_angry.state.expression, Expression::Angry);
    assert_eq!(during_angry.haptic, None);

    assert_eq!(engine.tick(10_200).state.expression, Expression::Idle);
}

#[test]
fn interaction_in_the_left_slot_plays_the_interaction_sequence() {
    let mut engine = AffectEngine::new(0);

    let kind = EventKind::ModuleInserted(ModuleKind::Interaction);
    engine.on_event(module(100, EventSource::ModuleLeft, kind));
    let (curious, happy, done) = insert_stages(100);
    assert_eq!(engine.tick(100).state.expression, Expression::Surprised);
    assert_eq!(engine.tick(curious).state.expression, Expression::Curious);
    assert_eq!(engine.tick(happy).state.expression, Expression::Happy);
    assert_eq!(engine.tick(done).state.expression, Expression::Idle);

    let kind = EventKind::ModuleRemoved(ModuleKind::Interaction);
    engine.on_event(module(10_000, EventSource::ModuleLeft, kind));
    let (confused, done) = remove_stages(10_000);
    assert_eq!(engine.tick(10_000).state.expression, Expression::Surprised);
    assert_eq!(engine.tick(confused).state.expression, Expression::Confused);
    assert_eq!(engine.tick(done).state.expression, Expression::Idle);
}

#[test]
fn each_slot_tracks_its_own_presence() {
    let mut engine = AffectEngine::new(0);

    let insert = EventKind::ModuleInserted(ModuleKind::Interaction);
    engine.on_event(module(100, EventSource::ModuleRight, insert));
    assert_eq!(
        engine.tick(insert_stages(100).2).state.expression,
        Expression::Idle
    );

    // A second board on the other side is a new insertion, not a duplicate.
    engine.on_event(module(10_000, EventSource::ModuleLeft, insert));
    let second = engine.tick(10_000);
    assert_eq!(second.state.expression, Expression::Surprised);
    assert_eq!(second.haptic, Some(HapticPattern::ModuleInsert));
}
