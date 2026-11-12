use super::*;
use {
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::EventSource,
    crate::companion::behavior::affect::ReactionReason,
};

use {
    crate::companion::behavior::personality::SCREEN_CENTER_X,
    crate::companion::behavior::personality::SCREEN_CENTER_Y,
};

#[test]
fn native_boot_retains_early_module_battery_and_light_inputs() {
    let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
    runtime.ignore_touch_until(1000);
    for event in [
        SensorEvent {
            at_ms: 10,
            source: EventSource::ModuleRight,
            kind: EventKind::ModuleInserted(
                crate::companion::behavior::affect::ModuleKind::Interaction,
            ),
        },
        SensorEvent {
            at_ms: 20,
            source: EventSource::Battery,
            kind: EventKind::BatteryLevel(3),
        },
        SensorEvent {
            at_ms: 30,
            source: EventSource::Light,
            kind: EventKind::LightLevel(60),
        },
    ] {
        runtime.on_sensor(event);
    }
    runtime.tick(100);
    assert_ne!(runtime.expression(), Expression::Idle);
    assert!(!runtime.on_pointer(PointerSample {
        at_ms: 100,
        x: SCREEN_CENTER_X,
        y: SCREEN_CENTER_Y,
        pressed: true
    }));
    runtime.tick(6000);
    assert_eq!(runtime.expression(), Expression::Dying);
    assert_eq!(runtime.affect_state().expression, Expression::Dying);
}

#[test]
fn system_actions_are_consumed_once_and_shutdown_discards_both_pending_stages() {
    use crate::companion::behavior::affect::SystemCommand;
    for (kind, expected) in [
        (EventKind::ButtonShort, SystemCommand::StartListening),
        (EventKind::ButtonLong, SystemCommand::Hub),
        (EventKind::ButtonDouble, SystemCommand::StopActivity),
    ] {
        let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
        runtime.on_sensor(SensorEvent {
            at_ms: 100,
            source: EventSource::ButtonBoot,
            kind,
        });
        runtime.tick(100);
        assert_eq!(
            runtime.take_command(),
            None,
            "Boot key cannot issue AI actions"
        );
        runtime.on_sensor(SensorEvent {
            at_ms: 200,
            source: EventSource::ButtonPower,
            kind,
        });
        runtime.tick(200);
        runtime.tick(210);
        assert_eq!(runtime.take_command(), Some(expected));
        assert_eq!(runtime.take_command(), None);
        runtime.tick(220);
        assert_eq!(runtime.take_command(), None);
        for tick_before_shutdown in [false, true] {
            let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
            runtime.on_sensor(SensorEvent {
                at_ms: 300,
                source: EventSource::ButtonPower,
                kind,
            });
            if tick_before_shutdown {
                runtime.tick(300);
            }
            runtime.begin_shutdown();
            assert_eq!(runtime.take_command(), None);
            assert_eq!(runtime.tick(400), None);
            assert_eq!(runtime.take_command(), None);
        }
    }
}

fn tap(runtime: &mut Runtime, at_ms: u64) {
    runtime.on_pointer(PointerSample {
        at_ms,
        x: SCREEN_CENTER_X,
        y: SCREEN_CENTER_Y,
        pressed: true,
    });
    runtime.on_pointer(PointerSample {
        at_ms: at_ms + 40,
        x: SCREEN_CENTER_X,
        y: SCREEN_CENTER_Y,
        pressed: false,
    });
}

#[test]
fn shutdown_overrides_reactions_and_is_terminal_without_haptics() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));
    runtime.on_sensor(SensorEvent {
        at_ms: 100,
        source: EventSource::ButtonPower,
        kind: EventKind::ButtonShort,
    });
    runtime.begin_shutdown();
    assert_eq!(runtime.expression(), Expression::Sleep);
    runtime.on_sensor(SensorEvent {
        at_ms: 200,
        source: EventSource::System,
        kind: EventKind::Activity(Expression::Error),
    });
    tap(&mut runtime, 300);
    runtime.on_interaction(500);
    assert_eq!(runtime.tick(10_000), None);
    assert_eq!(control.expression(), Expression::Sleep);
    assert_eq!(control.visual(), FoxVisual::default());
    runtime.begin_shutdown();
    assert_eq!(runtime.tick(20_000), None);
}

#[test]
fn a_screen_tap_wakes_her_drowsy_instead_of_a_tap_reaction() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));
    runtime.on_sensor(SensorEvent {
        at_ms: 0,
        source: EventSource::Light,
        kind: EventKind::LightLevel(5),
    });
    runtime.tick(60_000);
    assert_eq!(runtime.expression(), Expression::Sleep);

    tap(&mut runtime, 61_000);
    runtime.tick(61_300);
    assert_eq!(runtime.expression(), Expression::Tired);
    assert_eq!(runtime.affect_state().reason, ReactionReason::Wake);

    // A second tap while she is still drowsy makes her grumpy.
    tap(&mut runtime, 62_000);
    runtime.tick(62_100);
    assert_eq!(runtime.expression(), Expression::Angry);
}

#[test]
fn the_fresh_runtime_shows_the_idle_face() {
    let control = FoxControl::new(Expression::Idle);
    let runtime = Runtime::new(0, Rc::clone(&control));
    assert_eq!(runtime.expression(), Expression::Idle);
    assert_eq!(runtime.affect_state().reason, ReactionReason::Idle);
}

#[test]
fn a_sensor_reaction_reaches_the_fox() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));
    runtime.on_sensor(SensorEvent {
        at_ms: 1_000,
        source: EventSource::Touch,
        kind: EventKind::TouchPoke,
    });
    assert_eq!(runtime.expression(), Expression::Puzzled);
    assert_eq!(runtime.affect_state().reason, ReactionReason::TouchPoke);
}

#[test]
fn a_personality_reaction_outranks_an_affect_reaction() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));

    runtime.on_sensor(SensorEvent {
        at_ms: 1_000,
        source: EventSource::Touch,
        kind: EventKind::TouchPoke,
    });
    assert_eq!(runtime.expression(), Expression::Puzzled);

    tap(&mut runtime, 1_100);
    runtime.tick(1_100 + 40 + 220);
    assert_eq!(
        runtime.expression(),
        Expression::Curious,
        "the tap answers with its own reaction"
    );

    // Both layers have expired: the resting face returns.
    runtime.tick(1_100 + 40 + 220 + 2_000);
    assert_eq!(runtime.expression(), Expression::Idle);
}

fn accel(runtime: &mut Runtime, at_ms: u64, az_mg: i16) {
    runtime.on_sensor(SensorEvent {
        at_ms,
        source: EventSource::Imu,
        kind: EventKind::Accel {
            ax_mg: 0,
            ay_mg: 0,
            az_mg,
        },
    });
}

#[test]
fn a_shake_panic_survives_the_tap_that_follows_it() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));

    // A violent shake, five hard peaks inside a second: the affect engine
    // panics.
    for n in (0..5u64).rev() {
        accel(&mut runtime, 990 - n * 60, 1_000);
        accel(&mut runtime, 1_000 - n * 60, 2_500);
    }
    runtime.tick(1_000);
    assert_eq!(runtime.expression(), Expression::Panic);

    // A tap during the scream must not cover it.
    tap(&mut runtime, 1_100);
    runtime.tick(1_100 + 40 + 220);
    assert_eq!(
        runtime.expression(),
        Expression::Panic,
        "an urgent affect state outranks a touch reaction"
    );
}

#[test]
fn an_error_survives_a_tap() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));
    runtime.on_sensor(SensorEvent {
        at_ms: 1_000,
        source: EventSource::System,
        kind: EventKind::Activity(Expression::Error),
    });
    runtime.tick(1_000);
    assert_eq!(runtime.expression(), Expression::Error);

    tap(&mut runtime, 1_100);
    runtime.tick(1_100 + 40 + 220);
    assert_eq!(runtime.expression(), Expression::Error);
}

#[test]
fn merge_order_matches_the_haptic_order() {
    use Expression as E;
    assert_eq!(merge_expression(Some(E::Curious), E::Error), E::Error);
    assert_eq!(merge_expression(Some(E::Angry), E::Panic), E::Panic);
    assert_eq!(merge_expression(Some(E::Curious), E::Puzzled), E::Curious);
    assert_eq!(merge_expression(None, E::Tired), E::Tired);
}

#[test]
fn a_low_battery_face_is_the_resting_layer() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));
    runtime.on_sensor(SensorEvent {
        at_ms: 1_000,
        source: EventSource::Battery,
        kind: EventKind::BatteryLevel(10),
    });
    assert_eq!(runtime.expression(), Expression::Tired);
}

#[test]
fn the_runtime_publishes_the_tap_pulse_and_gaze() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));

    runtime.on_pointer(PointerSample {
        at_ms: 1_000,
        x: SCREEN_CENTER_X + 80,
        y: SCREEN_CENTER_Y,
        pressed: true,
    });
    runtime.tick(1_020);
    let visual = runtime.visual();
    assert!(visual.pulse > 0.0, "the press pulses");
    assert!(visual.look_x > 0.0, "the eyes glance toward the finger");
}

#[test]
fn a_reaction_buzzes_once_and_not_again() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));

    tap(&mut runtime, 1_000);
    assert_eq!(runtime.tick(1_100), None, "a single tap does not buzz");

    tap(&mut runtime, 1_400);
    assert_eq!(
        runtime.tick(1_400 + 40 + 380),
        Some(HapticPattern::Puzzled),
        "the second tap buzzes when its reaction starts"
    );
    assert_eq!(runtime.tick(1_400 + 40 + 400), None, "no repeat buzz");
}

#[test]
fn the_touch_hold_off_suppresses_taps() {
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = Runtime::new(0, Rc::clone(&control));
    runtime.ignore_touch_until(3_000);

    // A discarded touch must not request an immediate response frame.
    let acted = runtime.on_pointer(PointerSample {
        at_ms: 1_000,
        x: SCREEN_CENTER_X,
        y: SCREEN_CENTER_Y,
        pressed: true,
    });
    assert!(!acted, "a held-off press is reported as not acted on");
    runtime.on_pointer(PointerSample {
        at_ms: 1_040,
        x: SCREEN_CENTER_X,
        y: SCREEN_CENTER_Y,
        pressed: false,
    });
    runtime.tick(1_500);
    assert_eq!(runtime.expression(), Expression::Idle);

    let acted = runtime.on_pointer(PointerSample {
        at_ms: 3_100,
        x: SCREEN_CENTER_X,
        y: SCREEN_CENTER_Y,
        pressed: true,
    });
    assert!(acted, "after the hold-off the press is real");
    runtime.on_pointer(PointerSample {
        at_ms: 3_140,
        x: SCREEN_CENTER_X,
        y: SCREEN_CENTER_Y,
        pressed: false,
    });
    runtime.tick(3_100 + 40 + 220);
    assert_eq!(runtime.expression(), Expression::Curious);
}
