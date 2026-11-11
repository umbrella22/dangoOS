use super::input::{Inbox, Input};
use super::*;
use crate::companion::behavior::{
    affect::{EventKind, EventSource, ModuleKind, SensorEvent},
    personality::PointerSample,
};
use alloc::{string::String, vec::Vec};
#[test]
fn ts_default_matches_lua_outputs_across_sensor_pointer_and_time_transitions() {
    let mut ts = BrainVm::new(DEFAULT_SOURCE, 0, &app_host::GLOBAL_ALLOCATOR, None).unwrap();
    let mut lua = lua_oracle::BrainVm::new(
        include_str!("../../../assets/brain/default.lua"),
        0,
        &app_host::GLOBAL_ALLOCATOR,
        None,
    )
    .unwrap();
    let mut inputs = Vec::new();
    let mut now = 0;
    let mut push = |kind: EventKind, source: EventSource| {
        now += 100;
        inputs.push((
            Input::Sensor(SensorEvent {
                at_ms: now,
                source,
                kind,
            }),
            now,
        ));
        inputs.push((Input::Tick(now), now));
    };
    for _ in 0..8 {
        push(EventKind::TouchPoke, EventSource::Touch);
    }
    for (kind, source) in [
        (
            EventKind::Gyro {
                gx_dps: i16::MIN,
                gy_dps: 0,
                gz_dps: i16::MAX,
            },
            EventSource::Imu,
        ),
        (EventKind::TouchPet, EventSource::Touch),
        (EventKind::KeyDown, EventSource::KeyLeft),
        (EventKind::KeyUp, EventSource::KeyLeft),
        (EventKind::ButtonShort, EventSource::ButtonPower),
        (EventKind::ButtonDouble, EventSource::ButtonPower),
        (EventKind::ButtonLong, EventSource::ButtonBoot),
        (EventKind::MagneticApproach, EventSource::Bmm150),
        (EventKind::MagneticContact, EventSource::Bmm150),
        (EventKind::MagneticRelease, EventSource::Bmm150),
        (EventKind::MagneticOrbit, EventSource::Bmm150),
        (
            EventKind::MagneticEdge { confidence: 100 },
            EventSource::Bmm150,
        ),
        (
            EventKind::ModuleInserted(ModuleKind::Camera),
            EventSource::ModuleLeft,
        ),
        (
            EventKind::ModuleRemoved(ModuleKind::Camera),
            EventSource::ModuleLeft,
        ),
    ] {
        push(kind, source);
    }
    for expression in crate::Expression::ALL {
        push(EventKind::Activity(expression), EventSource::System);
    }
    for level in [100, 60, 10, 0, 40, 90] {
        push(EventKind::LightLevel(level), EventSource::Light);
    }
    for value in [true, false, true] {
        push(EventKind::Presence(value), EventSource::Pir);
    }
    for value in [100, 15, 5, 7, 8, 16, 18, 100] {
        push(EventKind::BatteryLevel(value), EventSource::Battery);
    }
    for value in [true, false] {
        push(EventKind::NetworkConnection(value), EventSource::System);
        push(EventKind::BluetoothConnection(value), EventSource::System);
        push(EventKind::MusicPlayback(value), EventSource::System);
    }
    for kind in [
        EventKind::ModuleInserted(ModuleKind::Interaction),
        EventKind::KeyDown,
        EventKind::KeyUp,
        EventKind::ModuleRemoved(ModuleKind::Interaction),
    ] {
        push(kind, EventSource::ModuleRight);
    }
    for _ in 0..8 {
        push(
            EventKind::Accel {
                ax_mg: 0,
                ay_mg: 0,
                az_mg: 2400,
            },
            EventSource::Imu,
        );
        push(
            EventKind::Accel {
                ax_mg: 0,
                ay_mg: 0,
                az_mg: 1000,
            },
            EventSource::Imu,
        );
    }
    drop(push);
    for i in 0..20 {
        now += 200;
        inputs.push((
            Input::Pointer(PointerSample {
                at_ms: now,
                x: 100 + i * 9,
                y: 240,
                pressed: i % 4 != 3,
            }),
            now,
        ));
        inputs.push((Input::Tick(now), now));
    }
    for offset in [1, 500, 3000, 15000, 60000, 300000] {
        let at = now + offset;
        inputs.push((Input::Tick(at), at));
    }
    for (n, (input, at)) in inputs.into_iter().enumerate() {
        let t = ts.rpc_inbox(Inbox::Typed(input), at).unwrap();
        let l = lua.rpc_inbox(Inbox::Typed(input), at).unwrap();
        assert_eq!(t, l, "response #{n} {input:?}");
        let t = ts.output();
        let l = lua.output();
        assert_eq!(t.expression, l.expression, "expression #{n} {input:?}");
        assert_eq!(t.haptic, l.haptic, "haptic #{n}");
        assert_eq!(t.command, l.command, "command #{n}");
        assert!(
            (t.visual.look_x - l.visual.look_x).abs() < 0.0001,
            "look #{n}"
        );
        assert!((t.visual.pet - l.visual.pet).abs() < 0.0001, "pet #{n}");
    }
    assert!(ts.memory_used() < HEAP_MAX);
}
fn compiled(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../resources/embedts/fixtures")
        .join(name);
    ts_vm::encode(&std::fs::read(path).unwrap())
}
#[test]
fn ts_brain_candidate_failures_and_durable_commit_failures_keep_old_outputs() {
    let happy = compiled("brain-happy.etb");
    let angry = compiled("brain-angry.etb");
    let mut c = Controller::new(&happy, 0, &app_host::GLOBAL_ALLOCATOR, None).unwrap();
    for source in [
        "brain-loop.etb",
        "brain-forbidden.etb",
        "brain-bad-first.etb",
    ] {
        assert!(c
            .replace_with_commit(&compiled(source), 1, || panic!(
                "invalid candidate reached commit"
            ))
            .is_err());
        assert_eq!(c.output().unwrap().expression, crate::Expression::Happy);
    }
    assert!(c
        .replace_with_commit(&angry, 2, || Err("disk full".into()))
        .is_err());
    assert_eq!(c.source(), happy);
    assert_eq!(c.output().unwrap().expression, crate::Expression::Happy);
    c.replace_with_commit(&angry, 3, || Ok(())).unwrap();
    assert_eq!(c.output().unwrap().expression, crate::Expression::Angry);
    let fault = compiled("brain-later-fault.etb");
    c.replace(&fault, 4).unwrap();
    c.tick(5);
    assert!(!c.active());
    assert!(c.error().unwrap().contains("later failure"));
}
