use super::*;
use {
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::EventSource,
    crate::companion::behavior::affect::ReactionReason,
};

#[cfg(feature = "ts-engine")]
#[test]
fn ts_rejects_lua_source_and_retains_native_reactions() {
    let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
    assert!(runtime
        .enable_brain("return {}", 0, &app_host::GLOBAL_ALLOCATOR, None)
        .is_err());
    assert!(!runtime.brain_active());
    assert!(runtime.brain_error().unwrap().contains("bytecode envelope"));
    runtime.on_sensor(SensorEvent {
        at_ms: 100,
        source: EventSource::Touch,
        kind: EventKind::TouchPoke,
    });
    runtime.tick(100);
    assert_eq!(runtime.expression(), Expression::Puzzled);
}
use {
    crate::companion::behavior::personality::SCREEN_CENTER_X,
    crate::companion::behavior::personality::SCREEN_CENTER_Y,
};

#[test]
fn bundled_bootstrap_retains_pre_mount_inputs_and_bad_selection_falls_back_warm() {
    let mut native = Runtime::new(0, FoxControl::new(Expression::Idle));
    let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
    runtime
        .enable_brain(
            crate::companion::brain::DEFAULT_SOURCE,
            0,
            &app_host::GLOBAL_ALLOCATOR,
            None,
        )
        .unwrap();
    runtime.ignore_touch_until(1000);
    native.ignore_touch_until(1000);
    // These arrive while USB/storage startup is pending. Module insertion
    // is an edge, and must not disappear into a newly created Idle engine.
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
        native.on_sensor(event);
        runtime.on_sensor(event);
    }
    assert_eq!(native.tick(100), runtime.tick(100));
    assert_eq!(runtime.expression(), native.expression());
    assert_ne!(runtime.expression(), Expression::Idle);
    // An absent persisted selection leaves this active VM untouched. Its
    // module sequence continues instead of restarting at storage-ready.
    assert_eq!(native.tick(1000), runtime.tick(1000));
    assert_eq!(runtime.expression(), native.expression());
    assert_eq!(native.tick(6000), runtime.tick(6000));
    assert_eq!(runtime.expression(), Expression::Dying);
    assert_eq!(runtime.expression(), native.expression());
    // A present but bad persisted source deliberately switches to the
    // native engines that consumed the very same early inputs.
    assert!(runtime
        .enable_brain(
            "error('bad persisted selection')",
            6100,
            &app_host::GLOBAL_ALLOCATOR,
            None
        )
        .is_err());
    assert!(!runtime.brain_active());
    assert_eq!(runtime.expression(), Expression::Dying);
    assert_eq!(native.tick(6200), runtime.tick(6200));
    assert_eq!(runtime.expression(), native.expression());
    #[cfg(feature = "lua-engine")]
    let happy =
        "return {on_event=function()end,tick=function()fox.set_expression('Happy')end}".to_string();
    #[cfg(feature = "ts-engine")]
    let happy = crate::companion::brain::ts_vm::encode(include_bytes!(
        "../../../../../resources/embedts/fixtures/brain-happy.etb"
    ));
    runtime.reload_brain(&happy, 6300).unwrap();
    assert!(runtime.brain_active());
    assert_eq!(runtime.expression(), Expression::Happy);
}

#[cfg(feature = "lua-engine")]
#[test]
fn native_urgent_reflexes_override_custom_brain_and_default_off_home_filter_matches() {
    let control = FoxControl::new(Expression::Idle);
    let mut custom = Runtime::new(0, Rc::clone(&control));
    custom.enable_brain("return {on_event=function()end,tick=function()fox.set_expression('Happy');fox.play('Charging',5000);haptic.play('Happy')end}",0,&app_host::GLOBAL_ALLOCATOR,None).unwrap();
    custom.on_sensor(SensorEvent {
        at_ms: 100,
        source: EventSource::System,
        kind: EventKind::Activity(Expression::Error),
    });
    assert_eq!(custom.expression(), Expression::Error);
    assert_eq!(control.play_pose(), None);
    assert_ne!(custom.tick(100), Some(HapticPattern::Happy));
    custom.on_sensor(SensorEvent {
        at_ms: 200,
        source: EventSource::System,
        kind: EventKind::Activity(Expression::Idle),
    });
    assert_eq!(custom.expression(), Expression::Happy);
    let mut native = Runtime::new(0, FoxControl::new(Expression::Idle));
    let mut lua = Runtime::new(0, FoxControl::new(Expression::Idle));
    lua.enable_brain(
        crate::companion::brain::DEFAULT_SOURCE,
        0,
        &app_host::GLOBAL_ALLOCATOR,
        None,
    )
    .unwrap();
    for kind in [
        EventKind::TouchPoke,
        EventKind::KeyDown,
        EventKind::Accel {
            ax_mg: 2000,
            ay_mg: 0,
            az_mg: 0,
        },
        EventKind::BatteryLevel(3),
    ] {
        if crate::shell::model::reaches_fox_off_home(&kind) {
            let event = SensorEvent {
                at_ms: 100,
                source: EventSource::Touch,
                kind,
            };
            native.on_sensor(event);
            lua.on_sensor(event);
        }
    }
    assert_eq!(native.tick(500), lua.tick(500));
    assert_eq!(native.expression(), lua.expression());
}

#[cfg(feature = "lua-engine")]
#[test]
fn resident_runtime_is_warm_and_recovers_after_bad_boot_and_runtime_failure() {
    let control = FoxControl::new(Expression::Idle);
    let mut native = Runtime::new(0, Rc::clone(&control));
    let other = FoxControl::new(Expression::Idle);
    let mut lua = Runtime::new(0, other);
    assert!(lua
        .enable_brain(
            "error('bad persisted source')",
            0,
            &app_host::GLOBAL_ALLOCATOR,
            None
        )
        .is_err());
    assert!(!lua.brain_active());
    assert!(lua.brain_error().is_some());
    lua.reload_brain(crate::companion::brain::DEFAULT_SOURCE, 0)
        .unwrap();
    let events = [
        SensorEvent {
            at_ms: 100,
            source: EventSource::System,
            kind: EventKind::Activity(Expression::Listening),
        },
        SensorEvent {
            at_ms: 300,
            source: EventSource::Touch,
            kind: EventKind::TouchPet,
        },
        SensorEvent {
            at_ms: 500,
            source: EventSource::Light,
            kind: EventKind::LightLevel(60),
        },
    ];
    for event in events {
        native.on_sensor(event);
        lua.on_sensor(event);
        assert_eq!(native.expression(), lua.expression());
        assert_eq!(native.tick(event.at_ms), lua.tick(event.at_ms));
    }
    lua.reload_brain("return {on_event=function()fox.set_expression('Angry');error('runtime fault')end,tick=function()fox.set_expression('Happy')end}",600).unwrap();
    assert_eq!(lua.expression(), Expression::Happy);
    let event = SensorEvent {
        at_ms: 700,
        source: EventSource::Battery,
        kind: EventKind::BatteryLevel(3),
    };
    native.on_sensor(event);
    lua.on_sensor(event);
    assert!(!lua.brain_active());
    assert_eq!(lua.expression(), native.expression());
    assert_eq!(lua.tick(10000), native.tick(10000));
    assert_eq!(lua.expression(), native.expression());
    lua.reload_brain("return {on_event=function()end,tick=function()fox.play('Charging',100);led_strip.set(1,255,0,0);haptic.play('Happy')end}",11000).unwrap();
    assert!(lua.brain_leds().is_some());
    lua.begin_shutdown();
    assert_eq!(lua.expression(), Expression::Sleep);
    assert!(lua.brain_leds().is_none());
    assert_eq!(lua.tick(12000), None);
    assert!(lua
        .reload_brain(crate::companion::brain::DEFAULT_SOURCE, 12000)
        .is_err());
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
