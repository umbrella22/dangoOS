#![cfg(feature = "ts-engine")]
//! Precompiled EmbedTS -> real LVGL and LittleFS, with a simulated board.
use app_host::{
    AudioControl, AudioStatus, HardwareRequest, HardwareSnapshot, HeapInfo, NetworkStatus,
    ServiceSnapshot,
};
use os::apps::{
    native_ui::{NativeUiService, RedrawBoundary},
    ts::host::{self, AppHost, Platform},
};
use rs_lvgl::{sys as s, Display};
use std::{cell::RefCell, ptr, rc::Rc, time::Duration};
use ts_engine::{CallState, HostError, HostReply, Limits, Package, Vm, GLOBAL_ALLOCATOR};
#[derive(Default)]
struct BoardState {
    log: Vec<String>,
    audio: Vec<AudioControl>,
    hardware: Vec<HardwareRequest>,
}
struct Board(Rc<RefCell<BoardState>>);
impl Platform for Board {
    fn print(&mut self, text: &str, _: bool) {
        self.0.borrow_mut().log.push(text.into());
    }
    fn now_ms(&self) -> u64 {
        1000
    }
    fn services(&mut self) -> ServiceSnapshot {
        ServiceSnapshot {
            audio: Some(AudioStatus {
                available: true,
                controls_available: true,
                state: "idle",
                sample_rate_hz: 48000,
                volume: 40,
                output_ceiling_db: -10,
                muted: false,
                tx_frames: 0,
                rx_frames: 0,
                underruns: 0,
                overruns: 0,
                peak: 0,
                error: None,
            }),
            network: Some(NetworkStatus {
                wifi_state: "up",
                bluetooth_state: "off",
                ip: Some([192, 168, 1, 9]),
            }),
            ..Default::default()
        }
    }
    fn hardware(&mut self) -> HardwareSnapshot {
        let mut h = HardwareSnapshot {
            available: 63,
            valid: 127,
            accel: [-123, 0, 1000],
            ..Default::default()
        };
        h.onboard.ready = 1;
        h.onboard.edge(0, true, 1000);
        h
    }
    fn audio_control(&mut self, r: AudioControl) -> Result<(), i32> {
        self.0.borrow_mut().audio.push(r);
        Ok(())
    }
    fn hardware_request(&mut self, r: HardwareRequest) -> Result<(), i32> {
        self.0.borrow_mut().hardware.push(r);
        Ok(())
    }
    fn heap_info(&mut self, _: u32) -> HeapInfo {
        HeapInfo {
            valid_fields: app_host::heap_fields::SIZES,
            ..Default::default()
        }
    }
}
fn tick() {
    rs_lvgl::tick_inc(Duration::from_millis(20));
    unsafe {
        s::lv_timer_handler();
    }
}
#[test]
fn ts7_sdk_runs_ui_callbacks_storage_and_output_services_and_releases_resources() {
    let package = include_bytes!("../../../resources/embedts/fixtures/ts-app.etb");
    rs_lvgl::init();
    let flushes = Rc::new(RefCell::new(0));
    let observed = flushes.clone();
    let display =
        Display::register::<_, { 80 * 8 }>(80, 80, move |_| *observed.borrow_mut() += 1).unwrap();
    let previous = unsafe { s::lv_screen_active() };
    let mut service = NativeUiService::new(80, 80);
    let mut redraw = RedrawBoundary::default();
    let root = "/data/appdata/parity";
    let mut vfs = fs::Vfs::new();
    vfs.mount(
        "data",
        fs::Fs::mount_or_format(
            Box::new(fs::RamDevice::new(4096, 64)),
            fs::Geometry::flash(64),
        )
        .unwrap()
        .0,
    );
    vfs.create_dir_all(root).unwrap();
    let state = Rc::new(RefCell::new(BoardState::default()));
    let host = AppHost::new(Board(state.clone()), r#"{"probe":"yes"}"#, root.into()).unwrap();
    let mut vm = Vm::new(Box::new(host), Limits::default(), &GLOBAL_ALLOCATOR).unwrap();
    unsafe { vm.load(Package::parse(package).unwrap()) }.unwrap();
    assert_eq!(vm.start("main", None).unwrap(), CallState::Pending);
    let mut sleeps = 0;
    let mut done = false;
    let mut saw_redraw = false;
    for _ in 0..10_000 {
        vm.drain_jobs(32).unwrap();
        while let Some(queued) = vm.take_request() {
            let request = match queued {
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
            let reply = if request.operation.starts_with("ui.") {
                let command = host::ui_command(request, root).unwrap();
                let op = command.op.clone();
                let reply = service.execute(command);
                redraw.completed(&op, reply.is_ok());
                let timer = unsafe { s::lv_display_get_refr_timer(s::lv_display_get_default()) };
                if op == "clean" {
                    assert!(unsafe { s::lv_timer_get_paused(timer) });
                    saw_redraw = true;
                }
                reply
                    .map(|r| HostReply {
                        json: r.json,
                        bytes: r.bytes,
                    })
                    .map_err(|e| HostError::new(-1, e))
            } else if request.operation.starts_with("storage.") {
                host::storage(&mut vfs, root, &request, 2 * 1024 * 1024)
            } else if request.operation == "sleep" {
                assert_eq!(host::sleep_ms(&request).unwrap(), 1);
                if let Some(command) = redraw.before_sleep(root) {
                    service.execute(command).unwrap();
                }
                tick();
                sleeps += 1;
                if sleeps == 1 {
                    unsafe {
                        let button = s::lv_obj_get_child(s::lv_screen_active(), 0);
                        assert!(!button.is_null());
                        s::lv_obj_send_event(
                            button,
                            s::lv_event_code_t_LV_EVENT_CLICKED,
                            ptr::null_mut(),
                        );
                    }
                }
                Ok(HostReply::json("null"))
            } else {
                panic!("unsupported request: {}", request.operation)
            };
            vm.complete(id, reply).unwrap();
            tick();
        }
        match vm.entry_status().unwrap() {
            CallState::Done(42) => {
                done = true;
                break;
            }
            CallState::Pending => {}
            other => panic!("entry {other:?}"),
        }
    }
    assert!(done);
    assert!(saw_redraw);
    assert_eq!(sleeps, 2);
    assert_eq!(vm.pending_requests(), 0);
    let final_stats = vm.close();
    assert_eq!(final_stats.used, 0);
    assert!(final_stats.peak < 2 * 1024 * 1024);
    assert_eq!(service.object_count(), 0);
    assert_eq!(service.pixel_bytes(), 0);
    assert_eq!(unsafe { s::lv_screen_active() }, previous);
    assert!(*flushes.borrow() > 0);
    assert_eq!(vfs.read_file(&format!("{root}/renamed")).unwrap(), [0, 255]);
    let board = state.borrow();
    assert_eq!(
        board.audio,
        [AudioControl::Volume(0), AudioControl::Muted(true)]
    );
    assert_eq!(board.hardware.len(), 3);
    assert!(board
        .log
        .iter()
        .any(|s| s.contains("expected callback failure")));
    assert!(board.log.iter().any(|s| s == "parity:ok"));
    drop(board);
    service.release();
    drop(service);
    drop(display);
    unsafe {
        rs_lvgl::deinit();
    }
}
