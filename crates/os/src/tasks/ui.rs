//! The single LVGL-owning task.
//!
//! Everything that touches LVGL runs here, once every 5 ms ([`UI_PERIOD`]):
//!
//! 1. drain `SENSOR_EVENTS` into the [`Runtime`];
//! 2. drain the bounded touch channel into the runtime's personality engine;
//! 3. apply queued UI messages (`Ui::drain`);
//! 4. advance the runtime, which merges the affect and personality layers and
//!    forwards haptics;
//! 5. advance custom widgets (`Ui::tick`): the fox widget renders into a
//!    free canvas-pool slot on its 50 ms cadence (or immediately after a touch);
//! 6. hand the newest frame to the panel plane, tagged with the press it
//!    answers;
//! 7. log a once-per-second stats line.
//!
//! No other task calls LVGL. `Ui::tick` runs on this task, outside
//! `lv_timer_handler` and outside every C callback.
//!
//! # Display path
//!
//! The device uses only the fixed panel plane ([`super::plane`]): LVGL hosts
//! the custom widget's lifecycle, but never renders or flushes the screen
//! (`lv_timer_handler` is not called), so the display port is headless.
//! Rendering does not wait for the upload: the canvas pool gives the renderer
//! the second slot while the first one is on the wire.
//!
//! The LVGL splash therefore cannot be shown on this path; the compile-time
//! check below keeps `ui::SHOW_SPLASH` from silently freezing the boot screen.
//!
//! # Hub pages (RFC 0006)
//!
//! The fox is the home page. The launcher and the native apps are LVGL
//! pages: on them this task runs `lv_timer_handler`, which renders into the
//! display port's PSRAM shadow, and submits the damaged rectangle to the
//! same plane. Touch first goes through [`EdgeGestures`]: edge swipes switch
//! pages, everything else goes to the fox on Home and to LVGL elsewhere.
//!
//! Off Home the fox is paused: the runtime keeps its clocks, but only
//! lasting state (battery, modules, releases) reaches it, haptics play only
//! on Home, and the LEDs belong to the page ([`light_owner`]): Settings
//! shows a preview so the light switch answers on the hardware, and
//! switching haptics on buzzes once. A finished countdown buzzes and opens
//! the timer page.
//!
//! Lua apps (Snake, 2048) are pages too: opening one starts the script in
//! the Lua task, leaving it stops the script. Once the script opens the
//! display, LVGL stops rendering that page and the script's presented
//! rectangles are copied into the shadow instead ([`lua::take_frame`]);
//! touches that are not edge swipes go to the script.
//!
//! Launcher <-> app switches slide ([`os::hub::transition`]): the
//! old page is copied into the port's snapshot, the new page is rendered
//! once, and for 280 ms the plane uploads bands of both. LVGL and touch are
//! paused during the slide.

use alloc::rc::Rc;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};
use os::behavior::affect::HapticPattern;
use os::behavior::personality::PointerSample;
use os::behavior::{Expression, Runtime};
use os::fox::entrance;
use os::hub::gesture::{EdgeGestures, Route};
use os::hub::transition::{self, Direction};
use os::hub::{reaches_fox_off_home, AppId, Page};
use os::telemetry::{display as display_stats, latency};
use os::ui::fox_widget::{self, FoxControl};
use os::ui::splash::SplashControl;
use os::ui::{theme, AppMsg, MosaicoApp, Screen, SHOW_SPLASH};
use rs_lvgl::sys;
use rs_lvgl_ui::{App, Ui};

use super::{
    apps, buttons, haptic,
    input::{INPUT, INPUT_CAPACITY},
    lights, lua,
    plane::{self, Frame, PlaneJob, PlaneUi},
    sensors::SENSOR_EVENTS,
    skins,
};
use crate::drivers::geometry::{HEIGHT, WIDTH};
use crate::drivers::lvgl_port::{self, DisplayPort};

const _: () = assert!(
    !SHOW_SPLASH,
    "the panel plane cannot display the LVGL splash; upload the splash canvas through the plane before enabling it"
);

/// UI loop period.
pub const UI_PERIOD: Duration = Duration::from_millis(5);
/// Largest animation step passed to `Ui::tick`. Longer stalls are clamped so a
/// log backlog cannot make the fox jump by seconds.
pub const MAX_TICK_MS: u32 = 1000;
/// Console stats line period.
pub const STATS_PERIOD: Duration = Duration::from_secs(1);
/// Maximum messages drained per loop.
const DRAIN_MAX: usize = 32;
/// Maximum sensor events consumed per UI loop. The queue is capacity 64; a
/// larger backlog is left for the next loop so the UI never stalls on sensors.
const SENSOR_DRAIN_MAX: u32 = 24;

/// Keep snapshots current while panel commands or ScreenOff suspend rendering.
fn drain_standby_sensors(app: &mut MosaicoApp, runtime: &mut Runtime) {
    for _ in 0..SENSOR_DRAIN_MAX {
        let Ok(event) = SENSOR_EVENTS.try_receive() else {
            break;
        };
        app.hub_mut().snapshot.observe(&event);
        if reaches_fox_off_home(&event.kind) {
            runtime.on_sensor(event);
        }
    }
}

/// Sensors page refresh period.
const SENSORS_REFRESH_MS: u64 = 200;

/// A page slide between the launcher and an app (`hub::transition`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slide {
    Idle,
    /// Waiting for the plane to release the shadow so the old page can be
    /// copied into the snapshot.
    Pending(Direction),
    /// Uploading slide frames.
    Running {
        direction: Direction,
        start_ms: u64,
    },
    /// One last full upload of the new page.
    Finishing,
}

/// Who drives the LEDs on a page: the fox on Home, the Settings preview in
/// Settings (so the light switch shows on the hardware), dark elsewhere.
fn light_owner(page: Page) -> lights::Owner {
    match page {
        Page::Home => lights::Owner::Fox,
        Page::App(AppId::Settings) => lights::Owner::Preview,
        Page::App(AppId::Lua(_)) => lights::Owner::Lua,
        _ => lights::Owner::Dark,
    }
}

/// Pages whose content a Lua script draws.
fn is_lua_page(page: Page) -> bool {
    matches!(page, Page::App(AppId::Lua(_)))
}

#[inline(always)]
fn execute_lua_native(
    service: &mut os::lua_lvgl::LuaLvglService,
    command: lua_engine::ui::UiCommand,
) -> Result<lua_engine::ui::UiReply, alloc::string::String> {
    #[cfg(feature = "native-ui-diagnostics")]
    let started = latency::now_us();
    let result = service.execute(command);
    #[cfg(feature = "native-ui-diagnostics")]
    os::telemetry::native_ui::with(|p| {
        let ended = latency::now_us();
        let active = service.active() && !lua::screen_open();
        if !p.active() && active {
            p.set_active(true, started);
        }
        p.rpc(ended.saturating_sub(started));
        p.set_active(active, ended);
    });
    result
}

/// Start the Lua app on `page`, if it is one that can run.
fn start_lua_page(app: &MosaicoApp, page: Page) {
    match app.lua_entry(page) {
        Some(entry) if entry.runnable() => lua::start(entry.app.clone()),
        Some(entry) => warn!("apps: {} needs {:?}", entry.app.id, entry.missing),
        None => {}
    }
}

/// Apply a request from the USB push protocol (see `apps`).
fn apply_apps_request(app: &mut MosaicoApp, req: apps::Request, registry: Option<os::hub::registry::Registry>) {
    let page = app.page();
    let open_id = app.lua_entry(page).map(|e| e.app.id.clone());
    if req.reload {
        app.set_registry(registry.expect("rescan result precedes UI publication"));
        if is_lua_page(page) {
            // The same page, possibly at a new index: restart it with its
            // new files, or stop it if it is gone or cannot run any more.
            let still = app
                .lua_entry(app.page())
                .filter(|e| e.runnable())
                .map(|e| e.app.clone());
            match (still, &req.action) {
                (_, Some(apps::Action::Stop)) => {}
                (Some(entry), _) if open_id.as_deref() == Some(entry.id.as_str()) => {
                    info!("apps: restarting {}", entry.id);
                    lua::start(entry);
                }
                _ => lua::stop(),
            }
        }
    }
    match req.action {
        Some(apps::Action::Run(id)) => match app.registry().find(&id) {
            Some(i) => {
                let target = Page::App(AppId::Lua(i as u8));
                if app.page() == target {
                    if !req.reload {
                        start_lua_page(app, target);
                    }
                } else {
                    // The page change starts it (see the main loop).
                    app.go(target);
                }
            }
            None => warn!("apps: run {id}: no such app"),
        },
        Some(apps::Action::Stop) => {
            if is_lua_page(app.page()) {
                app.go(Page::Launcher);
            }
        }
        None => {}
    }
}

/// Number of one-second UI stats intervals processed since boot.
pub static HEARTBEATS: AtomicU32 = AtomicU32::new(0);

/// Region statistics are sampled, not a historical minimum or a contiguous
/// allocation bound. TLSF free payload and occupied block sizes do not sum to
/// the pool span. Call only outside UI work, handler and native RPC timing.
#[cfg(feature = "native-ui-diagnostics")]
fn sample_native_internal_heap(phase: &str, session: u32) {
    let at = latency::now_us();
    let stats = esp_alloc::HEAP.stats();
    let (mut total, mut free, mut allocated) = (0usize, 0usize, 0usize);
    for region in stats.region_stats.iter().flatten() {
        if region
            .capabilities
            .contains(esp_alloc::MemoryCapability::Internal)
        {
            total += region.size;
            free += region.free;
            allocated += region.used;
        }
    }
    let sample_us = latency::now_us().saturating_sub(at);
    info!(
        "native-heap sampled=1 phase={} s={} at={} total={} free={} allocated={} sample_us={}",
        phase, session, at, total, free, allocated, sample_us,
    );
}

/// LVGL tick: monotonic milliseconds from the same clock as the input task.
unsafe extern "C" fn tick_ms() -> u32 {
    (latency::now_us() / 1000) as u32
}

#[embassy_executor::task]
pub async fn ui_task(mut plane: PlaneUi) {
    rs_lvgl::init();
    unsafe { sys::lv_tick_set_cb(Some(tick_ms)) };

    // The fox never renders through LVGL; hub pages do (see the module docs).
    // SAFETY: this is the only task that calls LVGL.
    let mut port = unsafe { DisplayPort::new() };

    // The widget's profiling counters need a microsecond clock; the widget
    // also runs on the host, so the firmware installs the device clock here.
    fox_widget::install_mono_us(latency::now_us);

    let display = rs_lvgl::Display::default();
    let mut ui = Ui::new(&display, theme()).expect("LVGL UI creation failed");
    let control = FoxControl::new(Expression::Idle);
    control.set_loading(true);
    // Must precede the mount: the widget hides its native canvas and leaves
    // both pool slots to the renderer, and the pool is sized for the fox.
    control.set_plane_mode(true);
    let splash = SplashControl::new();
    let mut app = MosaicoApp::new(Rc::clone(&control), Rc::clone(&splash));
    // Decoded launcher icons live as long as the firmware; rescans reuse them.
    // Built-ins until the file systems are mounted (`main::mount_storage`).
    let mut scan_pending = None::<apps::Request>;
    let mut scanned = false;
    let mut pending_management = None;
    let mut management_frame_submitted = false;
    let mut business_monitor = os::behavior::business::Monitor::default();
    app.set_registry(os::hub::registry::Registry::default());
    ui.mount(&app).expect("initial UI mount failed");

    // The runtime starts with the same clock as the input task: the personality
    // engine's timestamps come from there, and mixing clocks would make the
    // first tap look ancient or future-dated.
    let boot_ms = latency::now_us() / 1000;
    let mut runtime = Runtime::new(boot_ms, Rc::clone(&control));
    // The bundled brain needs no filesystem. Start before draining any input
    // so one-shot module edges and early battery/light state are retained while
    // USB startup mounts storage in another task.
    if let Err(error) = runtime.enable_brain(
        os::behavior::brain::DEFAULT_SOURCE,
        boot_ms,
        &super::lua::PSRAM,
        Some(crate::resident_budget::clock_us),
    ) {
        warn!("brain: bundled startup failed, native fallback: {error}");
    }
    let mut brain_booted = false;
    let mut lua_lvgl = os::lua_lvgl::LuaLvglService::new(WIDTH as u16, HEIGHT as u16);
    // Touch is held off through the entrance: a finger resting on the glass
    // during boot must not interrupt the drop.
    runtime.ignore_touch_until(boot_ms + MosaicoApp::touch_hold_off_ms());

    let plane_size = (
        fox_widget::CANVAS_WIDTH as u16,
        fox_widget::CANVAS_HEIGHT as u16,
    );
    info!(
        "ui_task: plane path, fox {}x{} @ {}ms, 2 canvas slots",
        plane_size.0,
        plane_size.1,
        fox_widget::FRAME_INTERVAL_MS,
    );

    let mut press_state = false;
    let mut last_stats = Instant::now();
    let mut last_tick = Instant::now();
    // A press edge waiting for the next submitted frame.
    let mut pending_touch: Option<u64> = None;
    let mut last_shown = Expression::Idle;
    let mut last_screen = app.screen();
    let mut gestures = EdgeGestures::new();
    let mut last_page = app.page();
    let mut settings = app.hub().settings;
    let mut preferences_loaded = false;
    let mut settings_edited = false;
    let mut last_settings_result = super::settings::result();
    let mut saving_frame_submitted = 0u32;
    let mut skin_loading_rendered = false;
    let mut skin_loading_frame_submitted = false;
    let mut last_settings_page = os::hub::settings::Page::Root;
    let mut last_sensors_refresh = 0u64;
    let mut sensors_refresh_pending = false;
    let mut last_timer_second = u32::MAX;
    let mut last_camera_frame = 0u32;
    // The next fox frame must clear what the previous page left around it.
    let mut clear_on_fox = false;
    let mut slide = Slide::Idle;

    #[cfg(feature = "native-ui-diagnostics")]
    let mut heap_sample_state = (false, 0u32, Instant::now());
    #[cfg(feature = "native-ui-diagnostics")]
    {
        port.report_draw_configuration();
        sample_native_internal_heap("startup", 0);
        // #region debug-point popup-shadow-build-marker
        info!("{}", os::telemetry::popup_shadow::STARTUP_MARKER);
        // #endregion debug-point popup-shadow-build-marker
    }

    let mut standby = super::power::Coordinator::new();
    // #region debug-point G-I:cooperative-ui-phases
    #[cfg(feature = "radio-hang-diagnostics")]
    let mut work_trace = (0u32, 0u64, 0u64, 0u64, 0u64);
    // #endregion
    loop {
        // Native controller/host lifecycle calls can preempt this thread for
        // longer than a resident Lua callback's wall deadline. Acknowledge only
        // here, after the preceding callback finished, then retain its VM state
        // without executing callbacks until the operation releases the pause.
        if super::radio::classic::ui_checkpoint() && !buttons::low_battery_requested() {
            Timer::after_millis(1).await;
            continue;
        }
        #[cfg(feature = "native-ui-diagnostics")]
        let profile_loop_start = latency::now_us();
        #[cfg(feature = "native-ui-diagnostics")]
        os::telemetry::native_ui::with(|p| p.loop_start(profile_loop_start));
        // Wall-clock animation step. The loop period is a floor, not a fixed
        // timestep: a long render must slow the engine down by the same amount
        // instead of replaying it faster.
        #[cfg(feature = "touch-latency-diagnostics")]
        os::telemetry::touch_trace::with(|t| t.begin_loop());
        let now = Instant::now();
        let elapsed_ms = now
            .duration_since(last_tick)
            .as_millis()
            .min(u64::from(MAX_TICK_MS)) as u32;
        last_tick = now;
        let now_ms = latency::now_us() / 1000;

        let standby_blocker = if !crate::drivers::storage::ready() || !scanned || scan_pending.is_some() {
            Some("boot not ready")
        } else if super::camera::busy() {
            Some("camera active")
        } else if app.page() != Page::Home || slide != Slide::Idle || lua::active_or_pending() {
            Some("standby requires idle Home")
        } else if app.management_busy()
            || apps::is_managing()
            || pending_management.is_some()
            || apps::has_request()
            || skins::reload_pending()
        {
            Some("UI work pending")
        } else if app.hub().countdown.phase() == os::hub::countdown::Phase::Running {
            Some("countdown running")
        } else if press_state
            || buttons::snapshot()
                .pressed
                .iter()
                .any(|&pressed| pressed != 0)
        {
            Some("input held")
        } else {
            None
        };
        let power = {
            // Sleep Out waits for the panel for over a second. Keep consuming
            // sensor state while the same, pinned request remains in flight.
            let mut poll = core::pin::pin!(standby.poll(standby_blocker));
            loop {
                match select(poll.as_mut(), Timer::after_millis(20)).await {
                    Either::First(power) => break power,
                    Either::Second(()) => drain_standby_sensors(&mut app, &mut runtime),
                }
            }
        };
        if power.redraw {
            control.mark_urgent();
            clear_on_fox = true;
            port.invalidate_all();
            ui.mark_dirty();
        }
        if power.paused {
            super::settings::feedback_painted(super::settings::preferences().saving_revision);
            super::settings::skin_feedback_painted();
            // Keep persistent sensor state current without running animations or
            // the resident brain. Input producers retain wake generations.
            drain_standby_sensors(&mut app, &mut runtime);
            Timer::after_millis(20).await;
            continue;
        }

        // Reserve protection outranks OTA/candidate windows and every app.
        // The factory bootloader splash has already completed before main().
        if buttons::low_battery_requested() {
            lua::stop();
            lua_lvgl.release();
            if app.screen() != Screen::LowBattery {
                app.show_low_battery();
                lvgl_port::set_pointer(0, 0, false);
                port.invalidate_all();
                plane::set_brightness(20);
                ui.mark_dirty();
            }
            if !port.is_busy() {
                let _ = ui.drain(&mut app, DRAIN_MAX);
                ui.timer_handler();
                if let Some((screen, _)) = port.take_frame() {
                    plane.submit(PlaneJob {
                        shutdown_low_battery: true,
                        shutdown_sleep: false,
                        canvas: Frame::Screen(screen),
                        size: (WIDTH as u16, HEIGHT as u16),
                        at: Some((0, 0)),
                        damage: None,
                        clear_outside: false,
                        touch_at_us: None,
                    });
                }
            }
            Timer::after_millis(20).await;
            continue;
        }

        if crate::ota_runtime::pauses_ui() {
            super::settings::feedback_painted(super::settings::preferences().saving_revision);
            super::settings::skin_feedback_painted();
            lua_lvgl.release();
            while INPUT.try_receive().is_ok() {}
            while SENSOR_EVENTS.try_receive().is_ok() {}
            // Installation retains only the native progress view. Lua and the
            // resident behavior remain paused while NOR writes yield.
            if super::update::busy() && !port.is_busy() {
                let update = super::update::snapshot();
                if app.settings_session().update != update
                    || app.page() != Page::App(AppId::Settings)
                    || app.hub().settings_page != os::hub::settings::Page::Update
                {
                    app.go(Page::App(AppId::Settings));
                    app.hub_mut().settings_page = os::hub::settings::Page::Update;
                    app.settings_session_mut().update = update;
                    app.settings_session_mut().info.busy = true;
                    ui.mark_dirty();
                }
                lvgl_port::set_pointer(0, 0, false);
                let _ = ui.drain(&mut app, DRAIN_MAX);
                ui.timer_handler();
                if let Some((screen, damage)) = port.take_frame() {
                    plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                        shutdown_sleep: false,
                        canvas: Frame::Screen(screen),
                        size: (WIDTH as u16, HEIGHT as u16),
                        at: Some((0, 0)),
                        damage: Some(damage),
                        clear_outside: false,
                        touch_at_us: None,
                    });
                }
            }
            Timer::after_millis(5).await;
            continue;
        }
        crate::ota_runtime::selftest(brain_booted && !control.loading() && scanned && scan_pending.is_none());
        // Storage mounts after USB startup; do not mistake this transient
        // pre-mount state for a damaged persisted brain. Resolve selection once
        // after the mount attempt completes. No custom source keeps the same
        // default VM, including every input it consumed before mounting.
        if !brain_booted && crate::drivers::storage::ready() {
            super::brain_control::boot(
                &mut runtime,
                now_ms,
                &super::lua::PSRAM,
                Some(crate::resident_budget::clock_us),
            ).await;
            runtime.ignore_touch_until(boot_ms + MosaicoApp::touch_hold_off_ms());
            brain_booted = true;
        }

        if let Some((id, choices)) = skins::activate_prepared(&control) {
            let session = app.settings_session_mut();
            session.selected_skin = id;
            session.skins = choices;
            session.loading_skin = None;
            session.message = None;
            ui.mark_dirty();
        }

        // ---- sensor events -> affect engine ----
        // Drained before the render so a shake that happened during the last
        // frame is part of this one. The channel is bounded and the producer
        // drops the oldest non-edge event, so this loop always terminates.
        #[cfg(feature = "touch-latency-diagnostics")]
        let trace_sensor_started = latency::now_us();
        // #region debug-point G-I:cooperative-ui-phases
        #[cfg(feature = "radio-hang-diagnostics")]
        let work_sensor_start = (latency::now_us(), crate::resident_budget::clock_us());
        // #endregion
        let mut sensor_count = 0u32;
        let mut sensors_changed = false;
        while let Ok(event) = SENSOR_EVENTS.try_receive() {
            sensor_count = sensor_count.saturating_add(1);
            sensors_changed |= app.hub_mut().snapshot.observe(&event);
            // Off Home the fox is paused: the apps own the lights and the
            // motor, and only lasting state (battery, modules) reaches her.
            match event.kind {
                os::behavior::affect::EventKind::KeyDown => lights::native_reflex(now_ms, 260),
                os::behavior::affect::EventKind::ModuleInserted(_)
                | os::behavior::affect::EventKind::ModuleRemoved(_) => {
                    lights::native_reflex(now_ms, 5000)
                }
                _ => {}
            }
            if app.page() == Page::Home || reaches_fox_off_home(&event.kind) {
                // #region debug-point G-I:cooperative-ui-phases
                #[cfg(feature = "radio-hang-diagnostics")]
                let work_rpc_start = crate::resident_budget::clock_us();
                // #endregion
                runtime.on_sensor(event);
                // #region debug-point G-I:cooperative-ui-phases
                #[cfg(feature = "radio-hang-diagnostics")]
                { work_trace.4 = work_trace.4.max(crate::resident_budget::clock_us().saturating_sub(work_rpc_start)); }
                // #endregion
            }
            // A FIFO batch may deliver 32 IMU events at once. Keep their
            // order, but let input I2C completion and HCI pumping run between
            // brain callbacks instead of waiting for the whole batch/render.
            embassy_futures::yield_now().await;
            if sensor_count >= SENSOR_DRAIN_MAX {
                break;
            }
        }
        if sensor_count >= SENSOR_DRAIN_MAX {
            warn!("ui_task: sensor drain hit {SENSOR_DRAIN_MAX} events in one loop");
        }
        // #region debug-point G-I:cooperative-ui-phases
        #[cfg(feature = "radio-hang-diagnostics")]
        {
            work_trace.0 += sensor_count;
            work_trace.1 += latency::now_us().saturating_sub(work_sensor_start.0);
            work_trace.2 += crate::resident_budget::clock_us().saturating_sub(work_sensor_start.1);
            work_trace.3 = work_trace.3.max(latency::now_us().saturating_sub(work_sensor_start.0));
        }
        // #endregion

        #[cfg(feature = "touch-latency-diagnostics")]
        os::telemetry::touch_trace::with(|t| {
            t.sensor(latency::now_us().saturating_sub(trace_sensor_started))
        });

        // ---- pointer queue -> personality engine ----
        // Every event is delivered in order: collapsing would swallow the press
        // edge of a tap that starts and ends inside one 5 ms loop.
        // Edge swipes go to the shell; the rest to the fox (Home) or LVGL.
        // Yielding lets producers refill the channel, so retain a finite
        // per-loop bound even while a finger keeps producing new samples.
        for _ in 0..INPUT_CAPACITY {
            let Ok(event) = INPUT.try_receive() else { break; };
            #[cfg(feature = "touch-latency-diagnostics")]
            let trace_dequeue = latency::now_us();
            #[cfg(feature = "touch-latency-diagnostics")]
            let trace_rpc_before = os::telemetry::touch_trace::with(|t| t.rpc_snapshot());
            let sample = PointerSample {
                at_ms: (event.at_us / 1000).max(now_ms),
                x: event.x as i32,
                y: event.y as i32,
                pressed: event.pressed,
            };
            match gestures.feed(sample) {
                Route::Gesture(gesture) => {
                    // Swiping between pages counts as using her: stay awake.
                    runtime.on_interaction(sample.at_ms);
                    if let Err(error) = ui.post(AppMsg::Gesture(gesture)) {
                        warn!("ui_task: gesture post rejected: {error}");
                    }
                }
                Route::Held => {}
                Route::Pass if app.page() == Page::Home => {
                    let was_pressed = press_state;
                    press_state = event.pressed;
                    // Only a press the runtime actually acted on is tagged: a
                    // touch swallowed by the hold-off must not become a
                    // latency sample.
                    let acted = runtime.on_pointer(sample);
                    if event.pressed && !was_pressed && acted && pending_touch.is_none() {
                        #[cfg(feature = "touch-latency-diagnostics")]
                        os::telemetry::touch_trace::with(|t| {
                            t.accept(
                                event.at_us,
                                trace_dequeue,
                                latency::now_us(),
                                trace_rpc_before,
                            )
                        });
                        pending_touch = Some(event.at_us);
                    }
                }
                // Touches during a slide are dropped; LVGL resumes clean.
                Route::Pass if slide != Slide::Idle => press_state = event.pressed,
                Route::Pass => {
                    if event.pressed && !press_state {
                        // Using an app keeps her awake, without a wake face.
                        runtime.on_interaction(sample.at_ms);
                    }
                    press_state = event.pressed;
                    if is_lua_page(app.page()) {
                        if lua_lvgl.active() {
                            lvgl_port::set_pointer(sample.x, sample.y, sample.pressed);
                        } else {
                            lua::set_pointer(sample.x, sample.y, sample.pressed);
                        }
                    } else {
                        lvgl_port::set_pointer(sample.x, sample.y, sample.pressed);
                    }
                }
            }
            // A touch burst must likewise allow the hardware producers to
            // progress while every press/release still reaches its consumer.
            embassy_futures::yield_now().await;
        }

        // ---- hub clock, countdown and live pages ----
        let camera = super::camera::snapshot();
        let previous = app.hub().camera;
        // Live frame counters do not rebuild status cards for every capture.
        if (camera.state, camera.pid, camera.saved, camera.error)
            != (previous.state, previous.pid, previous.saved, previous.error)
        {
            app.hub_mut().camera = camera;
            sensors_changed = true;
            if app.page() == Page::App(AppId::Camera) {
                ui.mark_dirty();
            }
        }
        let radio = os::radio::Snapshot {
            wifi: super::radio::wifi::state(),
            bluetooth: super::radio::classic::state(),
            ip: super::radio::wifi::ip(),
        };
        for event in business_monitor.observe(now_ms, radio, super::audio::snapshot().state).into_iter().flatten() {
            runtime.on_sensor(event);
        }
        if app.hub().radio != radio {
            app.hub_mut().radio = radio;
            sensors_changed = true;
        }
        sensors_refresh_pending |= sensors_changed;
        app.hub_mut().now_ms = now_ms;
        if app.hub_mut().countdown.poll(now_ms) {
            info!("hub: countdown finished");
            if app.hub().settings.haptics {
                haptic::play(HapticPattern::Surprised);
            }
            app.go(Page::App(AppId::Timer));
            ui.mark_dirty();
        }
        match app.page() {
            Page::App(AppId::Timer) => {
                let second = app.hub().countdown.display_seconds(now_ms);
                if second != last_timer_second {
                    last_timer_second = second;
                    ui.mark_dirty();
                }
            }
            Page::App(AppId::Sensors)
                if sensors_refresh_pending
                    && now_ms.saturating_sub(last_sensors_refresh) >= SENSORS_REFRESH_MS =>
            {
                last_sensors_refresh = now_ms;
                sensors_refresh_pending = false;
                ui.mark_dirty();
            }
            Page::App(AppId::Settings) if app.hub().settings_page == os::hub::settings::Page::System => {
                let uptime_s = now_ms / 1000;
                if app.hub().about.uptime_s != uptime_s {
                    let about = &mut app.hub_mut().about;
                    about.uptime_s = uptime_s;
                    about.free_kib = (esp_alloc::HEAP.free() / 1024) as u32;
                    ui.mark_dirty();
                }
            }
            _ => {}
        }

        // ---- queued UI messages ----
        if let Err(error) = ui.drain(&mut app, DRAIN_MAX) {
            warn!("ui_task: Ui::drain rejected: {error}");
        }
        apps::set_ui_management_pending(app.management_busy());
        if app.take_management_haptic() && app.hub().settings.haptics {
            haptic::play(HapticPattern::MagneticContact);
        }
        if app.screen() != last_screen {
            if app.screen() == Screen::Fox {
                runtime.ignore_touch_until(now_ms + entrance::TOTAL_MS);
            }
            last_screen = app.screen();
        }
        if !scanned && crate::drivers::storage::ready() {
            scanned = true;
            apps::request_scan(true);
            scan_pending = Some(apps::Request { reload: true, action: None });
        }
        if let Some(registry) = apps::take_scanned() {
            let request = scan_pending.take().expect("requested registry result");
            apply_apps_request(&mut app, request, Some(registry));
            apps::acknowledge_management_refresh();
            ui.mark_dirty();
        }
        if management_frame_submitted && !port.is_busy() && !buttons::shutdown_requested() {
            if let Some(op) = pending_management.take() {
                let result = apps::manage(op).await;
                app.management_finished(op, result);
                apps::set_ui_management_pending(false);
                ui.mark_dirty();
            }
            management_frame_submitted = false;
        }
        // The request is retained through this loop's render/submit before
        // its potentially slow stop barrier and flash writes next iteration.
        if pending_management.is_none() {
            pending_management = app.take_management();
            if pending_management.is_some() {
                apps::set_ui_management_pending(true);
            }
        }
        if pending_management.is_none() && scan_pending.is_none() {
            if let Some(req) = apps::take_request() {
                if req.reload {
                    apps::request_scan(false);
                    scan_pending = Some(req);
                } else {
                    apply_apps_request(&mut app, req, None);
                }
                ui.mark_dirty();
            }
        }
        // A Lua app that ended by itself (finished or failed) leaves its page.
        if lua::take_ended() && is_lua_page(app.page()) {
            app.go(Page::Launcher);
            ui.mark_dirty();
        }
        // Terminal shutdown is the sole page-rule override: stop any Lua app,
        // show Sleep on Home, and ignore queued navigation until power is cut.
        if buttons::shutdown_requested() {
            if app.page() != Page::Home {
                app.go(Page::Home);
                ui.mark_dirty();
            }
            runtime.begin_shutdown();
        }
        super::brain_control::poll(&mut runtime, now_ms, buttons::shutdown_requested()).await;
        if app.page() != last_page {
            info!("hub: {:?} -> {:?}", last_page, app.page());
            gestures.reset_pass();
            gestures.set_full_screen(is_lua_page(app.page()));
            lvgl_port::set_pointer(0, 0, false);
            if is_lua_page(last_page) {
                lua::stop();
                lua::set_pointer(0, 0, false);
            }
            start_lua_page(&app, app.page());
            if last_page==Page::App(AppId::Camera) || app.page()==Page::App(AppId::Camera) {
                super::camera::set_ui_preview(app.page()==Page::App(AppId::Camera));
                last_camera_frame=0;
            }
            if app.page() == Page::Home {
                // The entrance replays; a finger still on the glass from the
                // swipe must not interrupt it.
                runtime.ignore_touch_until(now_ms + entrance::TOTAL_MS);
                port.discard_damage();
                clear_on_fox = true;
            } else {
                port.invalidate_all();
            }
            slide = match transition::Direction::between(last_page, app.page()) {
                Some(direction) => Slide::Pending(direction),
                None => Slide::Idle,
            };
            lights::set_owner(light_owner(app.page()));
            last_timer_second = u32::MAX;
            last_page = app.page();
        }
        let preferences = super::settings::preferences();
        if app.page() != Page::App(AppId::Settings) || app.hub().settings_page != os::hub::settings::Page::Character {
            // Once the chooser is hidden, activation need not wait for its frame.
            super::settings::skin_feedback_painted();
            skin_loading_frame_submitted = false;
        } else if skin_loading_frame_submitted && !port.is_busy() {
            // The loading frame has reached the panel before any resource I/O.
            super::settings::skin_feedback_painted();
            skin_loading_frame_submitted = false;
        }
        if app.page() != Page::App(AppId::Settings) || matches!(app.hub().settings_page, os::hub::settings::Page::Password | os::hub::settings::Page::Proxy) {
            super::settings::feedback_painted(preferences.saving_revision);
        } else if saving_frame_submitted == preferences.saving_revision && !port.is_busy() {
            // The shadow lease has returned: the indicator reached the panel.
            super::settings::feedback_painted(saving_frame_submitted);
        }
        if preferences.saving_revision != saving_frame_submitted {
            saving_frame_submitted = 0;
        }
        if !preferences_loaded && preferences.loaded {
            // Edits made before mounting storage take precedence over saved
            // boot values. Loading preferences never replays haptics.
            if !settings_edited && app.hub().settings == settings {
                if let Some(initial) = preferences.initial {
                    app.hub_mut().settings = initial;
                    settings = initial;
                    plane::set_brightness(initial.brightness);
                    super::audio::volume(initial.audio_volume);
                    super::audio::mute(initial.audio_muted);
                    lights::set_enabled(initial.lights);
                    haptic::set_enabled(initial.haptics);
                    super::camera::set_preferences(initial);
                    ui.mark_dirty();
                }
            }
            preferences_loaded = true;
        }
        if let Some(action) = app.take_settings_action() {
            let accepted = super::settings::submit(action);
            if !accepted {
                app.settings_session_mut().loading_skin = None;
            }
            app.settings_session_mut().message = if accepted { None } else { Some("请等待当前操作完成") };
            ui.mark_dirty();
        }
        if let Some(proxy) = super::settings::take_proxy_change() {
            app.update(AppMsg::UpdateProxySet(proxy));
            ui.mark_dirty();
        }
        let result = super::settings::result();
        if result != last_settings_result {
            last_settings_result = result;
            app.settings_session_mut().networks = super::radio::wifi::networks();
            app.settings_session_mut().message = if result & 1 != 0 { Some("操作失败，请重试") } else { None };
            if result & 1 != 0 {
                app.settings_session_mut().loading_skin = None;
            }
            ui.mark_dirty();
        }
        if app.hub().settings_page != last_settings_page {
            last_settings_page = app.hub().settings_page;
            if last_settings_page == os::hub::settings::Page::Storage { super::settings::refresh_storage(); }
        }
        if app.page() == Page::App(AppId::Settings) {
            let wifi_saved = super::radio::wifi::saved();
            if app.settings_session().wifi_saved != wifi_saved {
                app.settings_session_mut().wifi_saved = wifi_saved;
                ui.mark_dirty();
            }
            let update = super::update::snapshot();
            if app.settings_session().update != update {
                app.settings_session_mut().update = update;
                if matches!(app.hub().settings_page, os::hub::settings::Page::Root | os::hub::settings::Page::Update) {
                    ui.mark_dirty();
                }
            }
            let sound=super::audio::snapshot();
        if app.hub().settings.audio_volume==settings.audio_volume && sound.volume!=settings.audio_volume {
            app.hub_mut().settings.audio_volume=sound.volume;ui.mark_dirty();
        }
        if app.hub().settings.audio_muted==settings.audio_muted && sound.muted!=settings.audio_muted {
            app.hub_mut().settings.audio_muted=sound.muted;ui.mark_dirty();
        }
        let current = app.hub().settings;
            let info = os::hub::settings::Info {
                wifi: radio.wifi, bluetooth: radio.bluetooth, ip: radio.ip,
                audio: super::audio::snapshot().state,
                busy: super::settings::busy() || super::radio::wifi::busy() || app.settings_session().wifi_saved.saving || app.settings_session().loading_skin.is_some() || crate::ota_runtime::active() || super::update::busy(),
                storage_ready: preferences.storage_ready, data_total_kib: preferences.total_kib, data_free_kib: preferences.free_kib,
                saved: preferences.persisted == Some(current), save_error: preferences.error || app.settings_session().wifi_saved.error,
                saving: preferences.saving_revision != 0 || app.settings_session().wifi_saved.saving,
                candidate: crate::ota_runtime::candidate(),
                boot_slot: crate::ota_runtime::booted_slot().ok(),
            };
            if app.settings_session().info != info {
                app.settings_session_mut().info = info;
                ui.mark_dirty();
            }
        }
        let current = app.hub().settings;
        super::update::configure(current);
        if app.page()==Page::App(AppId::Camera) {
            if let Some(action)=app.take_camera_action() {
                match action {
                    os::camera::Command::Capture=>{super::camera::request_capture();},
                    os::camera::Command::Start=>super::camera::set_ui_preview(true),
                    _=>{},
                }
            }
        }
        if current != settings {
            if current.audio_volume != settings.audio_volume { super::audio::volume(current.audio_volume); }
            if current.audio_muted != settings.audio_muted { super::audio::mute(current.audio_muted); }
            if current.brightness != settings.brightness {
                plane::set_brightness(current.brightness);
            }
            if current.lights != settings.lights {
                lights::set_enabled(current.lights);
                if current.lights {
                    // Switched on in Settings: replay the wipe on the strip.
                    lights::replay_preview();
                }
            }
            haptic::set_enabled(current.haptics);
            if current.haptics && !settings.haptics {
                // Switched on: one buzz so the motor answers the switch.
                haptic::play(HapticPattern::Satisfied);
            }
            super::camera::set_preferences(current);
            settings_edited = true;
            // Returning a control to its saved value must also cancel an older
            // queued edit; the preference task skips the resulting no-op write.
            super::settings::save(current);
            settings = current;
        }

        if lua::take_lvgl_release() || !is_lua_page(app.page()) || buttons::shutdown_requested() {
            lua_lvgl.release();
        }
        if slide == Slide::Idle {
            if let Some(command) = lua::take_lvgl_command() {
                lua::complete_lvgl(execute_lua_native(&mut lua_lvgl, command));
            }
        }

        #[cfg(feature = "native-ui-diagnostics")]
        os::telemetry::native_ui::with(|p| {
            p.set_active(
                slide == Slide::Idle
                    && is_lua_page(app.page())
                    && lua_lvgl.active()
                    && !lua::screen_open(),
                latency::now_us(),
            )
        });

        // ---- merged expression and haptics ----
        // The fox keeps living off Home (lights still react); it only buzzes
        // where it can be seen.
        // Input drains can spend milliseconds in the brain. Age touch responses
        // against the clock here, rather than the timestamp before those drains.
        let tick_now_ms = latency::now_us() / 1_000;
        if let Some(pattern) = runtime.tick(tick_now_ms) {
            if settings.haptics && app.page() == Page::Home {
                haptic::play(pattern);
            }
        }
        lights::show(runtime.expression());
        lights::show_native(runtime.native_expression());
        lights::brain_frame(runtime.brain_leds(), now_ms);
        if let Some(command) = runtime.take_brain_command() {
            match command {
                os::behavior::affect::SystemCommand::Hub => {
                    app.go(Page::Launcher);
                    ui.mark_dirty();
                }
                os::behavior::affect::SystemCommand::StartListening
                | os::behavior::affect::SystemCommand::StopActivity => {}
            }
        }

        // ---- render into a free slot, then hand the frame to the plane ----
        let rendered_before_tick = control.rendered_frames();
        if let Err(error) = ui.tick(elapsed_ms) {
            warn!("ui_task: Ui::tick rejected: {error}");
        }
        if app.page() == Page::Home {
            if let Some(canvas) = control.take_frame() {
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: buttons::shutdown_requested()
                        && control.shutdown_sleep_frame_since(rendered_before_tick),
                    canvas: Frame::Pool(canvas),
                    size: plane_size,
                    at: None,
                    damage: None,
                    clear_outside: core::mem::take(&mut clear_on_fox),
                    touch_at_us: pending_touch.take(),
                });
            }
        } else if slide != Slide::Idle {
            slide = step_slide(slide, &mut port, &mut plane, now_ms);
        } else if is_lua_page(app.page()) && lua::screen_open() {
            // The script owns the screen: copy its presented frame into the
            // shadow and upload the damage; LVGL stays paused.
            if !port.is_busy() {
                if let Some(blits) = lua::take_frame() {
                    for b in &blits {
                        // SAFETY: the script's framebuffer stays untouched
                        // until `frame_done` below (it waits for it).
                        unsafe {
                            port.write_rect((b.x0, b.y0, b.x1, b.y1), b.pixels, b.stride);
                        }
                    }
                    lua::frame_done();
                }
                if let Some((screen, damage)) = port.take_frame() {
                    plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                        shutdown_sleep: false,
                        canvas: Frame::Screen(screen),
                        size: (WIDTH as u16, HEIGHT as u16),
                        at: Some((0, 0)),
                        damage: Some(damage),
                        clear_outside: false,
                        touch_at_us: None,
                    });
                }
            }
        } else if !port.is_busy() {
            #[cfg(feature = "native-ui-diagnostics")]
            os::telemetry::native_ui::with(|p| p.begin_handler(latency::now_us()));
            let skin_feedback_on_frame = skin_loading_rendered;
            ui.timer_handler();
            #[cfg(feature = "native-ui-diagnostics")]
            os::telemetry::native_ui::with(|p| p.end_handler(latency::now_us()));
            // Callbacks fired by the timer handler only queue messages.
            match ui.drain(&mut app, DRAIN_MAX) {
                Ok(_) => {
                    skin_loading_rendered = app.page() == Page::App(AppId::Settings)
                        && app.hub().settings_page == os::hub::settings::Page::Character
                        && app.settings_session().loading_skin.is_some();
                }
                Err(error) => warn!("ui_task: Ui::drain rejected: {error}"),
            }
            // #region debug-point popup-shadow-handler-finished
            #[cfg(feature = "native-ui-diagnostics")]
            lua_lvgl.popup_diagnostics().handler_finished();
            // #endregion debug-point popup-shadow-handler-finished
            apps::set_ui_management_pending(app.management_busy());
            if app.page()==Page::App(AppId::Camera) {
                if let Some(frame)=super::camera::frame() {
                    if frame.sequence!=last_camera_frame {
                        // The Arc retains the completed CPU buffer while the
                        // shadow receives it; camera DMA uses a different buffer.
                        unsafe {port.write_rect((60,96,420,366),frame.pixels.as_ptr().cast(),360*2);}
                        last_camera_frame=frame.sequence;
                    }
                }
            }
            if let Some((screen, damage)) = port.take_frame() {
                if skin_feedback_on_frame {
                    skin_loading_frame_submitted = true;
                }
                // This is the ordinary LVGL busy-state frame, never a Lua
                // or transition frame. Its lease must finish before deletion.
                if pending_management.is_some() && app.page() == Page::Launcher {
                    management_frame_submitted = true;
                }
                if app.page() == Page::App(AppId::Settings) && !matches!(app.hub().settings_page, os::hub::settings::Page::Password | os::hub::settings::Page::Proxy) && app.settings_session().info.saving {
                    saving_frame_submitted = preferences.saving_revision;
                }
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: false,
                    canvas: Frame::Screen(screen),
                    size: (WIDTH as u16, HEIGHT as u16),
                    at: Some((0, 0)),
                    damage: Some(damage),
                    clear_outside: false,
                    touch_at_us: None,
                });
            }
        } else {
            #[cfg(feature = "native-ui-diagnostics")]
            os::telemetry::native_ui::with(|p| p.busy_skip());
        }

        if last_stats.elapsed() >= STATS_PERIOD {
            last_stats = Instant::now();
            // #region debug-point G-I:cooperative-ui-phases
            #[cfg(feature = "radio-hang-diagnostics")]
            {
                info!("ui work: sensors={} wall_us={} cpu_us={} burst_us={} rpc_cpu_max_us={}",work_trace.0,work_trace.1,work_trace.2,work_trace.3,work_trace.4);
                work_trace = (0, 0, 0, 0, 0);
            }
            // #endregion
            #[cfg(feature = "native-ui-diagnostics")]
            os::telemetry::native_ui::report(latency::now_us());
            let (samples, p50, p95, max) = latency::summary();
            let heartbeat = HEARTBEATS.fetch_add(1, Ordering::Relaxed) + 1;
            #[cfg(feature = "touch-latency-diagnostics")]
            os::telemetry::touch_trace::report();
            let fps = display_stats::FPS.load(Ordering::Relaxed);
            let transfer_ms = display_stats::AVG_TRANSFER_US.load(Ordering::Relaxed) / 1000;
            let shown = runtime.expression();
            if shown != last_shown {
                let affect = runtime.affect_state();
                info!(
                    "ui fox {:?} -> {:?} (persona={:?} affect={:?}/{:?})",
                    last_shown,
                    shown,
                    runtime.reaction(),
                    affect.expression,
                    affect.reason
                );
                last_shown = shown;
            }
            info!(
                "plane p50={p50} p95={p95} max={max} n={samples} tx={transfer_ms} fps={fps} anim={} ras={} drop={} fox={shown:?} q={}/{} hb={heartbeat}",
                control.last_animate_us(),
                control.last_render_us(),
                control.dropped_frames(),
                INPUT.len(),
                INPUT_CAPACITY,
            );
        }

        // Reuse only this loop's existing idle interval for native Lua RPCs.
        // Every request still runs on this sole LVGL owner. The deadline never
        // moves, and eight requests cap work even if the Lua task floods us.
        // Input, behavior, ticking and rendering stay in the normal loop.
        let deadline = Instant::now() + UI_PERIOD;
        #[cfg(feature = "native-ui-diagnostics")]
        os::telemetry::native_ui::with(|p| {
            p.work(latency::now_us().saturating_sub(profile_loop_start))
        });
        let mut window = os::lua_lvgl::RpcWindow::until(deadline.as_micros());
        let mut return_to_main = false;
        if slide == Slide::Idle && is_lua_page(app.page()) && !lua::screen_open() {
            while window.available(Instant::now().as_micros()) {
                match select(Timer::at(deadline), lua::wait_lvgl_ready()).await {
                    Either::First(()) => break,
                    Either::Second(()) => {
                        // Stop/release are only observed here, never consumed.
                        // The outer loop owns teardown and page transitions.
                        if lua::lvgl_cleanup_pending()
                            || buttons::shutdown_requested()
                            || crate::ota_runtime::active()
                        {
                            return_to_main = true;
                            break;
                        }
                        if let Some(command) = lua::take_lvgl_command() {
                            lua::complete_lvgl(execute_lua_native(&mut lua_lvgl, command));
                            window.completed();
                        }
                    }
                }
            }
        }
        if !return_to_main {
            Timer::at(deadline).await;
        }
        // #region debug-point popup-shadow-sample-and-output
        #[cfg(feature = "native-ui-diagnostics")]
        {
            // Diagnostics only: read the stable shadow synchronously, then emit
            // at most one cached short line. No await, render, lease or mutation.
            let active = slide == Slide::Idle
                && is_lua_page(app.page())
                && lua_lvgl.active()
                && !lua::screen_open();
            let started = latency::now_us();
            let stable = if active {
                port.diagnostic_stable_shadow()
            } else {
                None
            };
            let diagnostics = lua_lvgl.popup_diagnostics();
            let captured = diagnostics.sample_if_ready(started, active, stable);
            let ended = latency::now_us();
            if captured {
                diagnostics.set_sample_micros(ended.saturating_sub(started));
            }
            if let Some(line) = diagnostics.next_line(ended) {
                info!("{line}");
            }
        }
        // #endregion debug-point popup-shadow-sample-and-output
        #[cfg(feature = "native-ui-diagnostics")]
        {
            // Handler/work/native RPC measurements and the existing idle
            // window have ended. Sampling/logging still adds to loop-to-loop
            // wall time and can overlap an asynchronous plane lease.
            let active = os::telemetry::native_ui::with(|p| p.active());
            let phase = if active != heap_sample_state.0 {
                heap_sample_state.0 = active;
                if active {
                    heap_sample_state.1 += 1;
                    Some("enter")
                } else {
                    Some("exit")
                }
            } else if active && heap_sample_state.2.elapsed() >= STATS_PERIOD {
                Some("periodic")
            } else {
                None
            };
            if let Some(phase) = phase {
                heap_sample_state.2 = Instant::now();
                sample_native_internal_heap(phase, heap_sample_state.1);
            }
        }
    }
}

/// Advance a page slide by one UI loop. LVGL is paused throughout: the new
/// page is rendered once up front and stays in the shadow.
fn step_slide(slide: Slide, port: &mut DisplayPort, plane: &mut PlaneUi, now_ms: u64) -> Slide {
    match slide {
        Slide::Idle => Slide::Idle,
        Slide::Pending(direction) => {
            if !port.snapshot() {
                return slide;
            }
            port.render_now();
            Slide::Running {
                direction,
                start_ms: now_ms,
            }
        }
        Slide::Running {
            direction,
            start_ms,
        } => {
            let elapsed = now_ms.saturating_sub(start_ms);
            if elapsed >= transition::SLIDE_MS {
                return step_slide(Slide::Finishing, port, plane, now_ms);
            }
            if let Some((new, old)) = port.take_slide() {
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: false,
                    canvas: Frame::Slide {
                        new,
                        old,
                        direction,
                        offset: transition::offset(elapsed, WIDTH as u16),
                    },
                    size: (WIDTH as u16, HEIGHT as u16),
                    at: Some((0, 0)),
                    damage: None,
                    clear_outside: false,
                    touch_at_us: None,
                });
            }
            slide
        }
        Slide::Finishing => match port.take_full_frame() {
            Some((screen, damage)) => {
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: false,
                    canvas: Frame::Screen(screen),
                    size: (WIDTH as u16, HEIGHT as u16),
                    at: Some((0, 0)),
                    damage: Some(damage),
                    clear_outside: false,
                    touch_at_us: None,
                });
                port.reset_pointer();
                Slide::Idle
            }
            None => slide,
        },
    }
}
