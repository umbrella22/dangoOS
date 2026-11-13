//! Reproducible Rust allocation measurements on real shell pages.
//! Compare the same workload across published framework releases.
use os::{
    shell::{
        app::theme,
        components::splash::SplashControl,
        navigation::route::{AppId, Page},
        pages::settings::model::Page as SettingsPage,
    },
    AppMsg, Expression, FoxControl, MosaicoApp,
};
use rs_lvgl::Display;
use rs_lvgl_ui::Ui;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering::Relaxed},
    time::Instant,
};

struct MeasuredAllocator;
static MEASURING: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

fn grow(bytes: isize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    if MEASURING.load(Relaxed) {
        PEAK.fetch_max(live, Relaxed);
    }
}

// SAFETY: allocation and deallocation are delegated unchanged to System. The
// counters allocate nothing and only count successful allocations/reallocations.
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grow(layout.size() as isize);
            if MEASURING.load(Relaxed) {
                ALLOCATIONS.fetch_add(1, Relaxed);
            }
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grow(layout.size() as isize);
            if MEASURING.load(Relaxed) {
                ALLOCATIONS.fetch_add(1, Relaxed);
            }
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Relaxed);
        unsafe { System.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        let result = unsafe { System.realloc(p, layout, bytes) };
        if !result.is_null() {
            grow(bytes as isize - layout.size() as isize);
            if MEASURING.load(Relaxed) {
                REALLOCATIONS.fetch_add(1, Relaxed);
            }
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

fn measure(name: &str, ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, msg: Option<AppMsg>) {
    // Warm the final state; measure updates rather than initial page mounting.
    if let Some(msg) = &msg {
        ui.post(msg.clone()).unwrap();
        ui.drain(app, 32).unwrap();
    }
    let live = LIVE.load(Relaxed);
    let before = ui.stats();
    ALLOCATIONS.store(0, Relaxed);
    REALLOCATIONS.store(0, Relaxed);
    PEAK.store(live, Relaxed);
    let start = Instant::now();
    MEASURING.store(true, Relaxed);
    for index in 0..16 {
        if let Some(msg) = &msg {
            let msg = match msg {
                AppMsg::AudioVolume(_) => AppMsg::AudioVolume(40 + index % 2),
                _ => msg.clone(),
            };
            ui.post(msg).unwrap();
        }
        ui.drain(app, 32).unwrap();
    }
    MEASURING.store(false, Relaxed);
    let elapsed = start.elapsed().as_nanos();
    let after = ui.stats();
    let allocations = ALLOCATIONS.load(Relaxed);
    let reallocations = REALLOCATIONS.load(Relaxed);
    let peak = (PEAK.load(Relaxed) - live).max(0);
    let delta = LIVE.load(Relaxed) - live;
    println!(
        "{}",
        serde_json::json!({
            "workload": name, "iterations": 16, "rust_allocations": allocations,
            "rust_reallocations": reallocations, "rust_peak_extra_bytes": peak,
            "rust_live_delta_bytes": delta, "elapsed_ns": elapsed,
            "reconciliations": after.reconciled_updates - before.reconciled_updates,
            "created": after.created_nodes - before.created_nodes,
            "deleted": after.deleted_nodes - before.deleted_nodes,
            "native_heap_measured": false,
        })
    );
}

fn main() {
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 32 }>(480, 480, |_| {}).unwrap();
    let mut app = MosaicoApp::new(FoxControl::new(Expression::Idle), SplashControl::new());
    app.go(Page::App(AppId::Settings));
    app.set_settings_page(SettingsPage::Audio);
    let mut ui = Ui::new(&display, theme()).unwrap();
    ui.mount(&app).unwrap();
    measure("audio-idle", &mut ui, &mut app, None);
    measure(
        "audio-volume",
        &mut ui,
        &mut app,
        Some(AppMsg::AudioVolume(40)),
    );
    app.set_settings_page(SettingsPage::System);
    ui.update(&app).unwrap();
    measure(
        "system-message",
        &mut ui,
        &mut app,
        Some(AppMsg::SettingsPage(SettingsPage::System)),
    );
    app.go(Page::App(AppId::Camera));
    ui.update(&app).unwrap();
    measure(
        "camera-command",
        &mut ui,
        &mut app,
        Some(AppMsg::CameraCapture),
    );
    assert!(app.take_camera_action().is_some());
    ui.unmount();
    drop(ui);
    drop(app);
    // SAFETY: the UI and application are gone before the display and lifecycle.
    unsafe { display.delete() };
    unsafe { rs_lvgl::deinit() };
}
