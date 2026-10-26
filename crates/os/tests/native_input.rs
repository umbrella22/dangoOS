//! Real LVGL timer-pointer tests for native clean/rebuild transactions.
use lua_engine::ui::UiCommand;
use lvgl::{sys as s, Display};
use os::lua_lvgl::LuaLvglService;
use serde_json::{json, Value};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
static DOWN: AtomicBool = AtomicBool::new(false);
static LATCH: AtomicBool = AtomicBool::new(false);
static READS: AtomicUsize = AtomicUsize::new(0);
static DISABLED_READS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn input(_: *mut s::lv_indev_t, d: *mut s::lv_indev_data_t) {
    READS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        (*d).point.x = 70;
        (*d).point.y = 70;
        // Same one-read short-tap latch as the device port.
        let latched = LATCH.swap(false, Ordering::Relaxed);
        (*d).state = if DOWN.load(Ordering::Relaxed) || latched {
            s::lv_indev_state_t_LV_INDEV_STATE_PRESSED
        } else {
            s::lv_indev_state_t_LV_INDEV_STATE_RELEASED
        };
    }
}
unsafe extern "C" fn disabled(_: *mut s::lv_indev_t, d: *mut s::lv_indev_data_t) {
    DISABLED_READS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        (*d).state = s::lv_indev_state_t_LV_INDEV_STATE_RELEASED;
    }
}
fn call(service: &mut LuaLvglService, op: &str, args: Value) -> Value {
    let reply = service
        .execute(UiCommand {
            op: op.into(),
            args: args.to_string(),
            bytes: vec![],
            root: "/data/input-probe".into(),
        })
        .unwrap();
    serde_json::from_str::<Value>(&reply.json).unwrap()["values"].clone()
}
fn tick() {
    lvgl::tick_inc(Duration::from_millis(40));
    unsafe {
        s::lv_timer_handler();
    }
}
fn pressed() {
    LATCH.store(true, Ordering::Relaxed);
    DOWN.store(true, Ordering::Relaxed);
}
fn released() {
    DOWN.store(false, Ordering::Relaxed);
}
fn button(service: &mut LuaLvglService, screen: u64) {
    let id = call(
        service,
        "create",
        json!(["button",screen,{"w":20,"h":20,"align":"bottom_right","pad":0}]),
    )[0]
    .as_u64()
    .unwrap();
    call(service, "on", json!([id, "clicked", 7]));
}
fn events(service: &mut LuaLvglService) -> Value {
    call(service, "events", json!([0]))
}
#[test]
fn native_input() {
    unsafe {
        lvgl::deinit();
    }
    lvgl::init();
    let display = Display::register::<_, { 80 * 8 }>(80, 80, |_| {}).unwrap();
    let original = unsafe { s::lv_screen_active() };
    let indev = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        s::lv_indev_set_read_cb(p, Some(input));
        p
    };
    let timer = unsafe { s::lv_indev_get_read_timer(indev) };
    let mut service = LuaLvglService::new(80, 80);
    call(&mut service, "init", json!([{}]));
    let screen = call(&mut service, "create", json!(["screen", 0, {}]))[0]
        .as_u64()
        .unwrap();
    call(
        &mut service,
        "set_style",
        json!([screen,{"pad":0,"border_width":0}]),
    );
    call(&mut service, "load", json!([screen]));
    for mode in ["empty", "short-latched", "held"] {
        released();
        LATCH.store(false, Ordering::Relaxed);
        call(&mut service, "clean", json!([screen]));
        button(&mut service, screen);
        assert!(unsafe { s::lv_timer_get_paused(timer) });
        let before = READS.load(Ordering::Relaxed);
        if mode != "empty" {
            pressed();
            tick();
            if mode != "held" {
                released();
                tick();
            }
        }
        assert_eq!(READS.load(Ordering::Relaxed), before);
        assert_eq!(events(&mut service), json!([]));
        call(&mut service, "frame_end", json!([]));
        assert!(!unsafe { s::lv_timer_get_paused(timer) });
        assert_eq!(events(&mut service), json!([]));
        if mode == "held" {
            tick();
            assert_eq!(events(&mut service), json!([]));
            released();
            tick();
            assert_eq!(events(&mut service), json!([]));
        }
        // First new press can arrive before the first post-commit refresh.
        pressed();
        unsafe {
            s::lv_indev_read(indev);
        }
        released();
        unsafe {
            s::lv_indev_read(indev);
        }
        assert_eq!(
            events(&mut service),
            json!([7]),
            "first new tap after {mode}"
        );
        println!("PASS pointer gate {mode}: incomplete-tree tap discarded, first new tap accepted");
    }
    call(&mut service, "clean", json!([screen]));
    call(&mut service, "deinit", json!([]));
    assert_eq!(unsafe { s::lv_screen_active() }, original);
    assert!(!unsafe { s::lv_timer_get_paused(timer) });
    call(&mut service, "init", json!([{}]));
    let screen = call(&mut service, "create", json!(["screen", 0, {}]))[0]
        .as_u64()
        .unwrap();
    call(
        &mut service,
        "set_style",
        json!([screen,{"pad":0,"border_width":0}]),
    );
    call(&mut service, "load", json!([screen]));
    // An RPC failure does not unlock a partial tree; explicit recovery does.
    call(&mut service, "clean", json!([screen]));
    assert!(service
        .execute(UiCommand {
            op: "set_size".into(),
            args: "[999999,3,3]".into(),
            bytes: vec![],
            root: "/data/input-probe".into()
        })
        .is_err());
    assert!(unsafe { s::lv_timer_get_paused(timer) });
    button(&mut service, screen);
    call(&mut service, "frame_end", json!([]));
    assert!(!unsafe { s::lv_timer_get_paused(timer) });
    // Preserve an already paused pointer and an already disabled pointer.
    let paused = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        let t = s::lv_indev_get_read_timer(p);
        s::lv_timer_pause(t);
        p
    };
    let disabled = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        s::lv_indev_set_read_cb(p, Some(disabled));
        s::lv_indev_enable(p, false);
        p
    };
    let other = Display::register::<_, { 80 * 8 }>(80, 80, |_| {}).unwrap();
    let owner_display = unsafe { s::lv_indev_get_display(indev) };
    let other_display = unsafe {
        let mut p = s::lv_display_get_next(core::ptr::null_mut());
        while p == owner_display {
            p = s::lv_display_get_next(p);
        }
        assert!(!p.is_null() && p != owner_display);
        s::lv_display_set_default(owner_display);
        p
    };
    let other_pointer = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        s::lv_indev_set_display(p, other_display);
        p
    };
    let keypad = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_KEYPAD);
        p
    };
    call(&mut service, "clean", json!([screen]));
    assert!(unsafe { s::lv_timer_get_paused(s::lv_indev_get_read_timer(paused)) });
    assert!(!unsafe { s::lv_timer_get_paused(s::lv_indev_get_read_timer(other_pointer)) });
    assert!(!unsafe { s::lv_timer_get_paused(s::lv_indev_get_read_timer(keypad)) });
    // Deleting and replacing a device must not restore a reused address.
    let doomed = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        p
    };
    call(&mut service, "frame_end", json!([]));
    call(&mut service, "clean", json!([screen]));
    unsafe {
        s::lv_indev_delete(doomed);
    }
    let replacement = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        s::lv_timer_pause(s::lv_indev_get_read_timer(p));
        p
    };
    service.release();
    assert_eq!(unsafe { s::lv_screen_active() }, original);
    assert!(!unsafe { s::lv_timer_get_paused(timer) });
    assert!(unsafe { s::lv_timer_get_paused(s::lv_indev_get_read_timer(paused)) });
    assert!(unsafe { s::lv_timer_get_paused(s::lv_indev_get_read_timer(replacement)) });
    unsafe {
        s::lv_indev_read(disabled);
    }
    assert_eq!(DISABLED_READS.load(Ordering::Relaxed), 0);
    println!(
        "PASS pointer gate error/deinit/stop release, original paused/disabled state, other display, keypad and deleted/replaced device reused_address={}",
        doomed == replacement
    );
    unsafe {
        for p in [indev, paused, disabled, other_pointer, replacement, keypad] {
            s::lv_indev_delete(p);
        }
    }
    drop(service);
    drop(other);
    drop(display);
    unsafe {
        lvgl::deinit();
    }
}
