//! Shared native batching: real LVGL hierarchy, rollback, callback retirement and publication.
use app_host::ui::{UiCommand, BATCH_BYTES_MAX, BATCH_OBJECTS_MAX};
use os::apps::native_ui::{NativeUiService, RedrawBoundary};
use rs_lvgl::{sys as s, Display};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    ffi::CStr,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

static INPUT_READS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn input(_: *mut s::lv_indev_t, data: *mut s::lv_indev_data_t) {
    INPUT_READS.fetch_add(1, Ordering::Relaxed);
    unsafe { (*data).state = s::lv_indev_state_t_LV_INDEV_STATE_RELEASED };
}

fn execute(ui: &mut NativeUiService, op: &str, args: Value) -> Result<Value, String> {
    let reply = ui.execute(UiCommand {
        op: op.into(),
        args: args.to_string(),
        bytes: vec![],
        root: "/data/ui-batch".into(),
    })?;
    Ok(serde_json::from_str(&reply.json).unwrap())
}
fn call(ui: &mut NativeUiService, op: &str, args: Value) -> Value {
    execute(ui, op, args).unwrap()["values"].clone()
}
fn tick() {
    rs_lvgl::tick_inc(Duration::from_millis(20));
    unsafe {
        s::lv_timer_handler();
    }
}

#[test]
fn bounded_batches_create_update_rollback_and_retire_on_the_existing_ui_owner() {
    rs_lvgl::init();
    let flushes = Rc::new(Cell::new(0));
    let count = flushes.clone();
    let display =
        Display::register::<_, { 80 * 8 }>(80, 80, move |_| count.set(count.get() + 1)).unwrap();
    let pointer = unsafe {
        let pointer = s::lv_indev_create();
        s::lv_indev_set_type(pointer, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        s::lv_indev_set_read_cb(pointer, Some(input));
        pointer
    };
    let previous = unsafe { s::lv_screen_active() };
    let mut ui = NativeUiService::new(80, 80);
    call(&mut ui, "init", json!([{}]));
    let screen = call(&mut ui, "create", json!(["screen", 0, {}]))[0]
        .as_u64()
        .unwrap();
    call(&mut ui, "load", json!([screen]));
    tick();
    let before = flushes.get();
    let handles = call(
        &mut ui,
        "batch",
        json!([[
            ["create",["button",screen,{"w":60,"h":60,"pad":0}]],
            ["on",[{"local":0},"clicked",7]],
            ["create",["label",{"local":0},{"text":"温度 21°C","x":2,"y":3}]],
        ]]),
    );
    let button = handles[0].as_u64().unwrap();
    let label = handles[1].as_u64().unwrap();
    assert_eq!(ui.object_count(), 3);
    let refresh = unsafe { s::lv_display_get_refr_timer(s::lv_display_get_default()) };
    assert!(unsafe { s::lv_timer_get_paused(refresh) });
    tick();
    assert_eq!(
        flushes.get(),
        before,
        "deferred batch must not publish its tree"
    );
    unsafe {
        let parent = s::lv_obj_get_child(s::lv_screen_active(), 0);
        let child = s::lv_obj_get_child(parent, 0);
        assert_eq!(
            CStr::from_ptr(s::lv_label_get_text(child))
                .to_str()
                .unwrap(),
            "温度 21°C"
        );
        s::lv_obj_send_event(parent, s::lv_event_code_t_LV_EVENT_CLICKED, ptr::null_mut());
    }
    assert_eq!(call(&mut ui, "events", json!([])), json!([7]));
    call(
        &mut ui,
        "batch_commit",
        json!([[["set_text", [label, "ready"]], ["set_pos", [button, 4, 5]],]]),
    );
    assert!(!unsafe { s::lv_timer_get_paused(refresh) });
    tick();
    assert!(flushes.get() > before);
    assert_eq!(call(&mut ui, "get_pos", json!([button])), json!([4, 5]));
    let reads = INPUT_READS.load(Ordering::Relaxed);
    call(
        &mut ui,
        "batch_commit",
        json!([[["set_text", [label, "live update"]]]]),
    );
    assert_eq!(
        INPUT_READS.load(Ordering::Relaxed),
        reads,
        "an ordinary batch must not reset pointer input or consume its samples"
    );

    let failed = execute(
        &mut ui,
        "batch_commit",
        json!([[
            ["set_text",[label,"prefix applied"]],
            ["create",["container",screen,{}]],
            ["on",[{"local":0},"clicked",9]],
            ["create",["button",{"local":0},{}]],
            ["create",["label",{"local":1},{"text":"invalid","bg_color":"invalid color"}]],
        ]]),
    )
    .unwrap_err();
    assert!(failed.contains("command 4"));
    assert_eq!(
        ui.object_count(),
        3,
        "all new ancestors and descendants must roll back"
    );
    let reply = execute(&mut ui, "events", json!([])).unwrap();
    assert_eq!(reply["retired"], json!([9]));
    assert_eq!(reply["values"], json!([]));
    for id in 4..=6 {
        assert_eq!(call(&mut ui, "is_valid", json!([id])), json!([false]));
    }
    unsafe {
        let child = s::lv_obj_get_child(s::lv_obj_get_child(s::lv_screen_active(), 0), 0);
        assert_eq!(
            CStr::from_ptr(s::lv_label_get_text(child))
                .to_str()
                .unwrap(),
            "prefix applied"
        );
    }
    let next = call(&mut ui, "create", json!(["label", screen, {}]))[0]
        .as_u64()
        .unwrap();
    assert!(next > 6, "rolled-back handles must never be reused");
    call(&mut ui, "delete", json!([next]));
    let too_many: Vec<_> = (0..BATCH_OBJECTS_MAX + 1)
        .map(|_| json!(["create", ["label", screen, {}]]))
        .collect();
    assert!(execute(&mut ui, "batch_commit", json!([too_many])).is_err());
    assert_eq!(ui.object_count(), 3);
    let oversize = "x".repeat(BATCH_BYTES_MAX);
    assert!(execute(
        &mut ui,
        "batch_commit",
        json!([[["set_text", [label, oversize]]]])
    )
    .unwrap_err()
    .contains("byte limit"));

    call(&mut ui, "clean", json!([screen]));
    assert!(unsafe { s::lv_timer_get_paused(refresh) });
    assert!(execute(
        &mut ui,
        "batch_commit",
        json!([[
            ["create",["button",screen,{}]],
            ["create",["label",{"local":0},{"text":"bad\0text"}]],
        ]])
    )
    .is_err());
    assert_eq!(ui.object_count(), 1);
    assert!(
        unsafe { s::lv_timer_get_paused(refresh) },
        "failure must preserve an existing redraw barrier"
    );
    let mut redraw = RedrawBoundary::default();
    redraw.completed("batch", true);
    let commit = redraw.before_sleep("/data/ui-batch").unwrap();
    ui.execute(commit).unwrap();
    assert!(!unsafe { s::lv_timer_get_paused(refresh) });
    redraw.completed("batch", true);
    redraw.completed("batch_commit", true);
    assert!(redraw.before_sleep("/data/ui-batch").is_none());

    for index in 0..24 {
        let handle = call(
            &mut ui,
            "batch_commit",
            json!([[
                ["create",["button",screen,{}]],
                ["on",[{"local":0},"clicked",100+index]],
            ]]),
        )[0]
        .as_u64()
        .unwrap();
        unsafe {
            s::lv_obj_send_event(
                s::lv_obj_get_child(s::lv_screen_active(), 0),
                s::lv_event_code_t_LV_EVENT_CLICKED,
                ptr::null_mut(),
            );
        }
        let removed = execute(&mut ui, "delete", json!([handle])).unwrap();
        assert_eq!(removed["retired"], json!([100 + index]));
        assert_eq!(
            call(&mut ui, "events", json!([])),
            json!([]),
            "deleted callbacks must leave the event queue"
        );
        assert_eq!(ui.object_count(), 1);
    }
    ui.release();
    assert_eq!(ui.object_count(), 0);
    assert_eq!(ui.pixel_bytes(), 0);
    assert_eq!(unsafe { s::lv_screen_active() }, previous);
    drop(ui);
    unsafe { s::lv_indev_delete(pointer) };
    unsafe { display.delete() };
}
