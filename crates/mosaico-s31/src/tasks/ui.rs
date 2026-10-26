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
//! Launcher <-> app switches slide ([`mosaico_s31::hub::transition`]): the
//! old page is copied into the port's snapshot, the new page is rendered
//! once, and for 280 ms the plane uploads bands of both. LVGL and touch are
//! paused during the slide.

use alloc::rc::Rc;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};
use lvgl::sys;
use lvgl_ui::Ui;
use mosaico_s31::behavior::affect::HapticPattern;
use mosaico_s31::behavior::personality::PointerSample;
use mosaico_s31::behavior::{Expression, Runtime};
use mosaico_s31::fox::entrance;
use mosaico_s31::hub::gesture::{EdgeGestures, Route};
use mosaico_s31::hub::registry::IconCache;
use mosaico_s31::hub::transition::{self, Direction};
use mosaico_s31::hub::{AppId, Page, reaches_fox_off_home};
use mosaico_s31::telemetry::{display as display_stats, latency};
use mosaico_s31::ui::fox_widget::{self, FoxControl};
use mosaico_s31::ui::splash::SplashControl;
use mosaico_s31::ui::{AppMsg, MosaicoApp, SHOW_SPLASH, Screen, theme};

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
    service: &mut mosaico_s31::lua_lvgl::LuaLvglService,
    command: mosaico_lua::ui::UiCommand,
) -> Result<mosaico_lua::ui::UiReply, alloc::string::String> {
    #[cfg(feature = "native-ui-diagnostics")]
    let started = latency::now_us();
    let result = service.execute(command);
    #[cfg(feature = "native-ui-diagnostics")]
    mosaico_s31::telemetry::native_ui::with(|p| {
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
fn apply_apps_request(app: &mut MosaicoApp, icons: &mut IconCache, req: apps::Request) {
    let page = app.page();
    let open_id = app.lua_entry(page).map(|e| e.app.id.clone());
    if req.reload {
        app.set_registry(apps::scan(icons));
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
    lvgl::init();
    unsafe { sys::lv_tick_set_cb(Some(tick_ms)) };

    // The fox never renders through LVGL; hub pages do (see the module docs).
    // SAFETY: this is the only task that calls LVGL.
    let mut port = unsafe { DisplayPort::new() };

    // The widget's profiling counters need a microsecond clock; the widget
    // also runs on the host, so the firmware installs the device clock here.
    fox_widget::install_mono_us(latency::now_us);

    let display = lvgl::Display::default();
    let mut ui = Ui::new(&display, theme()).expect("LVGL UI creation failed");
    let control = FoxControl::new(Expression::Idle);
    // Must precede the mount: the widget hides its native canvas and leaves
    // both pool slots to the renderer, and the pool is sized for the fox.
    control.set_plane_mode(true);
    let splash = SplashControl::new();
    let mut app = MosaicoApp::new(Rc::clone(&control), Rc::clone(&splash));
    // Decoded launcher icons live as long as the firmware; rescans reuse them.
    // Built-ins until the file systems are mounted (`main::mount_storage`).
    let mut icons = IconCache::new();
    let mut scanned = false;
    let mut pending_management = None;
    let mut management_frame_submitted = false;
    let mut skin_loaded = false;
    app.set_registry(mosaico_s31::hub::registry::Registry::builtin_only(
        &mut icons,
    ));
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
        mosaico_s31::behavior::brain::DEFAULT_SOURCE,
        boot_ms,
        &super::lua::PSRAM,
        Some(super::lua::clock_us),
    ) {
        warn!("brain: bundled startup failed, native fallback: {error}");
    }
    let mut brain_booted = false;
    let mut lua_lvgl = mosaico_s31::lua_lvgl::LuaLvglService::new(WIDTH as u16, HEIGHT as u16);
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
    let mut last_sensors_refresh = 0u64;
    let mut last_timer_second = u32::MAX;
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
        info!("{}", mosaico_s31::telemetry::popup_shadow::STARTUP_MARKER);
        // #endregion debug-point popup-shadow-build-marker
    }

    loop {
        #[cfg(feature = "native-ui-diagnostics")]
        let profile_loop_start = latency::now_us();
        #[cfg(feature = "native-ui-diagnostics")]
        mosaico_s31::telemetry::native_ui::with(|p| p.loop_start(profile_loop_start));
        // Wall-clock animation step. The loop period is a floor, not a fixed
        // timestep: a long render must slow the engine down by the same amount
        // instead of replaying it faster.
        #[cfg(feature = "touch-latency-diagnostics")]
        mosaico_s31::telemetry::touch_trace::with(|t| t.begin_loop());
        let now = Instant::now();
        let elapsed_ms = now
            .duration_since(last_tick)
            .as_millis()
            .min(u64::from(MAX_TICK_MS)) as u32;
        last_tick = now;
        let now_ms = latency::now_us() / 1000;

        if crate::ota_runtime::active() {
            lua_lvgl.release();
            while INPUT.try_receive().is_ok() {}
            while SENSOR_EVENTS.try_receive().is_ok() {}
            Timer::after_millis(5).await;
            continue;
        }
        crate::ota_runtime::selftest();
        // Storage mounts after USB startup; do not mistake this transient
        // pre-mount state for a damaged persisted brain. Resolve selection once
        // after the mount attempt completes. No custom source keeps the same
        // default VM, including every input it consumed before mounting.
        if !brain_booted && crate::drivers::storage::ready() {
            super::brain_control::boot(
                &mut runtime,
                now_ms,
                &super::lua::PSRAM,
                Some(super::lua::clock_us),
            );
            runtime.ignore_touch_until(boot_ms + MosaicoApp::touch_hold_off_ms());
            brain_booted = true;
        }

        if crate::drivers::storage::ready() && (!skin_loaded || skins::take_reload()) {
            skins::apply(&control);
            skin_loaded = true;
        }

        // ---- sensor events -> affect engine ----
        // Drained before the render so a shake that happened during the last
        // frame is part of this one. The channel is bounded and the producer
        // drops the oldest non-edge event, so this loop always terminates.
        #[cfg(feature = "touch-latency-diagnostics")]
        let trace_sensor_started = latency::now_us();
        let mut sensor_count = 0u32;
        let mut sensors_changed = false;
        while let Ok(event) = SENSOR_EVENTS.try_receive() {
            sensor_count = sensor_count.saturating_add(1);
            sensors_changed |= app.hub_mut().snapshot.observe(&event);
            // Off Home the fox is paused: the apps own the lights and the
            // motor, and only lasting state (battery, modules) reaches her.
            match event.kind {
                mosaico_s31::behavior::affect::EventKind::KeyDown => {
                    lights::native_reflex(now_ms, 260)
                }
                mosaico_s31::behavior::affect::EventKind::ModuleInserted(_)
                | mosaico_s31::behavior::affect::EventKind::ModuleRemoved(_) => {
                    lights::native_reflex(now_ms, 5000)
                }
                _ => {}
            }
            if app.page() == Page::Home || reaches_fox_off_home(&event.kind) {
                runtime.on_sensor(event);
            }
            if sensor_count >= SENSOR_DRAIN_MAX {
                break;
            }
        }
        if sensor_count >= SENSOR_DRAIN_MAX {
            warn!("ui_task: sensor drain hit {SENSOR_DRAIN_MAX} events in one loop");
        }

        #[cfg(feature = "touch-latency-diagnostics")]
        mosaico_s31::telemetry::touch_trace::with(|t| {
            t.sensor(latency::now_us().saturating_sub(trace_sensor_started))
        });

        // ---- pointer queue -> personality engine ----
        // Every event is delivered in order: collapsing would swallow the press
        // edge of a tap that starts and ends inside one 5 ms loop.
        // Edge swipes go to the shell; the rest to the fox (Home) or LVGL.
        while let Ok(event) = INPUT.try_receive() {
            #[cfg(feature = "touch-latency-diagnostics")]
            let trace_dequeue = latency::now_us();
            #[cfg(feature = "touch-latency-diagnostics")]
            let trace_rpc_before = mosaico_s31::telemetry::touch_trace::with(|t| t.rpc_snapshot());
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
                        mosaico_s31::telemetry::touch_trace::with(|t| {
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
        }

        // ---- hub clock, countdown and live pages ----
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
                if sensors_changed
                    && now_ms.saturating_sub(last_sensors_refresh) >= SENSORS_REFRESH_MS =>
            {
                last_sensors_refresh = now_ms;
                ui.mark_dirty();
            }
            Page::App(AppId::Settings) => {
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
            app.set_registry(apps::scan_at_boot(&mut icons));
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
        if pending_management.is_none() {
            if let Some(req) = apps::take_request() {
                apply_apps_request(&mut app, &mut icons, req);
                apps::acknowledge_management_refresh();
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
        super::brain_control::poll(&mut runtime, now_ms, buttons::shutdown_requested());
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
        let current = app.hub().settings;
        if current != settings {
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
        mosaico_s31::telemetry::native_ui::with(|p| {
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
                mosaico_s31::behavior::affect::SystemCommand::Hub => {
                    app.go(Page::Launcher);
                    ui.mark_dirty();
                }
                mosaico_s31::behavior::affect::SystemCommand::StartListening
                | mosaico_s31::behavior::affect::SystemCommand::StopActivity => {}
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
            mosaico_s31::telemetry::native_ui::with(|p| p.begin_handler(latency::now_us()));
            ui.timer_handler();
            #[cfg(feature = "native-ui-diagnostics")]
            mosaico_s31::telemetry::native_ui::with(|p| p.end_handler(latency::now_us()));
            // Callbacks fired by the timer handler only queue messages.
            if let Err(error) = ui.drain(&mut app, DRAIN_MAX) {
                warn!("ui_task: Ui::drain rejected: {error}");
            }
            // #region debug-point popup-shadow-handler-finished
            #[cfg(feature = "native-ui-diagnostics")]
            lua_lvgl.popup_diagnostics().handler_finished();
            // #endregion debug-point popup-shadow-handler-finished
            apps::set_ui_management_pending(app.management_busy());
            if let Some((screen, damage)) = port.take_frame() {
                // This is the ordinary LVGL busy-state frame, never a Lua
                // or transition frame. Its lease must finish before deletion.
                if pending_management.is_some() && app.page() == Page::Launcher {
                    management_frame_submitted = true;
                }
                plane.submit(PlaneJob {
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
            mosaico_s31::telemetry::native_ui::with(|p| p.busy_skip());
        }

        if last_stats.elapsed() >= STATS_PERIOD {
            last_stats = Instant::now();
            #[cfg(feature = "native-ui-diagnostics")]
            mosaico_s31::telemetry::native_ui::report(latency::now_us());
            let (samples, p50, p95, max) = latency::summary();
            let heartbeat = HEARTBEATS.fetch_add(1, Ordering::Relaxed) + 1;
            #[cfg(feature = "touch-latency-diagnostics")]
            mosaico_s31::telemetry::touch_trace::report();
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
        mosaico_s31::telemetry::native_ui::with(|p| {
            p.work(latency::now_us().saturating_sub(profile_loop_start))
        });
        let mut window = mosaico_s31::lua_lvgl::RpcWindow::until(deadline.as_micros());
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
            let active = mosaico_s31::telemetry::native_ui::with(|p| p.active());
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
