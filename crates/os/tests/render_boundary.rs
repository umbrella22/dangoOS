//! Native refresh regression: pump LVGL after every UI RPC, as the device does.
//! The unmodified Album builds a new view through multiple such requests.
use lua_engine::ui::{UiCommand, UiReply};
use lua_engine::{Host, Limits, Request, Step, Vm};
use os::apps::lua::native_ui::LuaLvglService;
use rs_lvgl::{sys as s, Display};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
static INPUT_READS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn input(_: *mut s::lv_indev_t, d: *mut s::lv_indev_data_t) {
    unsafe {
        INPUT_READS.fetch_add(1, Ordering::Relaxed);
        (*d).state = s::lv_indev_state_t_LV_INDEV_STATE_RELEASED;
    }
}
fn tick() {
    rs_lvgl::tick_inc(Duration::from_millis(20));
    unsafe {
        s::lv_timer_handler();
    }
}
fn execute(service: &mut LuaLvglService, op: &str, args: Value) -> Result<UiReply, String> {
    service.execute(UiCommand {
        op: op.into(),
        args: args.to_string(),
        bytes: vec![],
        root: "/data/appdata/render-probe".into(),
    })
}
fn call(service: &mut LuaLvglService, op: &str, args: Value) -> Value {
    serde_json::from_str::<Value>(&execute(service, op, args).unwrap().json).unwrap()["values"]
        .clone()
}
fn solid(service: &mut LuaLvglService, screen: u64, color: &str) {
    call(
        service,
        "create",
        json!(["container",screen,{"x":0,"y":0,"w":80,"h":80,"bg_color":color,"bg_opa":255,"border_width":0,"radius":0}]),
    );
}
struct Console(Vec<String>);
impl Host for Console {
    fn print(&mut self, b: &[u8]) {
        self.0.push(String::from_utf8_lossy(b).into());
    }
}
#[test]
fn render_boundary() {
    unsafe {
        rs_lvgl::deinit();
    }
    rs_lvgl::init();
    let frames = Rc::new(RefCell::new((0usize, vec![0u8; 80 * 80 * 3])));
    let copy = frames.clone();
    let display = Display::register::<_, { 80 * 8 }>(80, 80, move |r| {
        let mut f = copy.borrow_mut();
        f.0 += 1;
        let w = (r.area.x2 - r.area.x1 + 1) as usize;
        for y in 0..=(r.area.y2 - r.area.y1) as usize {
            for x in 0..w {
                let c = r.colors[y * w + x];
                let at = ((r.area.y1 as usize + y) * 80 + r.area.x1 as usize + x) * 3;
                f.1[at..at + 3].copy_from_slice(&[c.r(), c.g(), c.b()]);
            }
        }
    })
    .unwrap();
    let timer = unsafe { s::lv_display_get_refr_timer(s::lv_display_get_default()) };
    unsafe {
        s::lv_timer_set_period(timer, 1);
    }
    let indev = unsafe {
        let p = s::lv_indev_create();
        s::lv_indev_set_type(p, s::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        s::lv_indev_set_read_cb(p, Some(input));
        p
    };
    let mut service = LuaLvglService::new(80, 80);
    call(&mut service, "init", json!([{}]));
    let screen = call(&mut service, "create", json!(["screen", 0, {}]))[0]
        .as_u64()
        .unwrap();
    call(
        &mut service,
        "set_style",
        json!([screen,{"bg_color":"#000000","bg_opa":255,"pad":0}]),
    );
    call(&mut service, "load", json!([screen]));
    solid(&mut service, screen, "#ff0000");
    call(&mut service, "frame_end", json!([]));
    tick();
    let center = |f: &(usize, Vec<u8>)| f.1[(40 * 80 + 40) * 3..(40 * 80 + 40) * 3 + 3].to_vec();
    assert!(center(&frames.borrow())[0] > 200);
    // Previously queued damage belongs to the completed old tree. No newly
    // invalidated clean/rebuild state may overwrite it while refresh is held.
    unsafe {
        s::lv_obj_invalidate(s::lv_screen_active());
    }
    let old_flushes = frames.borrow().0;
    call(&mut service, "clean", json!([screen]));
    assert!(unsafe { s::lv_timer_get_paused(timer) });
    let reads = INPUT_READS.load(Ordering::Relaxed);
    for _ in 0..5 {
        tick();
    }
    assert_eq!(frames.borrow().0, old_flushes);
    assert_eq!(
        INPUT_READS.load(Ordering::Relaxed),
        reads,
        "native pointer polling must wait for a complete object tree"
    );
    assert!(center(&frames.borrow())[0] > 200);
    // A native RPC error must not publish a half-built tree before pcall can
    // recover and the explicit boundary is reached.
    assert!(execute(&mut service, "set_size", json!([999999, 3, 3])).is_err());
    tick();
    assert_eq!(frames.borrow().0, old_flushes);
    solid(&mut service, screen, "#00ff00");
    tick();
    assert_eq!(frames.borrow().0, old_flushes);
    let aligned = call(
        &mut service,
        "create",
        json!(["container",screen,{"w":20,"h":10,"align":"bottom_right","pad":0,"border_width":0}]),
    )[0]
    .as_u64()
    .unwrap();
    assert_eq!(
        call(&mut service, "get_pos", json!([aligned])),
        json!([60, 70])
    );
    tick();
    assert_eq!(
        frames.borrow().0,
        old_flushes,
        "layout reads must not publish the rebuilding tree"
    );
    let row = call(
        &mut service,
        "create",
        json!(["container",screen,{"w":60,"h":20,"pad":0,"border_width":0}]),
    )[0]
    .as_u64()
    .unwrap();
    call(&mut service, "set_flex", json!([row,{"flow":"row"}]));
    let first = call(
        &mut service,
        "create",
        json!(["container",row,{"w":10,"h":10,"pad":0,"border_width":0}]),
    )[0]
    .as_u64()
    .unwrap();
    assert_eq!(call(&mut service, "get_pos", json!([first])), json!([0, 0]));
    tick();
    assert_eq!(frames.borrow().0, old_flushes);
    call(&mut service, "frame_end", json!([]));
    tick();
    assert!(center(&frames.borrow())[1] > 200);

    call(&mut service, "clean", json!([screen]));
    service.release();
    call(&mut service, "frame_end", json!([]));
    tick();
    println!(
        "PASS native per-RPC rendering: old frame retained, pointer held, error recovery, commit, idle refresh and release"
    );
    // Exercise the actual Lua callback wrapper, including pcall(error) and a
    // process_events(0) boundary. Timer handling between each RPC exposes any
    // intermediate blank that a many-RPC-per-host-tick harness would conceal.
    let source=br#"local l=require('lvgl');local delay=require('delay');l.init();local screen=l.create_screen();screen:set_style({bg_color='#000000',bg_opa=255,pad=0});screen:load();local b=l.button(screen,{x=0,y=0,w=80,h=80,bg_color='#ff0000',bg_opa=255,border_width=0,radius=0});b:on('clicked',function()screen:clean();l.container(screen,{x=0,y=0,w=80,h=80,bg_color='#00ff00',bg_opa=255,border_width=0,radius=0});error('expected callback error')end);l.process_events(0);delay.delay_ms(1);l.process_events(0);delay.delay_ms(1);l.deinit()"#;
    let mut vm = Vm::new(
        Box::new(Console(vec![])),
        Limits::default(),
        &lua_engine::GLOBAL_ALLOCATOR,
        1,
    )
    .unwrap();
    vm.load("callback-boundary", source).unwrap();
    let mut sleeps = 0;
    let mut held_flushes = None;
    loop {
        match vm.resume() {
            Step::Slice => {}
            Step::Request(Request::Host(lua_engine::ui::REQUEST)) => {
                let cmd = vm.take_ui_command().unwrap();
                let op = cmd.op.clone();
                let reply = service.execute(cmd);
                vm.complete_ui(reply);
                if op == "clean" {
                    held_flushes = Some(frames.borrow().0);
                }
                tick();
                if let Some(count) = held_flushes {
                    if op == "frame_end" {
                        held_flushes = None;
                    } else {
                        assert_eq!(frames.borrow().0, count, "intermediate tree published");
                    }
                }
            }
            Step::Request(Request::Sleep { .. }) => {
                sleeps += 1;
                if sleeps == 1 {
                    unsafe {
                        let b = s::lv_obj_get_child(s::lv_screen_active(), 0);
                        s::lv_obj_send_event(
                            b,
                            s::lv_event_code_t_LV_EVENT_CLICKED,
                            ptr::null_mut(),
                        );
                    }
                } else {
                    assert!(center(&frames.borrow())[1] > 200);
                }
            }
            Step::Done => break,
            other => panic!("callback: {other:?}"),
        }
    }
    assert_eq!(sleeps, 2);
    assert!(vm
        .host_mut::<Console>()
        .unwrap()
        .0
        .iter()
        .any(|x| x.contains("expected callback error")));
    drop(vm);
    service.release();
    let source=br#"local l=require('lvgl');local delay=require('delay');l.init();local screen=l.create_screen();screen:load();local b=l.button(screen,{w=80,h=80});b:on('clicked',function()l.deinit()end);l.process_events(0);delay.delay_ms(1);l.process_events(0);print('DEINIT_CALLBACK_DONE')"#;
    let mut vm = Vm::new(
        Box::new(Console(vec![])),
        Limits::default(),
        &lua_engine::GLOBAL_ALLOCATOR,
        2,
    )
    .unwrap();
    vm.load("deinit-in-callback", source).unwrap();
    loop {
        match vm.resume() {
            Step::Slice => {}
            Step::Request(Request::Host(lua_engine::ui::REQUEST)) => {
                let cmd = vm.take_ui_command().unwrap();
                vm.complete_ui(service.execute(cmd));
                tick();
            }
            Step::Request(Request::Sleep { .. }) => unsafe {
                let b = s::lv_obj_get_child(s::lv_screen_active(), 0);
                s::lv_obj_send_event(b, s::lv_event_code_t_LV_EVENT_CLICKED, ptr::null_mut());
            },
            Step::Done => break,
            other => panic!("deinit callback: {other:?}"),
        }
    }
    assert!(vm
        .host_mut::<Console>()
        .unwrap()
        .0
        .iter()
        .any(|x| x == "DEINIT_CALLBACK_DONE"));
    assert!(!service.active());
    drop(vm);
    println!("PASS callback deinit and late idempotent commit");
    // Use the same boundary state and synthetic command as the device Sleep
    // branch. This script never calls process_events after its clean.
    let source = br#"local l=require('lvgl');local d=require('delay');l.init();local s=l.create_screen();s:load();l.container(s,{w=80,h=80,bg_color='#ff0000',bg_opa=255});d.delay_ms(1);s:clean();l.container(s,{w=80,h=80,bg_color='#0000ff',bg_opa=255,border_width=0,radius=0});d.delay_ms(1);l.deinit()"#;
    let mut vm = Vm::new(
        Box::new(Console(vec![])),
        Limits::default(),
        &lua_engine::GLOBAL_ALLOCATOR,
        3,
    )
    .unwrap();
    vm.load("sleep-boundary", source).unwrap();
    let mut boundary = os::apps::lua::native_ui::RedrawBoundary::default();
    let mut sleeps = 0;
    let mut held = None;
    loop {
        match vm.resume() {
            Step::Slice => {}
            Step::Request(Request::Host(lua_engine::ui::REQUEST)) => {
                let cmd = vm.take_ui_command().unwrap();
                let op = cmd.op.clone();
                let reply = service.execute(cmd);
                boundary.completed(&op, reply.is_ok());
                vm.complete_ui(reply);
                if op == "clean" {
                    held = Some(frames.borrow().0);
                }
                tick();
                if let Some(count) = held {
                    if op != "deinit" {
                        assert_eq!(frames.borrow().0, count);
                    }
                }
            }
            Step::Request(Request::Sleep { .. }) => {
                sleeps += 1;
                if let Some(cmd) = boundary.before_sleep("/data/appdata/render-probe") {
                    assert_eq!(sleeps, 2);
                    service.execute(cmd).unwrap();
                    held = None;
                    tick();
                    assert!(center(&frames.borrow())[2] > 200);
                }
                assert!(boundary.before_sleep("unused").is_none());
            }
            Step::Done => break,
            other => panic!("sleep boundary: {other:?}"),
        }
    }
    assert_eq!(sleeps, 2);
    drop(vm);
    println!(
        "PASS delay-only script: shared device Sleep boundary publishes complete blue frame once"
    );
    // Flood the same bounded RPC policy used in the UI's idle interval. The
    // normal LVGL/input tick runs between windows, even with work still queued.
    call(&mut service, "init", json!([{}]));
    let screen = call(&mut service, "create", json!(["screen", 0, {}]))[0]
        .as_u64()
        .unwrap();
    call(&mut service, "load", json!([screen]));
    solid(&mut service, screen, "#ff0000");
    call(&mut service, "frame_end", json!([]));
    tick();
    call(&mut service, "clean", json!([screen]));
    solid(&mut service, screen, "#0000ff");
    let held_flushes = frames.borrow().0;
    let reads = INPUT_READS.load(Ordering::Relaxed);
    let mut pending = 100;
    let mut now_us = 0;
    let mut windows = 0;
    while pending > 0 {
        let deadline = now_us + 5_000;
        let mut window = os::apps::lua::native_ui::RpcWindow::until(deadline);
        let mut served = 0;
        while pending > 0 && window.available(now_us) {
            assert_eq!(
                call(&mut service, "get_size", json!([screen])),
                json!([80, 80])
            );
            pending -= 1;
            served += 1;
            window.completed();
            now_us += 100;
        }
        assert!(served <= 8);
        now_us = deadline;
        assert!(!window.available(now_us));
        tick();
        assert_eq!(frames.borrow().0, held_flushes);
        assert!(center(&frames.borrow())[0] > 200);
        windows += 1;
    }
    assert_eq!(windows, 13);
    assert_eq!(INPUT_READS.load(Ordering::Relaxed), reads);
    // A slow single request can overrun the window; it cannot authorize a
    // second request after the fixed deadline, or reset that deadline.
    let mut window = os::apps::lua::native_ui::RpcWindow::until(5_000);
    assert!(window.available(4_999));
    window.completed();
    assert!(!window.available(6_000));
    call(&mut service, "frame_end", json!([]));
    tick();
    assert!(center(&frames.borrow())[2] > 200);
    service.release();
    assert!(execute(&mut service, "get_size", json!([screen])).is_err());
    println!(
        "PASS fixed RPC windows: 100 requests across 13 windows, eight-request/deadline limits, incomplete-tree pointer held, complete frame retained and late request rejected after release"
    );
    unsafe {
        s::lv_indev_delete(indev);
    }
    drop(service);
    drop(display);
    unsafe {
        rs_lvgl::deinit();
    }
    println!("PASS actual Lua callback error and zero-timeout event boundary");
}
