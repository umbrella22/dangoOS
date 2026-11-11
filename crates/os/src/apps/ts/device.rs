//! Foreground bytecode runner with bounded jobs and copied service/UI requests.
use super::graphics::Publication;
use super::host::{self, AppHost, Platform};
use super::scheduling::{RunProfile, TurnBudget, Wait};
use crate::shell::model::LuaApp;
use alloc::{
    boxed::Box,
    collections::VecDeque,
    string::{String, ToString},
    vec::Vec,
};
use app_host::{
    ui::{UiCommand, UiReply},
    RawAllocator,
};
use core::{
    alloc::{GlobalAlloc, Layout},
    cell::RefCell,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
    task::Poll,
};
use critical_section::Mutex;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use ts_engine::{CallState, HostError, HostReply, Request, RequestId};
use ts_engine::{Limits, Package, Vm};

unsafe fn alloc(layout: Layout) -> *mut u8 {
    unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) }
}
unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    unsafe { esp_alloc::HEAP.dealloc(ptr, layout) }
}
unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    unsafe { esp_alloc::HEAP.realloc_region(ptr, layout, new_size) }
}
pub(crate) static PSRAM: RawAllocator = RawAllocator {
    alloc,
    dealloc,
    realloc: Some(realloc),
};
#[no_mangle]
extern "C" fn embedts_platform_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}
static START: Signal<CriticalSectionRawMutex, LuaApp> = Signal::new();
static STOP: AtomicBool = AtomicBool::new(false);
static MANAGEMENT: AtomicBool = AtomicBool::new(false);
static STATE: AtomicU8 = AtomicU8::new(0);
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub fn start(app: LuaApp) {
    if !MANAGEMENT.load(Ordering::Acquire) {
        STOP.store(false, Ordering::Release);
        START.signal(app);
        WAKE.signal(());
    }
}
pub fn stop() {
    STOP.store(true, Ordering::Release);
    START.reset();
    critical_section::with(|cs| FRAME.borrow_ref_mut(cs).take());
    crate::services::feedback::lights::lua_release();
    LVGL_RELEASE.store(true, Ordering::Release);
    LVGL_READY.signal(());
    WAKE.signal(());
}
pub fn active_or_pending() -> bool {
    STATE.load(Ordering::Acquire) == 1 || START.signaled()
}
pub fn take_ended() -> bool {
    STATE
        .compare_exchange(2, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}
pub async fn quiesce() {
    MANAGEMENT.store(true, Ordering::Release);
    stop();
    while STATE.load(Ordering::Acquire) == 1 {
        Timer::after_millis(1).await;
    }
}
pub fn release_management() {
    MANAGEMENT.store(false, Ordering::Release);
}

// The publication crosses the channel with its immutable pixel allocation;
// the UI borrows blit views without allocating descriptor lists per frame.
static FRAME: Mutex<RefCell<Option<(RequestId, Publication)>>> = Mutex::new(RefCell::new(None));
static FRAME_TAKEN: Mutex<RefCell<Option<RequestId>>> = Mutex::new(RefCell::new(None));
static FRAME_DONE: Signal<CriticalSectionRawMutex, RequestId> = Signal::new();
static SCREEN_OPEN: AtomicBool = AtomicBool::new(false);
static POINTER_X: AtomicI32 = AtomicI32::new(0);
static POINTER_Y: AtomicI32 = AtomicI32::new(0);
static POINTER_DOWN: AtomicBool = AtomicBool::new(false);
pub fn take_frame() -> Option<Publication> {
    critical_section::with(|cs| {
        FRAME.borrow_ref_mut(cs).take().map(|(id, blits)| {
            *FRAME_TAKEN.borrow_ref_mut(cs) = Some(id);
            blits
        })
    })
}
pub fn frame_done() {
    if let Some(id) = critical_section::with(|cs| FRAME_TAKEN.borrow_ref_mut(cs).take()) {
        FRAME_DONE.signal(id);
        WAKE.signal(());
    }
}
pub fn screen_open() -> bool {
    SCREEN_OPEN.load(Ordering::Acquire)
}
pub fn set_pointer(x: i32, y: i32, down: bool) {
    POINTER_X.store(x, Ordering::Relaxed);
    POINTER_Y.store(y, Ordering::Relaxed);
    POINTER_DOWN.store(down, Ordering::Release);
}
fn publish_frame(id: RequestId, frame: Publication) {
    FRAME_DONE.reset();
    critical_section::with(|cs| *FRAME.borrow_ref_mut(cs) = Some((id, frame)));
}

static LVGL_REQUEST: Mutex<RefCell<Option<(RequestId, UiCommand)>>> =
    Mutex::new(RefCell::new(None));
static LVGL_REPLY: Signal<CriticalSectionRawMutex, (RequestId, Result<UiReply, String>)> =
    Signal::new();
static LVGL_TAKEN: Mutex<RefCell<Option<RequestId>>> = Mutex::new(RefCell::new(None));
static LVGL_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static LVGL_RELEASE: AtomicBool = AtomicBool::new(false);
pub fn take_lvgl_release() -> bool {
    LVGL_RELEASE.swap(false, Ordering::AcqRel)
}
pub fn lvgl_cleanup_pending() -> bool {
    LVGL_RELEASE.load(Ordering::Acquire) || STOP.load(Ordering::Acquire)
}
pub fn take_lvgl_command() -> Option<UiCommand> {
    critical_section::with(|cs| {
        let request = LVGL_REQUEST.borrow_ref_mut(cs).take();
        if STOP.load(Ordering::Acquire) || START.signaled() || MANAGEMENT.load(Ordering::Acquire) {
            return None;
        }
        request.map(|(id, command)| {
            *LVGL_TAKEN.borrow_ref_mut(cs) = Some(id);
            command
        })
    })
}
pub fn complete_lvgl(reply: Result<UiReply, String>) {
    if let Some(id) = critical_section::with(|cs| LVGL_TAKEN.borrow_ref_mut(cs).take()) {
        LVGL_REPLY.signal((id, reply));
        WAKE.signal(());
    }
}
pub async fn wait_lvgl_ready() {
    LVGL_READY.wait().await;
}
fn publish_lvgl(id: RequestId, command: UiCommand) {
    LVGL_REPLY.reset();
    critical_section::with(|cs| *LVGL_REQUEST.borrow_ref_mut(cs) = Some((id, command)));
    LVGL_READY.signal(());
}
struct DevicePlatform {
    id: String,
}
impl Drop for DevicePlatform {
    fn drop(&mut self) {
        SCREEN_OPEN.store(false, Ordering::Release);
        critical_section::with(|cs| FRAME.borrow_ref_mut(cs).take());
        crate::services::feedback::lights::lua_release();
        critical_section::with(|cs| LVGL_REQUEST.borrow_ref_mut(cs).take());
        LVGL_RELEASE.store(true, Ordering::Release);
        LVGL_READY.signal(());
    }
}
fn controls_blocked() -> bool {
    STOP.load(Ordering::Acquire)
        || MANAGEMENT.load(Ordering::Acquire)
        || START.signaled()
        || crate::services::update::boot::device::active()
        || crate::services::update::task::busy()
        || crate::services::power::button_task::shutdown_requested()
}
impl Platform for DevicePlatform {
    fn print(&mut self, text: &str, warning: bool) {
        if warning {
            log::warn!("ts[{}]: {text}", self.id);
        } else {
            log::info!("ts[{}]> {text}", self.id);
        }
    }
    fn now_ms(&self) -> u64 {
        embedts_platform_us() / 1000
    }
    fn services(&mut self) -> app_host::ServiceSnapshot {
        crate::apps::host_device::service_snapshot(controls_blocked())
    }
    fn hardware(&mut self) -> app_host::HardwareSnapshot {
        crate::apps::host_device::hardware_snapshot()
    }
    fn audio_control(&mut self, r: app_host::AudioControl) -> Result<(), i32> {
        crate::apps::host_device::audio_control(r, controls_blocked())
    }
    fn hardware_request(&mut self, r: app_host::HardwareRequest) -> Result<(), i32> {
        if controls_blocked() {
            return Err(app_host::esp_err::INVALID_STATE);
        }
        crate::apps::host_device::hardware_request(r)
    }
    fn display_info(&mut self) -> Result<app_host::DisplayInfo, i32> {
        if controls_blocked() {
            return Err(app_host::esp_err::INVALID_STATE);
        }
        Ok(app_host::DisplayInfo {
            width: crate::services::display::geometry::WIDTH as u16,
            height: crate::services::display::geometry::HEIGHT as u16,
            touch_available: true,
        })
    }
    fn display_open(&mut self, open: bool) {
        SCREEN_OPEN.store(open, Ordering::Release);
    }
    fn touch(&self) -> Vec<app_host::TouchPoint> {
        if POINTER_DOWN.load(Ordering::Acquire) {
            alloc::vec![app_host::TouchPoint {
                id: 0,
                x: POINTER_X.load(Ordering::Relaxed),
                y: POINTER_Y.load(Ordering::Relaxed)
            }]
        } else {
            Vec::new()
        }
    }
    fn heap_info(&mut self, caps: u32) -> app_host::HeapInfo {
        crate::platform::runtime::heap::snapshot(caps)
    }
    fn heap_probe(&mut self, caps: u32, layout: Layout) -> Result<bool, i32> {
        crate::apps::host_device::heap_probe(caps, layout)
    }
}
type StorageFuture = Pin<Box<dyn Future<Output = Result<HostReply, HostError>>>>;
async fn ready<T>(mut future: Pin<&mut dyn Future<Output = T>>) -> Option<T> {
    core::future::poll_fn(|cx| {
        Poll::Ready(match future.as_mut().poll(cx) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        })
    })
    .await
}
fn host_error(code: i32, message: impl Into<String>) -> HostError {
    HostError::new(code, message)
}
#[embassy_executor::task]
pub async fn app_task() {
    loop {
        let app = START.wait().await;
        if MANAGEMENT.load(Ordering::Acquire) {
            continue;
        }
        STATE.store(1, Ordering::Release);
        let result = run(&app).await;
        if let Err(error) = result {
            log::warn!("ts[{}]: {error}", app.id);
        }
        // run returns only after the VM and all its allocations are destroyed.
        STATE.store(
            if STOP.load(Ordering::Acquire) || START.signaled() {
                0
            } else {
                2
            },
            Ordering::Release,
        );
    }
}
async fn run(app: &LuaApp) -> Result<(), String> {
    if !app.entry.ends_with(".etb") {
        return Err("TS runtime requires an .etb package".into());
    }
    let id = app.id.clone();
    let entry = app.entry.clone();
    let root = alloc::format!("/data/appdata/{}", app.id);
    let data_root = root.clone();
    let bytes = crate::platform::runtime::executors::services::run(move || {
        ::fs::global::with(|vfs| {
            crate::apps::lifecycle::uninstall::ensure_ready(vfs, &id)
                .map_err(|_| ::fs::Errno::BUSY)?;
            vfs.create_dir_all(&data_root)?;
            let meta = vfs.stat(&entry)?;
            // Per-app package policy; package format itself has no 256 KiB rule.
            if meta.kind != ::fs::Kind::File || meta.size > 256 * 1024 {
                return Err(::fs::Errno::INVAL);
            }
            vfs.read_file(&entry)
        })
    })
    .await
    .map_err(|error| alloc::format!("package read: {error}"))?;
    if STOP.load(Ordering::Acquire) || START.signaled() {
        return Ok(());
    }
    let package = Package::parse(&bytes).map_err(|error| error.to_string())?;
    let limits = Limits {
        memory_bytes: 4 * 1024 * 1024,
        // Bytecode initialization and a complete software-rendered TS frame
        // exceed the host's 4 ms default on S31. Interrupts still terminate a
        // runaway call; awaits provide the normal scheduling boundaries.
        execution_us: 20_000,
        interrupt_polls: 200,
        clock: Some(crate::platform::runtime::resident_budget::clock_us),
        ..Default::default()
    };
    let host = AppHost::new(
        DevicePlatform { id: app.id.clone() },
        &app.args_json,
        root.clone(),
    )
    .map_err(|e| e.message)?
    .with_limits(limits);
    let graphics = host.graphics.clone();
    let mut vm = Vm::new(Box::new(host), limits, &host::allocator(&PSRAM))
        .map_err(|error| error.to_string())?;
    // Packages enter through the controlled development app-install channel.
    let loading = embedts_platform_us();
    {
        let _performance = crate::platform::runtime::frequency::device::performance();
        unsafe { vm.load(package) }.map_err(|error| alloc::format!("load: {error}"))?;
    }
    log::info!(
        "ts[{}]: loaded in {}us",
        app.id,
        embedts_platform_us() - loading
    );
    drop(bytes);
    let mut budget = crate::platform::runtime::frequency::device::Budget::new(8_000);
    let started = embedts_platform_us();
    let initial = budget
        .run(true, || vm.start("main", None))
        .map_err(|e| alloc::format!("main: {e}"))?;
    let mut run_profile = RunProfile {
        entry_us: embedts_platform_us().saturating_sub(started),
        ..Default::default()
    };
    if initial == CallState::Missing {
        return Err("TS app has no main export".into());
    }
    let mut entry_pending = initial == CallState::Pending;
    let mut main_finished = !entry_pending;
    let mut ticking = None;
    let mut next_tick = 0;
    let mut workers = ts_engine::workers::Group::new(limits, &host::allocator(&PSRAM));
    let mut queue = VecDeque::new();
    let mut timers: Vec<(RequestId, u64)> = Vec::new();
    let mut ui: Option<(RequestId, String)> = None;
    let mut storage: Option<(RequestId, String, StorageFuture)> = None;
    let mut redraw = crate::apps::native_ui::RedrawBoundary::default();
    let mut turn_budget = TurnBudget::default();
    log::info!("ts[{}]: started", app.id);
    // Stop is observed between bounded VM operations. Services IO finishes before
    // this instance is destroyed or a later application starts.
    let outcome: Result<(), String> = async {
        while !STOP.load(Ordering::Acquire) && !START.signaled() {
            run_profile.turns = run_profile.turns.saturating_add(1);
            let mut work_started = embedts_platform_us();
            let vm_ready = vm.has_pending_jobs() || workers.has_ready_work(work_started);
            // One frequency measurement/vote for this synchronous work window.
            // No performance request survives a following timer or UI await.
            budget.run(vm_ready, || -> Result<(), String> {
                pump_workers(&mut workers, &mut vm)?;
                vm.drain_jobs(32).map_err(|e| e.to_string())?;
                if entry_pending {
                    match vm.entry_status().map_err(|e| e.to_string())? {
                        CallState::Pending => {}
                        _ => {
                            entry_pending = false;
                            main_finished = true;
                        }
                    }
                }
                Ok(())
            })?;
            while let Some(request) = vm.take_request() {
                queue.push_back(request);
            }
            let now = embedts_platform_us() / 1000;
            let mut i = 0;
            while i < timers.len() {
                if embedts_platform_us() >= timers[i].1 {
                    let (id, _) = timers.swap_remove(i);
                    vm.complete_void(id).map_err(|e| e.to_string())?;
                } else {
                    i += 1;
                }
            }
            if let Some((id, op)) = ui.as_ref() {
                if let Some((reply_id, reply)) = LVGL_REPLY.try_take() {
                    if reply_id != *id {
                        continue;
                    }
                    redraw.completed(op, reply.is_ok());
                    if op == "init" && reply.is_err() {
                        graphics.borrow_mut().ui = false;
                    }
                    if op == "deinit" && reply.is_ok() {
                        graphics.borrow_mut().ui = false;
                    }
                    let reply = reply
                        .map(|r| HostReply {
                            json: r.json,
                            bytes: r.bytes,
                        })
                        .map_err(|e| host_error(app_host::esp_err::FAIL, e));
                    vm.complete(*id, reply).map_err(|e| e.to_string())?;
                    ui = None;
                }
            }
            if let Some((id, operation, future)) = storage.as_mut() {
                if let Some(reply) = ready(future.as_mut()).await {
                    let reply = graphics.borrow_mut().file_reply(operation, reply);
                    vm.complete(*id, reply).map_err(|e| e.to_string())?;
                    storage = None;
                }
            }
            for _ in 0..queue.len() {
                if STOP.load(Ordering::Acquire) || START.signaled() {
                    break;
                }
                let request = queue.pop_front().unwrap();
                let delay = match &request {
                    Request::Delay { id, micros } => Some((*id, Ok(*micros))),
                    Request::Host(request)
                        if request.operation == "sleep" || request.operation == "sleep_us" =>
                    {
                        Some((
                            request.id,
                            host::sleep_ms(request).and_then(|value| {
                                if request.operation == "sleep_us" {
                                    if value > 1_000_000 {
                                        Err(host_error(
                                            app_host::esp_err::INVALID_ARG,
                                            "delayUs must be 0..1000000",
                                        ))
                                    } else {
                                        Ok(u64::from(value))
                                    }
                                } else {
                                    Ok(u64::from(value) * 1000)
                                }
                            }),
                        ))
                    }
                    _ => None,
                };
                if let Some((id, delay)) = delay {
                    // A tree rebuilt through sequential RPCs commits before its
                    // first delay, with the same UI stop barrier as Lua.
                    if ui.is_none() {
                        if let Some(command) = redraw.before_sleep(&root) {
                            let waiting = embedts_platform_us();
                            run_profile.work_us = run_profile
                                .work_us
                                .saturating_add(waiting.saturating_sub(work_started));
                            publish_lvgl(id, command);
                            // Internal publication has no JS waiter; finish it
                            // before scheduling this sleep or another UI request.
                            while !STOP.load(Ordering::Acquire) && !START.signaled() {
                                if let Some((reply_id, _)) = LVGL_REPLY.try_take() {
                                    if reply_id == id {
                                        break;
                                    }
                                }
                                pump_workers(&mut workers, &mut vm)?;
                                embassy_futures::select::select(
                                    WAKE.wait(),
                                    Timer::after_millis(1),
                                )
                                .await;
                            }
                            turn_budget.reset();
                            budget.finish();
                            work_started = embedts_platform_us();
                            run_profile.barrier_us = run_profile
                                .barrier_us
                                .saturating_add(work_started.saturating_sub(waiting));
                        }
                    }
                    match delay {
                        Ok(0) => vm.complete_void(id).map_err(|e| e.to_string())?,
                        Ok(us) => timers.push((id, embedts_platform_us().saturating_add(us))),
                        Err(e) => vm.complete(id, Err(e)).map_err(|e| e.to_string())?,
                    }
                    continue;
                }
                let Request::Host(request) = request else {
                    unreachable!()
                };
                let id = request.id;
                if request.operation.starts_with("thread.") {
                    workers.command(request, embedts_platform_us() / 1000);
                } else if request.operation == "display.present" {
                    let publication = graphics.borrow_mut().present(&request.args);
                    match publication {
                        Err(e) => vm.complete(id, Err(e)).map_err(|e| e.to_string())?,
                        Ok(frame) => {
                            let changed = !frame.rectangles.is_empty();
                            if changed {
                                let waiting = embedts_platform_us();
                                run_profile.work_us = run_profile
                                    .work_us
                                    .saturating_add(waiting.saturating_sub(work_started));
                                publish_frame(id, frame);
                                while !STOP.load(Ordering::Acquire) && !START.signaled() {
                                    if FRAME_DONE.try_take() == Some(id) {
                                        break;
                                    }
                                    pump_workers(&mut workers, &mut vm)?;
                                    embassy_futures::select::select(
                                        WAKE.wait(),
                                        Timer::after_millis(1),
                                    )
                                    .await;
                                }
                                turn_budget.reset();
                                budget.finish();
                                work_started = embedts_platform_us();
                                run_profile.barrier_us = run_profile
                                    .barrier_us
                                    .saturating_add(work_started.saturating_sub(waiting));
                            }
                            graphics.borrow_mut().presented();
                            vm.complete(
                                id,
                                Ok(HostReply::json(if changed { "true" } else { "false" })),
                            )
                            .map_err(|e| e.to_string())?;
                        }
                    }
                } else if request.operation.starts_with("ui.") {
                    if ui.is_some() {
                        queue.push_back(Request::Host(request));
                        continue;
                    }
                    if graphics.borrow().screen.is_some() {
                        vm.complete(
                            id,
                            Err(host_error(
                                app_host::esp_err::INVALID_STATE,
                                "framebuffer already owns screen",
                            )),
                        )
                        .map_err(|e| e.to_string())?;
                        continue;
                    }
                    if request.operation == "ui.init" {
                        graphics.borrow_mut().ui = true;
                    }

                    match host::ui_command(request, &root) {
                        Ok(command) => {
                            let op = command.op.clone();
                            publish_lvgl(id, command);
                            ui = Some((id, op));
                        }
                        Err(e) => vm.complete(id, Err(e)).map_err(|e| e.to_string())?,
                    }
                } else if request.operation.starts_with("storage.")
                    || host::asset_io(&request.operation)
                {
                    if storage.is_some() {
                        queue.push_back(Request::Host(request));
                        continue;
                    }
                    let operation = request.operation.clone();
                    let mut request = request;
                    if let Err(e) = graphics.borrow_mut().file_request(&mut request) {
                        vm.complete(id, Err(e)).map_err(|e| e.to_string())?;
                        continue;
                    }
                    let storage_root = root.clone();
                    storage = Some((
                        id,
                        operation,
                        Box::pin(async move {
                            crate::platform::runtime::executors::services::run(move || {
                                if STOP.load(Ordering::Acquire) || START.signaled() {
                                    return Err(host_error(
                                        app_host::esp_err::INVALID_STATE,
                                        "application stopped",
                                    ));
                                }
                                ::fs::global::with(|vfs| {
                                    Ok(host::storage(
                                        vfs,
                                        &storage_root,
                                        &request,
                                        limits.host_bytes,
                                    ))
                                })
                                .map_err(|e| {
                                    host_error(e.0, alloc::format!("storage unavailable: {e}"))
                                })?
                            })
                            .await
                        }),
                    ));
                } else {
                    vm.complete(
                        id,
                        Err(host_error(
                            app_host::esp_err::NOT_SUPPORTED,
                            "unsupported async host operation",
                        )),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            if main_finished && !entry_pending && now >= next_tick && ticking != Some(false) {
                let state = budget
                    .run(true, || vm.start("onTick", Some(now as u32 as i32)))
                    .map_err(|e| e.to_string())?;
                ticking = Some(state != CallState::Missing);
                entry_pending = state == CallState::Pending;
                next_tick = now.saturating_add(20);
            }
            let now_us = embedts_platform_us();
            let worked = now_us.saturating_sub(work_started);
            run_profile.work_us = run_profile.work_us.saturating_add(worked);
            turn_budget.record(worked);
            if main_finished
                && !entry_pending
                && ticking == Some(false)
                && !vm.has_pending_jobs()
                && vm.pending_requests() == 0
            {
                break;
            }
            let runnable = vm.has_pending_jobs()
                || workers.has_ready_work(now_us)
                || queue.iter().any(|request| {
                    let Request::Host(request) = request else {
                        return true;
                    };
                    if request.operation.starts_with("ui.") {
                        ui.is_none()
                    } else if request.operation.starts_with("storage.")
                        || host::asset_io(&request.operation)
                    {
                        storage.is_none()
                    } else {
                        true
                    }
                });
            let deadline = timers.iter().map(|(_, end)| *end).min();
            let waiting = embedts_platform_us();
            match turn_budget.next_wait(runnable, deadline, now_us) {
                Wait::Continue => {}
                Wait::Cooperate => {
                    embassy_futures::yield_now().await;
                    run_profile.cooperate_us = run_profile
                        .cooperate_us
                        .saturating_add(embedts_platform_us().saturating_sub(waiting));
                }
                Wait::BusyPause => {
                    Timer::after_millis(1).await;
                    run_profile.pause_us = run_profile
                        .pause_us
                        .saturating_add(embedts_platform_us().saturating_sub(waiting));
                }
                Wait::Idle(micros) => {
                    budget.finish();
                    embassy_futures::select::select(WAKE.wait(), Timer::after_micros(micros)).await;
                    run_profile.idle_us = run_profile
                        .idle_us
                        .saturating_add(embedts_platform_us().saturating_sub(waiting));
                }
            }
        }
        Ok(())
    }
    .await;
    // Completion closures retain their data on the services executor. Wait even
    // after a JS fault so old writes cannot race uninstall or the next instance.
    if let Some((_, _, future)) = storage.take() {
        let _ = future.await;
    }
    critical_section::with(|cs| LVGL_REQUEST.borrow_ref_mut(cs).take());
    drop(workers);
    let profile = vm.profile();
    let stats = vm.close();
    drop(graphics);
    log::info!(
        "ts[{}]: closed used={} peak={} allocations={} failures={}",
        app.id,
        stats.used,
        stats.peak,
        stats.allocations,
        stats.failures
    );
    log::info!(
        "ts[{}]: reallocs={} moved={} copied={}",
        app.id,
        stats.reallocations,
        stats.realloc_moved,
        stats.realloc_copied_bytes
    );
    log::info!(
        "ts[{}]: calls={} clocks={} draws={} requests={} delays={} jobs={}",
        app.id,
        profile.calls,
        profile.clocks,
        profile.draws,
        profile.requests,
        profile.delays,
        profile.jobs
    );
    log::info!(
        "ts[{}]: dispatch_us={} enqueue_us={} complete_us={} jobs_us={}",
        app.id,
        profile.dispatch_us,
        profile.enqueue_us,
        profile.complete_us,
        profile.jobs_us
    );
    log::info!(
        "ts[{}]: turns={} entry_us={} work_us={}",
        app.id,
        run_profile.turns,
        run_profile.entry_us,
        run_profile.work_us
    );
    log::info!(
        "ts[{}]: cooperate_us={} pause_us={} idle_us={} barrier_us={}",
        app.id,
        run_profile.cooperate_us,
        run_profile.pause_us,
        run_profile.idle_us,
        run_profile.barrier_us
    );
    outcome?;
    if stats.used != 0 {
        return Err("engine allocation ledger did not return to zero".into());
    }
    Ok(())
}

fn pump_workers(group: &mut ts_engine::workers::Group, vm: &mut Vm) -> Result<(), String> {
    group.poll_us(embedts_platform_us());
    while let Some((id, reply)) = group.take_completion() {
        vm.complete(id, Ok(reply)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn selftest() -> bool {
    let Ok(package) = Package::parse(include_bytes!(
        "../../../../../resources/embedts/selftest.etb"
    )) else {
        return false;
    };
    let Ok(mut vm) = Vm::new(
        Box::new(()),
        Limits {
            clock: Some(crate::platform::runtime::resident_budget::clock_us),
            execution_us: 100_000,
            ..Default::default()
        },
        &host::allocator(&PSRAM),
    ) else {
        return false;
    };
    // This trusted package is exported by the matching external EmbedTS builder.
    let ok = unsafe { vm.load(package) }.is_ok() && matches!(vm.call("main", None), Ok(Some(42)));
    ok && vm.close().used == 0
}
