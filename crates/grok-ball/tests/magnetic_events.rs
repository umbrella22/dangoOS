//! H0 BMM150 timelines: approach, edge-triggered contact/release, orbit,
//! confidence gating, and duplicate suppression.

use grok_ball::affect::{
    AffectEngine, EventKind, EventSource, HapticPattern, ReactionReason, SensorEvent,
};
use grok_ball::types::BallExpression;

fn magnetic(at_ms: u64, kind: EventKind) -> SensorEvent {
    SensorEvent {
        at_ms,
        source: EventSource::Bmm150,
        kind,
    }
}

#[test]
fn approach_is_curious_for_2000_ms() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(magnetic(100, EventKind::MagneticApproach));
    let approach = engine.tick(100);
    assert_eq!(approach.state.expression, BallExpression::Curious);
    assert_eq!(approach.state.reason, ReactionReason::Magnetic);
    assert_eq!(approach.haptic, None);

    assert_eq!(engine.tick(2_099).state.expression, BallExpression::Curious);
    assert_eq!(engine.tick(2_100).state.expression, BallExpression::Idle);
}

#[test]
fn contact_is_satisfied_and_uses_the_magnetic_haptic() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(magnetic(100, EventKind::MagneticContact));
    let contact = engine.tick(100);
    assert_eq!(contact.state.expression, BallExpression::Satisfied);
    assert_eq!(contact.state.reason, ReactionReason::Magnetic);
    assert_eq!(contact.haptic, Some(HapticPattern::MagneticContact));

    assert_eq!(
        engine.tick(1_599).state.expression,
        BallExpression::Satisfied
    );
    assert_eq!(engine.tick(1_600).state.expression, BallExpression::Idle);
}

#[test]
fn duplicate_contact_without_release_is_ignored() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(magnetic(100, EventKind::MagneticContact));
    assert_eq!(
        engine.tick(100).haptic,
        Some(HapticPattern::MagneticContact)
    );

    // No release: this contact is not a new transition.
    engine.on_event(magnetic(500, EventKind::MagneticContact));
    let duplicate = engine.tick(500);
    assert_eq!(duplicate.state.expression, BallExpression::Satisfied);
    assert_eq!(duplicate.haptic, None);

    // The first contact still ends at 1600, not 2000.
    assert_eq!(engine.tick(1_600).state.expression, BallExpression::Idle);
}

#[test]
fn release_requires_a_contact() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(magnetic(100, EventKind::MagneticRelease));
    assert_eq!(engine.tick(100).state.expression, BallExpression::Idle);

    engine.on_event(magnetic(1_000, EventKind::MagneticContact));
    assert_eq!(
        engine.tick(1_000).state.expression,
        BallExpression::Satisfied
    );

    engine.on_event(magnetic(1_200, EventKind::MagneticRelease));
    let release = engine.tick(1_200);
    assert_eq!(release.state.expression, BallExpression::Confused);
    assert_eq!(release.state.reason, ReactionReason::Magnetic);

    assert_eq!(
        engine.tick(3_199).state.expression,
        BallExpression::Confused
    );
    assert_eq!(engine.tick(3_200).state.expression, BallExpression::Idle);
}

#[test]
fn duplicate_release_does_not_restart_confused() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(magnetic(100, EventKind::MagneticContact));
    engine.tick(100);
    engine.on_event(magnetic(200, EventKind::MagneticRelease));
    assert_eq!(engine.tick(200).state.expression, BallExpression::Confused);

    engine.on_event(magnetic(400, EventKind::MagneticRelease));
    let duplicate = engine.tick(400);
    assert_eq!(duplicate.state.expression, BallExpression::Confused);

    // Confused started at 200 and ends at 2200, not 2400.
    assert_eq!(
        engine.tick(2_199).state.expression,
        BallExpression::Confused
    );
    assert_eq!(engine.tick(2_200).state.expression, BallExpression::Idle);
}

#[test]
fn orbit_is_surprised_for_1200_ms() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(magnetic(100, EventKind::MagneticOrbit));
    let orbit = engine.tick(100);
    assert_eq!(orbit.state.expression, BallExpression::Surprised);
    assert_eq!(orbit.state.reason, ReactionReason::Magnetic);
    assert_eq!(orbit.haptic, Some(HapticPattern::Surprised));

    assert_eq!(
        engine.tick(1_299).state.expression,
        BallExpression::Surprised
    );
    assert_eq!(engine.tick(1_300).state.expression, BallExpression::Idle);
}

#[test]
fn magnetic_edge_confidence_gate() {
    let mut engine = AffectEngine::new(0);

    engine.on_event(magnetic(100, EventKind::MagneticEdge { confidence: 49 }));
    assert_eq!(engine.tick(100).state.expression, BallExpression::Idle);

    engine.on_event(magnetic(200, EventKind::MagneticEdge { confidence: 50 }));
    let edge = engine.tick(200);
    assert_eq!(edge.state.expression, BallExpression::Puzzled);
    assert_eq!(edge.state.reason, ReactionReason::Magnetic);
    assert_eq!(edge.haptic, Some(HapticPattern::Puzzled));

    assert_eq!(engine.tick(1_699).state.expression, BallExpression::Puzzled);
    assert_eq!(engine.tick(1_700).state.expression, BallExpression::Idle);
}
