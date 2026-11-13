#![cfg(feature = "ts-engine")]
use app_host::{DisplayInfo, TouchPoint};
use os::apps::{
    native_ui::NativeUiService,
    ts::host::{self, AppHost, Platform},
};
use rs_lvgl::{sys as s, Display};
use std::{cell::Cell, ffi::CStr, ptr, rc::Rc, time::Duration};
use ts_engine::{CallState, HostError, HostReply, Limits, Package, Vm, GLOBAL_ALLOCATOR};
unsafe extern "C" {
    static lv_label_class: s::lv_obj_class_t;
}
struct Board(Rc<Cell<u64>>);
impl Platform for Board {
    fn print(&mut self, text: &str, _: bool) {
        println!("TS {text}");
    }
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
    fn display_info(&mut self) -> Result<DisplayInfo, i32> {
        Ok(DisplayInfo {
            width: 480,
            height: 480,
            touch_available: true,
        })
    }
    fn touch(&self) -> Vec<TouchPoint> {
        Vec::new()
    }
}
unsafe fn label(p: *mut s::lv_obj_t, text: &str) -> Option<*mut s::lv_obj_t> {
    unsafe {
        if s::lv_obj_check_type(p, &lv_label_class)
            && CStr::from_ptr(s::lv_label_get_text(p)).to_bytes() == text.as_bytes()
        {
            return Some(p);
        }
        for i in 0..s::lv_obj_get_child_count(p) {
            if let Some(p) = label(s::lv_obj_get_child(p, i as i32), text) {
                return Some(p);
            }
        }
        None
    }
}
#[test]
fn official_games_render_and_photo_navigation_confirmed_delete_runs_on_real_lvgl_vfs() {
    let package = include_bytes!("../../../resources/embedts/fixtures/ts-official.etb");
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 8 }>(480, 480, |_| {}).unwrap();
    let previous = unsafe { s::lv_screen_active() };
    let mut ui = NativeUiService::new(480, 480);
    let root = "/data/appdata/parity";
    let mut vfs = fs::Vfs::new();
    vfs.mount(
        "data",
        fs::Fs::mount_or_format(
            Box::new(fs::RamDevice::new(4096, 128)),
            fs::Geometry::flash(128),
        )
        .unwrap()
        .0,
    );
    vfs.create_dir_all(&format!("{root}/photos/nested"))
        .unwrap();
    let jpeg = include_bytes!("../../runtime/app-host/test/images/red_blue_16x8.jpg");
    for file in ["a.jpg", "b.jpg", "nested/c.jpg"] {
        vfs.write_file(&format!("{root}/photos/{file}"), jpeg)
            .unwrap();
    }
    let now = Rc::new(Cell::new(1000));
    let limits = Limits {
        memory_bytes: 4 * 1024 * 1024,
        interrupt_polls: 1000,
        ..Default::default()
    };
    let host = AppHost::new(
        Board(now.clone()),
        r#"{"run_time_ms":1000,"run_ms":1000,"duration_ms":1000}"#,
        root.into(),
    )
    .unwrap()
    .with_limits(limits);
    let graphics = host.graphics.clone();
    let mut vm = Vm::new(Box::new(host), limits, &GLOBAL_ALLOCATOR).unwrap();
    unsafe { vm.load(Package::parse(package).unwrap()) }.unwrap();
    assert_eq!(vm.start("main", None).unwrap(), CallState::Pending);
    let (mut stage, mut frames, mut done) = (0, 0, false);
    for _ in 0..20000 {
        vm.drain_jobs(32).unwrap();
        while let Some(queued) = vm.take_request() {
            let mut request = match queued {
                ts_engine::Request::Delay { id, micros } => {
                    assert_eq!(micros % 1000, 0);
                    ts_engine::HostRequest {
                        id,
                        operation: "sleep".into(),
                        args: format!("[{}]", micros / 1000),
                        bytes: Vec::new(),
                    }
                }
                ts_engine::Request::Host(request) => request,
            };
            let id = request.id;
            let operation = request.operation.clone();
            let reply = if operation.starts_with("ui.") {
                ui.execute(host::ui_command(request, root).unwrap())
                    .map(|r| HostReply {
                        json: r.json,
                        bytes: r.bytes,
                    })
                    .map_err(|e| HostError::new(-1, e))
            } else if operation.starts_with("storage.") || host::asset_io(&operation) {
                graphics.borrow_mut().file_request(&mut request).unwrap();
                let result = host::storage(&mut vfs, root, &request, limits.host_bytes);
                graphics.borrow_mut().file_reply(&operation, result)
            } else if operation == "display.present" {
                let f = graphics.borrow_mut().present(&request.args).unwrap();
                assert_eq!(f.pixels.len(), 480 * 480 * 2);
                assert!(f.pixels.iter().any(|v| *v != 0));
                frames += 1;
                graphics.borrow_mut().presented();
                Ok(HostReply::json("true"))
            } else if operation == "sleep" {
                let ms = host::sleep_ms(&request).unwrap();
                now.set(now.get() + u64::from(ms));
                if ui.object_count() > 0 {
                    unsafe {
                        let screen = s::lv_screen_active();
                        let target = match stage {
                            0 => {
                                assert!(label(screen, "3 photos").is_some());
                                s::lv_obj_get_child(s::lv_obj_get_child(screen, 2), 0)
                            }
                            1 => label(screen, ">").unwrap(),
                            2 | 4 => label(screen, "Delete").unwrap(),
                            3 => label(screen, "Cancel").unwrap(),
                            5 => label(screen, "Delete").unwrap(),
                            6 => {
                                assert!(label(screen, "2 / 2").is_some());
                                s::lv_obj_get_child(screen, 0)
                            }
                            _ => panic!("unexpected photo stage"),
                        };
                        // Labels in button widgets forward a click to their parent. The message box
                        // footer is chosen by its latest Delete label after the confirmation opens.
                        let target = if stage == 5 {
                            fn last(p: *mut s::lv_obj_t) -> Option<*mut s::lv_obj_t> {
                                unsafe {
                                    let mut out = if s::lv_obj_check_type(p, &lv_label_class)
                                        && CStr::from_ptr(s::lv_label_get_text(p)).to_bytes()
                                            == b"Delete"
                                    {
                                        Some(p)
                                    } else {
                                        None
                                    };
                                    for i in 0..s::lv_obj_get_child_count(p) {
                                        if let Some(v) = last(s::lv_obj_get_child(p, i as i32)) {
                                            out = Some(v)
                                        }
                                    }
                                    out
                                }
                            }
                            last(screen).unwrap()
                        } else {
                            target
                        };
                        let target = if s::lv_obj_check_type(target, &lv_label_class) {
                            s::lv_obj_get_parent(target)
                        } else {
                            target
                        };
                        assert!(!target.is_null());
                        s::lv_obj_send_event(
                            target,
                            s::lv_event_code_t_LV_EVENT_CLICKED,
                            ptr::null_mut(),
                        );
                    }
                    stage += 1;
                }
                Ok(HostReply::json("null"))
            } else {
                panic!("unsupported {operation}")
            };
            vm.complete(id, reply).unwrap();
            rs_lvgl::tick_inc(Duration::from_millis(5));
            unsafe {
                s::lv_timer_handler();
            }
        }
        if vm.entry_status().unwrap() == CallState::Done(42) {
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(stage, 7);
    assert!(frames > 50);
    assert_eq!(vm.close().used, 0);
    assert!(graphics.borrow().screen.is_none());
    assert_eq!(ui.object_count(), 0);
    assert_eq!(ui.pixel_bytes(), 0);
    assert_eq!(unsafe { s::lv_screen_active() }, previous);
    drop(ui);
    drop(display);
    unsafe {
        rs_lvgl::deinit();
    }
}
