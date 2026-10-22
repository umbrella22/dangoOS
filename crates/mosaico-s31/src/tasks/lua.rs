//! Lua app runner (RFC 0006 section 5, stage L1).
//!
//! Runs one bundled Lua app at a time, started and stopped by the UI task
//! when its page opens and closes ([`start`], [`stop`]). The script runs in
//! short time slices so the UI keeps its frame rate (see
//! `docs/plans/lua-runtime-plan.md` section 4).
//!
//! # Screen
//!
//! `display.open()` gives the script the 480x480 screen. `present()` hands
//! over the dirty rectangles of the script's framebuffer ([`Host::display_blit`]
//! records them) and suspends the script with `Request::Present`. The UI task
//! copies those rectangles into the LVGL port's shadow ([`take_frame`]), uploads
//! them through the panel plane like any LVGL damage, and signals back; only
//! then does the script run again, so its framebuffer is never read while it
//! draws. Edge swipes still go to the shell; other touches reach
//! `screen:touch()` through [`set_pointer`].

use alloc::{boxed::Box, string::String, vec::Vec};
use core::alloc::{GlobalAlloc, Layout};
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

use critical_section::Mutex;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Instant, Timer};
use log::{info, warn};
use mosaico_lua::{
    caps, esp_err, Blit, DisplayInfo, HeapInfo, Host, Limits, RawAllocator, Request, Step,
    TouchPoint, Vm,
};
use mosaico_s31::hub::registry::SHARED_LUA_DIRS;
use mosaico_s31::hub::{LuaApp, LuaSource};

use crate::drivers::geometry::{HEIGHT, WIDTH};

/// Lua heap on PSRAM.
unsafe fn psram_alloc(layout: Layout) -> *mut u8 {
    unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) }
}
unsafe fn psram_dealloc(ptr: *mut u8, layout: Layout) {
    unsafe { esp_alloc::HEAP.dealloc(ptr, layout) }
}
pub(crate) static PSRAM: RawAllocator = RawAllocator {
    alloc: psram_alloc,
    dealloc: psram_dealloc,
};

pub(super) fn clock_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

/// Per-app limits: 4 MiB of PSRAM; 4 ms time slices checked every 200 VM
/// instructions; a script stuck 2 s where it cannot be yielded is stopped.
pub const LIMITS: Limits = Limits {
    memory_bytes: 4 * 1024 * 1024,
    hook_interval: 200,
    slice_hooks: 8,
    max_blocked_hooks: 20_000,
    clock: Some(clock_us),
    slice_us: 4_000,
    max_blocked_us: 2_000_000,
};

/// After this much Lua time without a sleep, the task sleeps 1 ms so timer
/// driven tasks (sensors) are not starved by back-to-back slices.
const BUSY_BUDGET_US: u64 = 16_000;

// ---- UI <-> Lua task -------------------------------------------------------

/// One dirty rectangle of the script's framebuffer (exclusive bounds).
#[derive(Clone, Copy)]
pub struct LuaBlit {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
    /// First pixel of the rectangle; rows are `stride` bytes apart. Valid
    /// until the frame is released with [`frame_done`].
    pub pixels: *const u8,
    pub stride: usize,
}

// SAFETY: the pointers refer to the script's framebuffer, which is neither
// written nor freed while the script waits for `FRAME_DONE`.
unsafe impl Send for LuaBlit {}

static START: Signal<CriticalSectionRawMutex, LuaApp> = Signal::new();
static MANAGEMENT: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
static FRAME: Mutex<RefCell<Option<Vec<LuaBlit>>>> = Mutex::new(RefCell::new(None));
static FRAME_DONE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// 0 idle, 1 running, 2 ended on its own (the UI leaves the page).
static STATE: AtomicU8 = AtomicU8::new(0);
static SCREEN_OPEN: AtomicBool = AtomicBool::new(false);
static POINTER_X: AtomicI32 = AtomicI32::new(0);
static POINTER_Y: AtomicI32 = AtomicI32::new(0);
static POINTER_DOWN: AtomicBool = AtomicBool::new(false);

/// Run `app` (the UI opened its page). Stops a running app first.
pub fn start(app: LuaApp) {
    if MANAGEMENT.load(Ordering::Acquire) {
        return;
    }
    STOP.store(false, Ordering::Release);
    START.signal(app);
}

/// Stop the running app (its page closed). The script cannot catch this.
pub fn stop() {
    super::lights::lua_release();
    STOP.store(true, Ordering::Release);
    START.reset();
    // A frame nobody will show must not keep the script waiting.
    release_frame();
}

/// True once after the app ended by itself (finished or failed).
pub fn take_ended() -> bool {
    STATE
        .compare_exchange(2, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// True while the script holds the screen: LVGL must not draw over it.
pub fn screen_open() -> bool {
    SCREEN_OPEN.load(Ordering::Acquire)
}

/// Touch for `screen:touch()` (panel coordinates).
pub fn set_pointer(x: i32, y: i32, pressed: bool) {
    POINTER_X.store(x, Ordering::Relaxed);
    POINTER_Y.store(y, Ordering::Relaxed);
    POINTER_DOWN.store(pressed, Ordering::Release);
}

/// The frame the script presented, if one is waiting. The caller copies the
/// rectangles, then must call [`frame_done`].
pub fn take_frame() -> Option<Vec<LuaBlit>> {
    critical_section::with(|cs| FRAME.borrow_ref_mut(cs).take())
}

/// The presented frame was copied; the script may draw again.
pub fn frame_done() {
    FRAME_DONE.signal(());
}

fn release_frame() {
    if critical_section::with(|cs| FRAME.borrow_ref_mut(cs).take()).is_some() {
        FRAME_DONE.signal(());
    }
}

// LVGL requests cross to the sole native UI owner. Stop releases waiters and
// marks cleanup before any later app can acquire the service.
static LVGL_REQUEST: Mutex<RefCell<Option<mosaico_lua::ui::UiCommand>>> =
    Mutex::new(RefCell::new(None));
static LVGL_REPLY: Signal<CriticalSectionRawMutex, Result<mosaico_lua::ui::UiReply, String>> =
    Signal::new();
static LVGL_RELEASE: AtomicBool = AtomicBool::new(false);
pub fn take_lvgl_command() -> Option<mosaico_lua::ui::UiCommand> {
    critical_section::with(|cs| LVGL_REQUEST.borrow_ref_mut(cs).take())
}
pub fn complete_lvgl(reply: Result<mosaico_lua::ui::UiReply, String>) {
    LVGL_REPLY.signal(reply);
}
pub fn take_lvgl_release() -> bool {
    LVGL_RELEASE.swap(false, Ordering::AcqRel)
}

// ---- host ------------------------------------------------------------------

struct DeviceHost {
    id: String,
    screen: bool,
    blits: Vec<LuaBlit>,
}

impl Drop for DeviceHost {
    fn drop(&mut self) {
        super::lights::lua_release();
        LVGL_RELEASE.store(true, Ordering::Release);
        critical_section::with(|cs| LVGL_REQUEST.borrow_ref_mut(cs).take());
    }
}

impl Host for DeviceHost {
    fn hardware_snapshot(&mut self) -> mosaico_lua::HardwareSnapshot {
        let mut snapshot = super::sensors::lua_snapshot();
        snapshot.onboard = super::buttons::snapshot();
        snapshot.led_generation = super::lights::generation();
        if super::haptic::available() {
            snapshot.available |= 32;
        } else {
            snapshot.available &= !32;
        }
        if super::lights::available() {
            snapshot.available |= 16;
        } else {
            snapshot.available &= !16;
        }
        snapshot
    }
    fn hardware_request(&mut self, request: mosaico_lua::HardwareRequest) -> Result<(), i32> {
        use mosaico_lua::{HapticPreset, HardwareRequest};
        use mosaico_s31::behavior::affect::HapticPattern;
        match request {
            HardwareRequest::Haptic(preset) => {
                if !super::haptic::available() {
                    return Err(esp_err::NOT_SUPPORTED);
                }
                if !super::haptic::enabled() {
                    return Err(esp_err::INVALID_STATE);
                }
                let pattern = match preset {
                    HapticPreset::Panic => HapticPattern::Panic,
                    HapticPreset::Angry => HapticPattern::Angry,
                    HapticPreset::Surprised => HapticPattern::Surprised,
                    HapticPreset::Puzzled => HapticPattern::Puzzled,
                    HapticPreset::Happy => HapticPattern::Happy,
                    HapticPreset::Satisfied => HapticPattern::Satisfied,
                    HapticPreset::Confused => HapticPattern::Confused,
                    HapticPreset::Listening => HapticPattern::Listening,
                    HapticPreset::Thinking => HapticPattern::Thinking,
                    HapticPreset::Error => HapticPattern::Error,
                    HapticPreset::ModuleInsert => HapticPattern::ModuleInsert,
                    HapticPreset::ModuleRemove => HapticPattern::ModuleRemove,
                    HapticPreset::MagneticContact => HapticPattern::MagneticContact,
                };
                if super::haptic::play(pattern) {
                    Ok(())
                } else {
                    Err(esp_err::INVALID_STATE)
                }
            }
            HardwareRequest::LedFrame(frame) => super::lights::lua_frame(frame),
            HardwareRequest::LedRelease => {
                super::lights::lua_release();
                Ok(())
            }
        }
    }

    fn print(&mut self, line: &[u8]) {
        info!("lua[{}]> {}", self.id, String::from_utf8_lossy(line));
    }

    fn log(&mut self, line: &[u8]) {
        warn!("lua[{}]: {}", self.id, String::from_utf8_lossy(line));
    }

    fn display_open(&mut self) -> Result<DisplayInfo, i32> {
        if self.screen {
            return Err(esp_err::INVALID_STATE);
        }
        self.screen = true;
        SCREEN_OPEN.store(true, Ordering::Release);
        Ok(self.display_info().unwrap())
    }

    fn display_info(&self) -> Option<DisplayInfo> {
        self.screen.then_some(DisplayInfo {
            width: WIDTH as u16,
            height: HEIGHT as u16,
            touch_available: true,
        })
    }

    fn display_close(&mut self) {
        self.screen = false;
        self.blits.clear();
        SCREEN_OPEN.store(false, Ordering::Release);
    }

    fn display_blit(&mut self, blit: &Blit) -> Result<(), i32> {
        self.blits.push(LuaBlit {
            x0: blit.x0,
            y0: blit.y0,
            x1: blit.x1,
            y1: blit.y1,
            pixels: blit.pixels.as_ptr(),
            stride: blit.stride,
        });
        Ok(())
    }

    fn touch(&mut self, out: &mut [TouchPoint]) -> Result<usize, i32> {
        if !POINTER_DOWN.load(Ordering::Acquire) {
            return Ok(0);
        }
        out[0] = TouchPoint {
            id: 0,
            x: POINTER_X.load(Ordering::Relaxed),
            y: POINTER_Y.load(Ordering::Relaxed),
        };
        Ok(1)
    }

    fn heap_info(&mut self, caps_mask: u32) -> HeapInfo {
        let stats = esp_alloc::HEAP.stats();
        let mut info = HeapInfo::default();
        for region in stats.region_stats.iter().flatten() {
            let external = region
                .capabilities
                .contains(esp_alloc::MemoryCapability::External);
            let wanted = match caps_mask {
                c if c & caps::SPIRAM != 0 => external,
                c if c & caps::INTERNAL != 0 => !external,
                _ => true,
            };
            if wanted {
                info.total_size += region.size as u32;
                info.free_size += region.free as u32;
                info.allocated_size += region.used as u32;
                info.largest_free_block = info.largest_free_block.max(region.free as u32);
                info.total_blocks += 1;
            }
        }
        info.minimum_free_size = info.free_size;
        info
    }
}

fn log_sink(text: &[u8]) {
    let text = String::from_utf8_lossy(text);
    let text = text.trim_end();
    if !text.is_empty() {
        warn!("lua: {text}");
    }
}

#[embassy_executor::task]
pub async fn lua_task() {
    mosaico_lua::set_log_sink(log_sink);
    let mut next = None;
    loop {
        let app = match next.take() {
            Some(app) => app,
            None => START.wait().await,
        };
        if MANAGEMENT.load(Ordering::Acquire) {
            continue;
        }
        next = run(app).await;
    }
}

/// Run one app until it ends or is stopped. Returns the app to start next
/// when a start request arrived meanwhile.
async fn run(app: LuaApp) -> Option<LuaApp> {
    STATE.store(1, Ordering::Release);
    let ready =
        mosaico_fs::global::with(|vfs| Ok(mosaico_s31::hub::uninstall::ensure_ready(vfs, &app.id)));
    if !matches!(ready, Ok(Ok(()))) {
        warn!("lua[{}]: pending recovery or unavailable storage", app.id);
        STATE.store(2, Ordering::Release);
        return None;
    }
    let free_before = esp_alloc::HEAP.free();
    let seed = Instant::now().as_ticks() as u32;
    let host = DeviceHost {
        id: app.id.clone(),
        screen: false,
        blits: Vec::new(),
    };
    let mut vm = match Vm::new(Box::new(host), LIMITS, &PSRAM, seed) {
        Ok(vm) => vm,
        Err(e) => {
            warn!("lua[{}]: state creation failed: {e}", app.id);
            STATE.store(2, Ordering::Release);
            return None;
        }
    };
    let root = alloc::format!("/data/appdata/{}", app.id);
    let root_ready = mosaico_fs::global::with(|vfs| vfs.create_dir_all(&root));
    if let Err(e) = root_ready
        .map_err(|e| alloc::format!("{e}"))
        .and_then(|()| vm.set_storage_root(&root))
    {
        warn!("lua[{}]: data root: {e}", app.id);
        drop(vm);
        STATE.store(2, Ordering::Release);
        return None;
    }
    if let Err(e) = vm.set_args_json(&app.args_json) {
        warn!("lua[{}]: args: {e}", app.id);
    }
    let loaded = match &app.source {
        LuaSource::Builtin { chunk, source } => vm.load(chunk, source.as_bytes()),
        LuaSource::File(path) => vm.load_file(path, &SHARED_LUA_DIRS),
    };
    if let Err(e) = loaded {
        warn!("lua[{}]: {e}", app.id);
        drop(vm);
        STATE.store(2, Ordering::Release);
        return None;
    }
    info!("lua[{}]: started", app.id);

    let started = Instant::now();
    let mut slices = 0u32;
    let mut frames = 0u32;
    let mut longest_us = 0u64;
    let mut busy_us = 0u64;
    let mut stopping = false;
    let mut next = None;
    let outcome = loop {
        if !stopping && (STOP.load(Ordering::Acquire) || START.signaled()) {
            stopping = true;
            vm.interrupt();
        }
        let slice_start = Instant::now();
        let step = vm.resume();
        let slice_us = slice_start.elapsed().as_micros();
        longest_us = longest_us.max(slice_us);
        busy_us += slice_us;
        slices += 1;
        match step {
            Step::Request(Request::Host(mosaico_lua::ui::REQUEST)) => {
                if let Some(command) = vm.take_ui_command() {
                    LVGL_REPLY.reset();
                    critical_section::with(|cs| *LVGL_REQUEST.borrow_ref_mut(cs) = Some(command));
                    match select(LVGL_REPLY.wait(), poll_workers_until_stop(&mut vm)).await {
                        Either::First(reply) => vm.complete_ui(reply),
                        Either::Second(()) => {
                            critical_section::with(|cs| LVGL_REQUEST.borrow_ref_mut(cs).take());
                            vm.interrupt();
                            vm.complete_ui(Err("stopped".into()));
                        }
                    }
                }
            }
            Step::Request(Request::Sleep { ms }) => {
                busy_us = 0;
                // Wake early for a stop so leaving the page is immediate.
                let _ = select(
                    Timer::after_millis(ms as u64),
                    poll_workers_until_stop(&mut vm),
                )
                .await;
            }
            Step::Request(Request::Present) => {
                busy_us = 0;
                frames += 1;
                let blits = core::mem::take(&mut vm.host_mut::<DeviceHost>().unwrap().blits);
                if stopping || blits.is_empty() {
                    continue;
                }
                FRAME_DONE.reset();
                critical_section::with(|cs| *FRAME.borrow_ref_mut(cs) = Some(blits));
                // Waits through a page slide too: a dropped frame would leave
                // parts of the launcher on screen (scripts present only what
                // changed). `stop` releases the wait.
                if let Either::Second(()) =
                    select(FRAME_DONE.wait(), poll_workers_until_stop(&mut vm)).await
                {
                    release_frame();
                }
            }
            Step::Slice | Step::Request(_) if busy_us >= BUSY_BUDGET_US => {
                busy_us = 0;
                Timer::after_millis(1).await;
            }
            Step::Slice | Step::Request(_) => embassy_futures::yield_now().await,
            other => break other,
        }
    };
    if START.signaled() {
        next = Some(START.wait().await);
    }
    let elapsed = started.elapsed().as_millis();
    let peak = vm.memory_peak();
    match &outcome {
        Step::Done => info!("lua[{}]: finished", app.id),
        Step::Interrupted => info!("lua[{}]: stopped", app.id),
        Step::Error(e) => warn!("lua[{}]: {e}", app.id),
        other => warn!("lua[{}]: ended with {other:?}", app.id),
    }
    release_frame();
    drop(vm);
    SCREEN_OPEN.store(false, Ordering::Release);
    info!(
        "lua[{}]: {elapsed} ms, {frames} frames, {slices} slices (longest {longest_us} us), \
         peak {peak} B, heap free {free_before} -> {} B",
        app.id,
        esp_alloc::HEAP.free()
    );
    // Ended by itself: the UI leaves the page. Stopped: the UI already left.
    let ended = !stopping && !matches!(outcome, Step::Interrupted);
    STATE.store(if ended { 2 } else { 0 }, Ordering::Release);
    next
}

async fn poll_workers_until_stop(vm: &mut mosaico_lua::Vm) {
    while !STOP.load(Ordering::Acquire) && !START.signaled() {
        vm.poll_workers();
        Timer::after_millis(1).await;
    }
}

/// Acquire the start barrier before requesting stop. State becomes non-running
/// only after lua_close and its finalizers have completed.
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
