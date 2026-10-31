use super::interface::*;
use super::*;
use crate::companion::behavior::Expression;
use crate::companion::render::FoxPose;
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
};
use lua_engine::{Host, Limits, RawAllocator, Step, Vm};

#[test]
fn source_compilation_latency_does_not_spend_callback_wall_budget() {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static NOW: AtomicU64 = AtomicU64::new(0);
    static DELAYED: AtomicBool = AtomicBool::new(false);
    fn clock() -> u64 {
        NOW.load(Ordering::SeqCst)
    }
    unsafe fn allocate(layout: core::alloc::Layout) -> *mut u8 {
        // A real user-chunk string constant forces parser allocations.
        // Charge EVERY large allocation, so reparsing the source during
        // execution would consume a fresh >20ms and fail this test.
        if layout.size() > 16_000 {
            DELAYED.store(true, Ordering::SeqCst);
            NOW.fetch_add(CALLBACK_US + 1, Ordering::SeqCst);
        }
        unsafe { (lua_engine::GLOBAL_ALLOCATOR.alloc)(layout) }
    }
    unsafe fn deallocate(ptr: *mut u8, layout: core::alloc::Layout) {
        unsafe { (lua_engine::GLOBAL_ALLOCATOR.dealloc)(ptr, layout) }
    }
    static ALLOCATOR: RawAllocator = RawAllocator {
        alloc: allocate,
        dealloc: deallocate,
    };
    NOW.store(0, Ordering::SeqCst);
    DELAYED.store(false, Ordering::SeqCst);
    let source = format!(
        "local text='{}'\nassert(#text==24000)\nreturn {{on_event=function()end,tick=function()fox.set_expression('Happy')end}}",
        "x".repeat(24_000)
    );
    let mut brain = BrainVm::new(&source, 0, &ALLOCATOR, Some(clock)).unwrap();
    assert!(DELAYED.load(Ordering::SeqCst));
    assert!(NOW.load(Ordering::SeqCst) > CALLBACK_US);
    assert_eq!(brain.output().expression, Expression::Happy);

    // Negative control: the old runtime load(source) reparses the same
    // constant after arming, and must fail with this same allocator/clock.
    let mut legacy = Vm::new(
        Box::new(BrainHost {
            inbox: None,
            response: None,
            output: Output::default(),
            now: 0,
        }),
        Limits {
            clock: Some(clock),
            hook_interval: 100,
            ..Limits::default()
        },
        &ALLOCATOR,
        1,
    )
    .unwrap();
    legacy
        .load(
            "runtime-reparse-control",
            format!(
                "local source={};assert(load(source))();for i=1,1000 do end",
                serde_json::to_string(&source).unwrap(),
            )
            .as_bytes(),
        )
        .unwrap();
    legacy.set_callback_budget(CALLBACK_HOOKS, CALLBACK_US);
    let error = loop {
        match legacy.resume() {
            Step::Slice => continue,
            Step::Error(error) => break error,
            other => panic!("runtime reparse escaped its budget: {other:?}"),
        }
    };
    assert!(
        error.contains("callback budget exhausted (wall;"),
        "{error}"
    );

    // Large allocations caused by USER EXECUTION remain inside the same
    // wall budget, even though compilation now occurs before it.
    let error = BrainVm::new(
        "local t={};for i=1,3000 do t[i]=i end;while true do end",
        0,
        &ALLOCATOR,
        Some(clock),
    )
    .err()
    .unwrap();
    assert!(
        error.contains("callback budget exhausted (wall;"),
        "{error}"
    );
}

#[test]
fn standalone_source_keeps_lexical_isolation_lines_and_size_boundary() {
    let allocator = &lua_engine::GLOBAL_ALLOCATOR;
    let isolated = r#"
          assert(SOURCE==nil and START_MS==nil and module==nil and host==nil and safe==nil and json==nil)
          assert(load==nil and require==nil and debug==nil and package==nil)
          assert(_G==_ENV)
          return {on_event=function()end,tick=function()fox.set_expression('Happy')end}
        "#;
    let mut brain = BrainVm::new(isolated, 123, allocator, None).unwrap();
    assert_eq!(brain.output().expression, Expression::Happy);
    for source in [
        "end; fox.set_expression('Angry'); local x=function()",
        "local ok=true\nerror('line receipt')",
    ] {
        let err = BrainVm::new(source, 0, allocator, None).err().unwrap();
        assert!(
            err.contains(if source.starts_with("end") {
                "brain:1:"
            } else {
                "brain:2:"
            }),
            "{err}"
        );
    }
    let mut maximum = String::from("return {on_event=function()end,tick=function()end}\n--");
    maximum.push_str(&"x".repeat(SOURCE_MAX - maximum.len()));
    assert_eq!(maximum.len(), SOURCE_MAX);
    assert!(BrainVm::new(&maximum, 0, allocator, None).is_ok());
    maximum.push('x');
    assert_eq!(
        BrainVm::new(&maximum, 0, allocator, None).err().unwrap(),
        "brain source exceeds 64 KiB"
    );
}

#[test]
fn bounded_adversaries_and_transactional_replacement() {
    let allocator = &lua_engine::GLOBAL_ALLOCATOR;
    for source in [
        "while true do end",
        "while true do pcall(function() while true do end end) end",
        "local x <close> = setmetatable({}, {__close=function() while true do end end}); error('close')",
        "local t={};while true do t[#t+1]={} end",
        "local x=setmetatable({}, {__gc=function() while true do pcall(function()error('caught')end) end end});error('drop')",
        "return {on_event=function()end,tick=function()while true do end end}",
    ] {
        assert!(
            BrainVm::new(source, 0, allocator, None).is_err(),
            "adversary accepted: {source}"
        );
    }
    let mut brain = Controller::new(
        "return {on_event=function()end,tick=function()fox.set_expression('Happy')end}",
        0,
        allocator,
        None,
    )
    .unwrap();
    assert_eq!(brain.output().unwrap().expression, Expression::Happy);
    let old = brain.source().to_string();
    assert!(brain.replace("return {on_event=function()end,tick=function()fox.set_expression('Angry');error('reject')end}",1).is_err());
    assert_eq!(brain.source(), old);
    assert_eq!(brain.output().unwrap().expression, Expression::Happy);
    let next = "return {on_event=function()end,tick=function()fox.set_expression('Curious')end}";
    assert!(brain
        .replace_with_commit(next, 2, || Err("disk full".into()))
        .is_err());
    assert_eq!(brain.source(), old);
    assert_eq!(brain.output().unwrap().expression, Expression::Happy);
    brain.replace(next, 3).unwrap();
    assert_eq!(brain.output().unwrap().expression, Expression::Curious);
}
#[test]
fn async_commit_failure_or_cancellation_keeps_active_vm_and_output() {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    let old = "return {on_event=function()end,tick=function()fox.set_expression('Happy')end}";
    let next = "return {on_event=function()end,tick=function()fox.set_expression('Curious')end}";
    let mut brain = Controller::new(old, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut change =
            pin!(brain.replace_with_commit_async(next, 1, async || Err("disk full".into())));
        assert!(matches!(change.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
    }
    assert_eq!(brain.source(), old);
    assert_eq!(brain.output().unwrap().expression, Expression::Happy);
    {
        let mut change =
            pin!(brain.replace_with_commit_async(next, 2, async || core::future::pending().await));
        assert!(change.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(brain.source(), old);
    assert_eq!(brain.output().unwrap().expression, Expression::Happy);
    {
        let mut change = pin!(brain.replace_with_commit_async(next, 3, async || Ok(())));
        assert!(matches!(change.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
    }
    assert_eq!(brain.source(), next);
    assert_eq!(brain.output().unwrap().expression, Expression::Curious);
}
#[test]
fn restricted_gc_and_string_colon_paths_cannot_run_unbounded_code() {
    let source = r#"
            local mt={__gc=function()while true do end end}
            for i=1,10000 do local x=setmetatable({},mt) end
            assert(string.match==nil and ('x').match==nil and string.rep==nil)
            assert(coroutine==nil and debug==nil and io==nil and os==nil and require==nil)
            return {on_event=function()end,tick=function()fox.set_expression('Happy')end}
        "#;
    let mut b = BrainVm::new(source, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    assert_eq!(b.output().expression, Expression::Happy);
    assert!(b.memory_used() <= HEAP_MAX);
}

#[test]
fn foreground_vm_and_resident_callbacks_keep_independent_state() {
    struct Foreground;
    impl Host for Foreground {
        fn print(&mut self, _: &[u8]) {}
    }
    let mut foreground = Vm::new(
        Box::new(Foreground),
        Limits::default(),
        &lua_engine::GLOBAL_ALLOCATOR,
        1,
    )
    .unwrap();
    foreground
        .set_storage_root("/data/appdata/foreground")
        .unwrap();
    foreground
        .load("foreground", b"for i=1,5 do coroutine.yield() end")
        .unwrap();
    let mut brain =
        Controller::new(DEFAULT_SOURCE, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    for now in 1..=5 {
        assert!(matches!(foreground.resume(), Step::Slice));
        brain.tick(now);
        assert!(brain.active());
    }
    assert!(matches!(foreground.resume(), Step::Done));
    brain.tick(100);
    assert!(brain.active());
}

fn raw_command(input: Input) -> (String, u64) {
    match input {
        Input::Tick(now) => (format!("{{\"op\":\"tick\",\"input\":{now}}}"), now),
        Input::Sensor(e) => (
            format!("{{\"op\":\"event\",\"input\":{}}}", sensor_json(e)),
            e.at_ms,
        ),
        Input::Pointer(p) => (
            format!(
                "{{\"op\":\"event\",\"input\":{{\"type\":\"PointerSample\",\"at_ms\":{},\"x\":{},\"y\":{},\"pressed\":{}}}}}",
                p.at_ms, p.x, p.y, p.pressed
            ),
            p.at_ms,
        ),
        Input::Interaction(now) => (
            format!("{{\"op\":\"event\",\"input\":{{\"type\":\"Interaction\",\"at_ms\":{now}}}}}"),
            now,
        ),
        Input::IgnoreTouch(until) => (
            format!(
                "{{\"op\":\"event\",\"input\":{{\"type\":\"IgnoreTouch\",\"at_ms\":{until}}}}}"
            ),
            0,
        ),
    }
}
fn inputs() -> alloc::vec::Vec<Input> {
    use EventKind::*;
    use {
        crate::companion::behavior::affect::EventSource::*,
        crate::companion::behavior::affect::ModuleKind,
    };
    let mut kinds = alloc::vec![
        Accel {
            ax_mg: i16::MIN,
            ay_mg: 0,
            az_mg: i16::MAX
        },
        Gyro {
            gx_dps: i16::MAX,
            gy_dps: i16::MIN,
            gz_dps: -1
        },
        TouchPet,
        TouchPoke,
        LightLevel(0),
        LightLevel(255),
        Presence(false),
        Presence(true),
        NetworkConnection(true),
        NetworkConnection(false),
        BluetoothConnection(true),
        BluetoothConnection(false),
        MusicPlayback(true),
        MusicPlayback(false),
        KeyDown,
        KeyUp,
        ButtonShort,
        ButtonLong,
        ButtonDouble,
        MagneticApproach,
        MagneticContact,
        MagneticRelease,
        MagneticOrbit,
        MagneticEdge { confidence: 255 },
        ModuleInserted(ModuleKind::Interaction),
        ModuleInserted(ModuleKind::Camera),
        ModuleRemoved(ModuleKind::Interaction),
        ModuleRemoved(ModuleKind::Camera),
        BatteryLevel(0),
        BatteryLevel(255),
    ];
    kinds.extend(Expression::ALL.into_iter().map(Activity));
    let sources = [
        Imu,
        Touch,
        Light,
        Pir,
        KeyLeft,
        KeyRight,
        ButtonPower,
        ButtonBoot,
        Bmm150,
        ModuleRight,
        ModuleLeft,
        Battery,
        System,
    ];
    let mut commands = alloc::vec::Vec::new();
    for (index, kind) in kinds.into_iter().enumerate() {
        commands.push(Input::Sensor(SensorEvent {
            at_ms: index as u64 * 100,
            source: sources[index % sources.len()],
            kind,
        }));
    }
    // Include JSON number rounding boundaries, beyond i64 and at u64 max.
    for now in [
        0,
        1,
        1_000,
        (1u64 << 53) - 1,
        1u64 << 53,
        (1u64 << 53) + 1,
        (1u64 << 63) - 1,
        1u64 << 63,
        u64::MAX,
    ] {
        commands.extend([
            Input::Tick(now),
            Input::Interaction(now),
            Input::IgnoreTouch(now),
        ]);
        for pressed in [false, true] {
            commands.push(Input::Pointer(
                crate::companion::behavior::personality::PointerSample {
                    at_ms: now,
                    x: i32::MIN,
                    y: i32::MAX,
                    pressed,
                },
            ));
        }
    }
    commands
}
#[test]
fn typed_inputs_match_raw_structure_and_number_observations() {
    // A sandboxed user brain observes exact structure, array key types,
    // numeric subtypes and tostring; read it back via unchanged raw oracle.
    let source = r#"
local last
local function signature(v)
 local t=type(v)
 if t=='number' then return t..':'..math.type(v)..':'..tostring(v) end
 if t~='table' then return t..':'..tostring(v) end
 local keys={};for k in pairs(v) do keys[#keys+1]=k end
 table.sort(keys,function(a,b)return tostring(a)<tostring(b) end)
 local out={'table['}
 for _,k in ipairs(keys) do out[#out+1]=signature(k)..'='..signature(v[k])..';' end
 return table.concat(out)..']'
end
return {on_event=function(e)last=signature(e);return e.pressed end,
 tick=function(n)last=signature(n) end,
 Affect={new=function()return {read=function()return last end}end}}
"#;
    let mut raw = BrainVm::new(source, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    let mut typed = BrainVm::new(source, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    for brain in [&mut raw, &mut typed] {
        brain
            .rpc(
                r#"{"op":"oracle","method":"affect.new","engine":1,"input":0}"#.into(),
                0,
            )
            .unwrap();
    }
    for input in inputs() {
        let (json, now) = raw_command(input);
        assert_eq!(raw.rpc(json, now), typed.rpc_typed(input, now), "{input:?}");
        assert_eq!(raw.output(), typed.output(), "{input:?}");
        let read = r#"{"op":"oracle","method":"affect.read","engine":1,"input":null}"#;
        assert_eq!(
            raw.rpc(read.into(), now),
            typed.rpc(read.into(), now),
            "{input:?}"
        );
    }
}
#[test]
fn typed_default_brain_replays_raw_outputs_and_expiry() {
    let mut raw = BrainVm::new(DEFAULT_SOURCE, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    let mut typed = BrainVm::new(DEFAULT_SOURCE, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    for input in inputs()
        .into_iter()
        .chain((0..120).map(|n| Input::Tick(n * 100)))
    {
        let (json, now) = raw_command(input);
        assert_eq!(raw.rpc(json, now), typed.rpc_typed(input, now), "{input:?}");
        assert_eq!(raw.output(), typed.output(), "{input:?}");
    }
    // Custom callback makes reset order, side-effect order and host now
    // observable; IgnoreTouch deliberately uses host now=0, not until.
    let source = r#"local n=0
return {tick=function()end,on_event=function(e)
n=n+1
if n==1 then fox.play('Happy',10);haptic.play('Happy');fox.command('Hub');fox.set_expression('Happy');fox.set_expression('Angry');led_strip.set(1,1,2,3)
elseif e.type=='IgnoreTouch' then fox.play('Curious',7) end
end}"#;
    raw = BrainVm::new(source, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    typed = BrainVm::new(source, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
    for input in [
        Input::Interaction(100),
        Input::Tick(109),
        Input::Tick(110),
        Input::IgnoreTouch(999),
        Input::Tick(6),
        Input::Tick(7),
    ] {
        let (json, now) = raw_command(input);
        assert_eq!(raw.rpc(json, now), typed.rpc_typed(input, now));
        assert_eq!(raw.output(), typed.output());
        match input {
            Input::Interaction(_) => {
                assert_eq!(typed.output().play, Some((FoxPose::Happy, 110)))
            }
            Input::IgnoreTouch(_) => {
                assert_eq!(typed.output().play, Some((FoxPose::Curious, 7)))
            }
            Input::Tick(109) => {
                assert!(typed.output().haptic.is_none());
                assert!(typed.output().command.is_none());
                assert!(typed.output().play.is_some());
            }
            Input::Tick(110) | Input::Tick(7) => assert!(typed.output().play.is_none()),
            _ => {}
        }
    }
}
#[test]
fn typed_controller_acceptance_and_failures_match_raw_fallback() {
    for result in [
        "nil",
        "false",
        "true",
        "42",
        "'invalid'",
        "error('failure')",
        "(function()while true do end end)()",
        "(function()local t={} while true do t[#t+1]={1,2,3,4} end end)()",
    ] {
        let source = format!(
            "return {{tick=function()end,on_event=function()fox.set_expression('Happy');return {result} end}}"
        );
        let mut raw = Controller::new(&source, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
        let mut typed = Controller::new(&source, 0, &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
        let p = crate::companion::behavior::personality::PointerSample {
            at_ms: 123,
            x: 1,
            y: 2,
            pressed: true,
        };
        let (json, now) = raw_command(Input::Pointer(p));
        let expected = raw.dispatch(json, now).is_some_and(|v| v != "false");
        assert_eq!(typed.pointer(p), expected, "{result}");
        assert_eq!(raw.active(), typed.active(), "{result}");
        assert_eq!(raw.output(), typed.output(), "{result}");
        assert_eq!(raw.error().is_some(), typed.error().is_some(), "{result}");
        if !raw.active() {
            raw.tick(200);
            typed.tick(200);
            assert!(!typed.active());
            assert!(typed.output().is_none());
        }
    }
}

use {
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::SensorEvent,
};
fn sensor_json(e: SensorEvent) -> String {
    let kind = match e.kind {
        EventKind::Accel {
            ax_mg,
            ay_mg,
            az_mg,
        } => {
            format!("{{\"type\":\"Accel\",\"ax_mg\":{ax_mg},\"ay_mg\":{ay_mg},\"az_mg\":{az_mg}}}")
        }
        EventKind::Gyro {
            gx_dps,
            gy_dps,
            gz_dps,
        } => format!(
            "{{\"type\":\"Gyro\",\"gx_dps\":{gx_dps},\"gy_dps\":{gy_dps},\"gz_dps\":{gz_dps}}}"
        ),
        EventKind::MagneticEdge { confidence } => {
            format!("{{\"type\":\"MagneticEdge\",\"confidence\":{confidence}}}")
        }
        EventKind::LightLevel(v) => format!("{{\"type\":\"LightLevel\",\"args\":[{v}]}}"),
        EventKind::BatteryLevel(v) => format!("{{\"type\":\"BatteryLevel\",\"args\":[{v}]}}"),
        EventKind::Presence(v) => format!("{{\"type\":\"Presence\",\"args\":[{v}]}}"),
        EventKind::NetworkConnection(v) => {
            format!("{{\"type\":\"NetworkConnection\",\"args\":[{v}]}}")
        }
        EventKind::BluetoothConnection(v) => {
            format!("{{\"type\":\"BluetoothConnection\",\"args\":[{v}]}}")
        }
        EventKind::MusicPlayback(v) => format!("{{\"type\":\"MusicPlayback\",\"args\":[{v}]}}"),
        EventKind::ModuleInserted(v) => {
            format!("{{\"type\":\"ModuleInserted\",\"args\":[\"{v:?}\"]}}")
        }
        EventKind::ModuleRemoved(v) => {
            format!("{{\"type\":\"ModuleRemoved\",\"args\":[\"{v:?}\"]}}")
        }
        EventKind::Activity(v) => format!("{{\"type\":\"Activity\",\"args\":[\"{v:?}\"]}}"),
        unit => format!("\"{unit:?}\""),
    };
    format!(
        "{{\"type\":\"SensorEvent\",\"at_ms\":{},\"source\":\"{:?}\",\"kind\":{kind}}}",
        e.at_ms, e.source
    )
}
