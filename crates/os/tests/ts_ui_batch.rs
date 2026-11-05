#![cfg(feature = "ts-engine")]
//! Precompiled bytecode/VM/LVGL integration and paired host measurements; no device access.
use os::apps::{
    native_ui::NativeUiService,
    ts_host::{self, AppHost, Platform},
};
use rs_lvgl::{sys as s, Display};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    ptr,
    rc::Rc,
    sync::Mutex,
    time::{Duration, Instant},
};
use ts_engine::{CallState, HostError, HostReply, Limits, Package, Request, Vm, GLOBAL_ALLOCATOR};

static UI_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct State {
    log: Vec<String>,
    phase: usize,
    mounted_at: Option<Instant>,
    updated_at: Option<Instant>,
}
struct Board(Rc<RefCell<State>>);
impl Platform for Board {
    fn print(&mut self, message: &str, _: bool) {
        let mut state = self.0.borrow_mut();
        if message == "ui-cost:mounted" {
            state.phase = 1;
            state.mounted_at = Some(Instant::now());
        }
        if message == "ui-cost:updated" {
            state.phase = 2;
            state.updated_at = Some(Instant::now());
        }
        state.log.push(message.into());
    }
    fn now_ms(&self) -> u64 {
        1000
    }
}
fn tick() {
    rs_lvgl::tick_inc(Duration::from_millis(5));
    unsafe {
        s::lv_timer_handler();
    }
}
fn package(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../artifacts/embedts/fixtures")
            .join(name),
    )
    .unwrap()
}
fn run(package: &[u8], mode: &str, stop_after_batch: bool, inject: bool) -> Value {
    rs_lvgl::init();
    let started = Instant::now();
    let pixels = Rc::new(RefCell::new(vec![0u8; 480 * 480 * 3]));
    let image = pixels.clone();
    let flushed_pixels = Rc::new(Cell::new(0u64));
    let flushed = flushed_pixels.clone();
    let display = Display::register::<_, { 480 * 8 }>(480, 480, move |region| {
        let mut pixels = image.borrow_mut();
        let width = (region.area.x2 - region.area.x1 + 1) as usize;
        flushed.set(flushed.get() + width as u64 * (region.area.y2 - region.area.y1 + 1) as u64);
        for y in 0..=(region.area.y2 - region.area.y1) as usize {
            for x in 0..width {
                let c = region.colors[y * width + x];
                let at = ((region.area.y1 as usize + y) * 480 + region.area.x1 as usize + x) * 3;
                pixels[at..at + 3].copy_from_slice(&[c.r(), c.g(), c.b()]);
            }
        }
    })
    .unwrap();
    let previous = unsafe { s::lv_screen_active() };
    let mut ui = NativeUiService::new(480, 480);
    let state = Rc::new(RefCell::new(State::default()));
    let limits = Limits {
        memory_bytes: 4 * 1024 * 1024,
        interrupt_polls: 5000,
        execution_us: 20_000,
        ..Default::default()
    };
    let host = AppHost::new(
        Board(state.clone()),
        &json!({"mode":mode}).to_string(),
        "/data/ui-batch".into(),
    )
    .unwrap()
    .with_limits(limits);
    let mut vm = Vm::new(Box::new(host), limits, &GLOBAL_ALLOCATOR).unwrap();
    unsafe { vm.load(Package::parse(package).unwrap()) }.unwrap();
    let begin = Instant::now();
    assert_eq!(vm.start("main", None).unwrap(), CallState::Pending);
    let mut requests = [0usize; 3];
    let mut native_us = [0u128; 3];
    let mut phases = [None, None];
    let mut peak_objects = 0;
    let mut sleeps = 0;
    let mut done = false;
    let mut stopped = false;
    let mut final_pixels = Vec::new();
    let mut mounted_stats = None;
    let mut updated_stats = None;
    let mut mounted_pixels = None;
    let mut updated_pixels = None;
    let mut mounted_rgb = Vec::new();
    for _ in 0..20_000 {
        vm.drain_jobs(32).unwrap();
        let phase = state.borrow().phase;
        if phase > 0 && phases[0].is_none() {
            phases[0] = Some(
                state
                    .borrow()
                    .mounted_at
                    .unwrap()
                    .duration_since(begin)
                    .as_micros(),
            );
            mounted_stats = Some(vm.memory_stats());
            mounted_pixels = Some(flushed_pixels.get());
            mounted_rgb = pixels.borrow().clone();
        }
        if phase > 1 && phases[1].is_none() {
            let state = state.borrow();
            phases[1] = Some(
                state
                    .updated_at
                    .unwrap()
                    .duration_since(state.mounted_at.unwrap())
                    .as_micros(),
            );
            updated_stats = Some(vm.memory_stats());
            updated_pixels = Some(flushed_pixels.get());
            final_pixels = pixels.borrow().clone();
        }
        while let Some(request) = vm.take_request() {
            let publish = matches!(&request, Request::Host(r) if matches!(r.operation.as_str(), "ui.frame_end" | "ui.batch_commit"));
            let (id, reply) = match request {
                Request::Host(request) if request.operation.starts_with("ui.") => {
                    let id = request.id;
                    let phase = state.borrow().phase;
                    requests[phase] += 1;
                    let is_batch =
                        request.operation == "ui.batch" || request.operation == "ui.batch_commit";
                    let command = ts_host::ui_command(request, "/data/ui-batch").unwrap();
                    let native_start = Instant::now();
                    let result = ui.execute(command);
                    native_us[phase] += native_start.elapsed().as_micros();
                    peak_objects = peak_objects.max(ui.object_count());
                    if is_batch && stop_after_batch {
                        assert!(result.is_ok());
                        stopped = true;
                        break;
                    }
                    (
                        id,
                        result
                            .map(|reply| HostReply {
                                json: reply.json,
                                bytes: reply.bytes,
                            })
                            .map_err(|e| HostError::new(-1, e)),
                    )
                }
                Request::Delay { id, .. } => {
                    sleeps += 1;
                    if inject && sleeps == 1 {
                        unsafe {
                            let root = s::lv_obj_get_child(s::lv_screen_active(), 0);
                            let button = s::lv_obj_get_child(root, 1);
                            assert!(!button.is_null());
                            s::lv_obj_send_event(
                                button,
                                s::lv_event_code_t_LV_EVENT_CLICKED,
                                ptr::null_mut(),
                            );
                        }
                    }
                    (id, Ok(HostReply::json("null")))
                }
                other => panic!("unexpected request {other:?}"),
            };
            vm.complete(id, reply).unwrap();
            if mode != "sequential" && mode != "batch" {
                tick();
            } else if publish {
                // Equal frame boundaries isolate request/VM costs. Functional
                // tests separately pump timers after every native operation.
                tick();
                unsafe { s::lv_refr_now(s::lv_display_get_default()) };
            }
        }
        if stopped {
            break;
        }
        if vm.entry_status().unwrap() == CallState::Done(42) {
            done = true;
            break;
        }
    }
    assert!(done || stopped, "entry failed to finish");
    if inject && !stopped {
        assert!(state.borrow().log.iter().any(|s| s == "ui-batch:ok"));
    }
    if !stopped {
        assert_eq!(vm.pending_requests(), 0);
    }
    ui.release();
    assert_eq!(ui.object_count(), 0);
    assert_eq!(ui.pixel_bytes(), 0);
    assert_eq!(unsafe { s::lv_screen_active() }, previous);
    let closed = vm.close();
    assert_eq!(closed.used, 0);
    assert_eq!(closed.failures, 0);
    use sha2::{Digest, Sha256};
    let result = json!({
        "mode":mode,"stopped_with_pending_reply":stopped,"status":"passed",
        "mount_requests":requests[0],"update_requests":requests[1],"cleanup_requests":requests[2],
        "mount_wall_us":phases[0],"updates_wall_us":phases[1],
        "native_mount_us":native_us[0],"native_updates_us":native_us[1],
        "mount_flushed_pixels":mounted_pixels,
        "updates_flushed_pixels":mounted_pixels.zip(updated_pixels).map(|(mount,update)|update-mount),
        "peak_tracked_objects":peak_objects,"mount_allocations":mounted_stats.map(|s|s.allocations),
        "update_allocations":mounted_stats.zip(updated_stats).map(|(mount,update)|update.allocations-mount.allocations),
        "vm_allocations":closed.allocations,"vm_peak_bytes":closed.peak,"vm_closed_used":closed.used,
        "native_final_objects":ui.object_count(),"native_final_pixels":ui.pixel_bytes(),
        "final_pixels_sha256":format!("{:x}",Sha256::digest(&final_pixels)),
        "mount_pixels_sha256":format!("{:x}",Sha256::digest(&mounted_rgb)),
        "package_sha256":format!("{:x}",Sha256::digest(package)),
        "harness_wall_us":started.elapsed().as_micros(),
    });
    drop(ui);
    // Display handles do not unregister LVGL displays on Drop. Remove this
    // display so the next pair renders on its own capture callback.
    unsafe { display.delete() };
    result
}

#[test]
fn sdk_tree_updates_callbacks_failures_and_repeated_disposal() {
    let _lock = UI_LOCK.lock().unwrap();
    let package = package("ts-ui-batch.etb");
    run(&package, "functional", false, true);
    run(&package, "stop-before-reply", true, false);
}

#[test]
fn paired_page_preserves_pixels_and_reduces_requests_with_memory_and_time_receipts() {
    let _lock = UI_LOCK.lock().unwrap();
    let package = package("ts-ui-cost.etb");
    let mut results = Vec::new();
    // The first pair warms the native allocator/font path; alternate measured order.
    run(&package, "sequential", false, false);
    run(&package, "batch", false, false);
    for round in 0..5 {
        let order = if round % 2 == 0 {
            ["sequential", "batch"]
        } else {
            ["batch", "sequential"]
        };
        for mode in order {
            let result = run(&package, mode, false, false);
            assert_eq!(
                result["mount_requests"],
                json!(if mode == "batch" { 6 } else { 29 })
            );
            assert_eq!(
                result["update_requests"],
                json!(if mode == "batch" { 32 } else { 288 })
            );
            assert_eq!(result["peak_tracked_objects"], json!(17));
            assert!(result["updates_flushed_pixels"].as_u64().unwrap() > 0);
            assert_ne!(result["mount_pixels_sha256"], result["final_pixels_sha256"]);
            results.push(result);
        }
    }
    let first = results[0]["final_pixels_sha256"].clone();
    let mount = results[0]["mount_pixels_sha256"].clone();
    assert!(
        results.iter().all(|r| r["final_pixels_sha256"] == first),
        "both paths must render identical pixels"
    );
    assert!(results.iter().all(|r| r["mount_pixels_sha256"] == mount));
    let receipt = json!({"status":"passed","device_executed":false,"measurement":"host application wall time; real VM and LVGL; refresh at matching explicit frame boundaries; no USB or RTOS scheduling", "samples":results});
    if let Ok(output) = std::env::var("DANGO_UI_BENCH_OUT") {
        std::fs::write(output, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    println!("UI_BATCH_COMPARISON {}", receipt);
}
