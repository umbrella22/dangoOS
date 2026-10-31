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
//! 6. hand the newest frame to the panel plane, prioritizing touch responses;
//! 7. publish a one-second liveness heartbeat for OTA health confirmation.
//!
//! No other task calls LVGL. `Ui::tick` runs on this task, outside
//! `lv_timer_handler` and outside every C callback.
//!
//! # Display path
//!
//! The device uses only the fixed panel plane ([`crate::services::display::plane`]): LVGL hosts
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
//! Launcher <-> app switches frames.slide ([`crate::shell::navigation::transition`]): the
//! old page is copied into the port's snapshot, the new page is rendered
//! once, and for 280 ms the plane uploads bands of both. LVGL and touch are
//! paused during the frames.slide.

use alloc::rc::Rc;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::companion::behavior::affect::HapticPattern;
use crate::companion::behavior::personality::PointerSample;
use crate::companion::render::entrance;
use crate::platform::runtime::time::now_us;
use crate::shell::components::splash::SplashControl;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};
use rs_lvgl::sys;
use rs_lvgl_ui::{App, Ui};
use {crate::companion::behavior::Expression, crate::companion::runtime::Runtime};
use {crate::companion::widget as fox_widget, crate::companion::widget::FoxControl};
use {
    crate::shell::app::theme, crate::shell::app::AppMsg, crate::shell::app::MosaicoApp,
    crate::shell::app::Screen, crate::shell::app::SHOW_SPLASH,
};
use {
    crate::shell::model::reaches_fox_off_home, crate::shell::navigation::route::AppId,
    crate::shell::navigation::route::Page,
};
use {
    crate::shell::navigation::gesture::EdgeGestures, crate::shell::navigation::gesture::TouchRoute,
};
use {crate::shell::navigation::transition, crate::shell::navigation::transition::Direction};

use {
    crate::apps::device as apps, crate::apps::lua::task as lua,
    crate::companion::skins::device as skins, crate::services::display::plane,
    crate::services::display::plane::Frame, crate::services::display::plane::PlaneJob,
    crate::services::display::plane::PlaneUi, crate::services::feedback::haptic,
    crate::services::feedback::lights, crate::services::input::sensors::SENSOR_EVENTS,
    crate::services::input::touch::INPUT, crate::services::input::touch::INPUT_CAPACITY,
    crate::services::power::button_task as buttons,
};
use {crate::services::display::geometry::HEIGHT, crate::services::display::geometry::WIDTH};
use {crate::services::display::lvgl_port, crate::services::display::lvgl_port::DisplayPort};

const _: () = assert!(
    !SHOW_SPLASH,
    "the panel plane cannot display the LVGL splash; upload the splash canvas through the plane before enabling it"
);

/// UI loop period.
pub const UI_PERIOD: Duration = Duration::from_millis(5);
/// Largest animation step passed to `Ui::tick`. Longer stalls are clamped so a
/// log backlog cannot make the fox jump by seconds.
pub const MAX_TICK_MS: u32 = 1000;
/// Liveness publication period for OTA startup health.
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(1);
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

/// Number of one-second UI liveness intervals processed since boot.
pub static HEARTBEATS: AtomicU32 = AtomicU32::new(0);

/// LVGL tick: monotonic milliseconds from the same clock as the input task.
unsafe extern "C" fn tick_ms() -> u32 {
    (now_us() / 1000) as u32
}

#[embassy_executor::task]
pub async fn ui_task(mut plane: PlaneUi) {
    rs_lvgl::init();
    unsafe { sys::lv_tick_set_cb(Some(tick_ms)) };

    // The fox never renders through LVGL; hub pages do (see the module docs).
    // SAFETY: this is the only task that calls LVGL.
    let mut port = unsafe { DisplayPort::new() };

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
    let mut frames = frames::Frames::new();
    let mut business_monitor = crate::companion::behavior::business::Monitor::default();
    app.set_registry(crate::apps::registry::Registry::default());
    ui.mount(&app).expect("initial UI mount failed");

    // The runtime starts with the same clock as the input task: the personality
    // engine's timestamps come from there, and mixing clocks would make the
    // first tap look ancient or future-dated.
    let boot_ms = now_us() / 1000;
    let mut runtime = Runtime::new(boot_ms, Rc::clone(&control));
    // The bundled brain needs no filesystem. Start before draining any input
    // so one-shot module edges and early battery/light state are retained while
    // USB startup mounts storage in another task.
    if let Err(error) = runtime.enable_brain(
        crate::companion::brain::DEFAULT_SOURCE,
        boot_ms,
        &crate::apps::lua::task::PSRAM,
        Some(crate::platform::runtime::resident_budget::clock_us),
    ) {
        warn!("brain: bundled startup failed, native fallback: {error}");
    }
    let mut brain_booted = false;
    let mut lua_lvgl =
        crate::apps::lua::native_ui::LuaLvglService::new(WIDTH as u16, HEIGHT as u16);
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
    let mut last_heartbeat = Instant::now();
    let mut last_tick = Instant::now();
    // A press edge waiting for the next submitted frame.
    let mut pending_touch = false;
    let mut last_screen = app.screen();
    let mut gestures = EdgeGestures::new();
    let mut last_page = app.page();
    let mut settings_sync = settings::Sync::new(app.hub().settings);
    let mut last_sensors_refresh = 0u64;
    let mut sensors_refresh_pending = false;
    let mut last_timer_second = u32::MAX;
    // The next fox frame must clear what the previous page left around it.

    let mut standby = crate::services::power::coordinator::Coordinator::new();

    loop {
        // Native controller/host lifecycle calls can preempt this thread for
        // longer than a resident Lua callback's wall deadline. Acknowledge only
        // here, after the preceding callback finished, then retain its VM state
        // without executing callbacks until the operation releases the pause.
        if crate::services::radio::device::classic::ui_checkpoint()
            && !buttons::low_battery_requested()
        {
            Timer::after_millis(1).await;
            continue;
        }

        // Wall-clock animation step. The loop period is a floor, not a fixed
        // timestep: a long render must slow the engine down by the same amount
        // instead of replaying it faster.

        let now = Instant::now();
        let elapsed_ms = now
            .duration_since(last_tick)
            .as_millis()
            .min(u64::from(MAX_TICK_MS)) as u32;
        last_tick = now;
        let now_ms = now_us() / 1000;

        let standby_blocker =
            if !crate::platform::storage::nor::ready() || !scanned || scan_pending.is_some() {
                Some("boot not ready")
            } else if crate::services::camera::task::busy() {
                Some("camera active")
            } else if app.page() != Page::Home
                || frames.slide != Slide::Idle
                || lua::active_or_pending()
            {
                Some("standby requires idle Home")
            } else if app.management_busy()
                || apps::is_managing()
                || pending_management.is_some()
                || apps::has_request()
                || skins::reload_pending()
            {
                Some("UI work pending")
            } else if app.hub().countdown.phase() == crate::services::timer::Phase::Running {
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
            frames.clear_on_fox = true;
            port.invalidate_all();
            ui.mark_dirty();
        }
        if power.paused {
            crate::services::settings::device::feedback_painted(
                crate::services::settings::device::preferences().saving_revision,
            );
            crate::services::settings::device::skin_feedback_painted();
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
                        urgent: false,
                    });
                }
            }
            Timer::after_millis(20).await;
            continue;
        }

        if crate::services::update::ota::device::pauses_ui() {
            crate::services::settings::device::feedback_painted(
                crate::services::settings::device::preferences().saving_revision,
            );
            crate::services::settings::device::skin_feedback_painted();
            lua_lvgl.release();
            while INPUT.try_receive().is_ok() {}
            while SENSOR_EVENTS.try_receive().is_ok() {}
            // Installation retains only the native progress view. Lua and the
            // resident behavior remain paused while NOR writes yield.
            if crate::services::update::task::busy() && !port.is_busy() {
                let update = crate::services::update::task::snapshot();
                if app.settings_session().update != update
                    || app.page() != Page::App(AppId::Settings)
                    || app.settings_page() != crate::shell::pages::settings::model::Page::Update
                {
                    app.go(Page::App(AppId::Settings));
                    app.set_settings_page(crate::shell::pages::settings::model::Page::Update);
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
                        urgent: false,
                    });
                }
            }
            Timer::after_millis(5).await;
            continue;
        }
        crate::services::update::ota::device::selftest(
            brain_booted && !control.loading() && scanned && scan_pending.is_none(),
        );
        // Storage mounts after USB startup; do not mistake this transient
        // pre-mount state for a damaged persisted brain. Resolve selection once
        // after the mount attempt completes. No custom source keeps the same
        // default VM, including every input it consumed before mounting.
        if !brain_booted && crate::platform::storage::nor::ready() {
            crate::companion::brain::control::boot(
                &mut runtime,
                now_ms,
                &crate::apps::lua::task::PSRAM,
                Some(crate::platform::runtime::resident_budget::clock_us),
            )
            .await;
            runtime.ignore_touch_until(boot_ms + MosaicoApp::touch_hold_off_ms());
            brain_booted = true;
        }

        if let Some((id, choices)) = skins::activate_prepared(&control) {
            settings_sync.skin_activated(&mut app, id, choices);
            ui.mark_dirty();
        }

        let drained = input::sensors(&mut app, &mut runtime, now_ms).await;
        let mut sensors_changed = drained.changed;

        input::pointer(
            &app,
            &mut runtime,
            &mut ui,
            &mut gestures,
            &mut press_state,
            &mut pending_touch,
            frames.slide != Slide::Idle,
            lua_lvgl.active(),
            now_ms,
        )
        .await;

        // ---- hub clock, countdown and live pages ----
        let camera = crate::services::camera::task::snapshot();
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
        let radio = crate::services::radio::Snapshot {
            wifi: crate::services::radio::device::wifi::state(),
            wifi_link: crate::services::radio::device::wifi::link(),
            bluetooth: crate::services::radio::device::classic::state(),
            ip: crate::services::radio::device::wifi::ip(),
        };
        control.set_radio_status(radio);
        for event in business_monitor
            .observe(
                now_ms,
                radio,
                crate::services::audio::task::snapshot().state,
            )
            .into_iter()
            .flatten()
        {
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
            Page::App(AppId::Settings)
                if app.settings_page() == crate::shell::pages::settings::model::Page::System =>
            {
                let uptime_s = now_ms / 1000;
                if app.hub().about.uptime_s != uptime_s {
                    app.hub_mut().about.uptime_s = uptime_s;
                    crate::services::settings::device::refresh_heap();
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
        if !scanned && crate::platform::storage::nor::ready() {
            scanned = true;
            apps::request_scan(true);
            scan_pending = Some(apps::Request {
                reload: true,
                action: None,
            });
        }
        if let Some(registry) = apps::take_scanned() {
            let request = scan_pending.take().expect("requested registry result");
            apply_apps_request(&mut app, request, Some(registry));
            apps::acknowledge_management_refresh();
            ui.mark_dirty();
        }
        if frames.management_frame_submitted && !port.is_busy() && !buttons::shutdown_requested() {
            if let Some(op) = pending_management.take() {
                let result = apps::manage(op).await;
                app.management_finished(op, result);
                apps::set_ui_management_pending(false);
                ui.mark_dirty();
            }
            frames.management_frame_submitted = false;
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
        crate::companion::brain::control::poll(&mut runtime, now_ms, buttons::shutdown_requested())
            .await;
        if app.page() != last_page {
            crate::shell::lifecycle::device::apply(
                last_page,
                app.page(),
                &app,
                &mut gestures,
                &mut runtime,
                &mut port,
                &mut frames,
                now_ms,
            );
            last_timer_second = u32::MAX;
            last_page = app.page();
        }
        let preferences = settings_sync.poll(&mut app, &mut ui, port.is_busy(), &mut frames, radio);

        if lua::take_lvgl_release() || !is_lua_page(app.page()) || buttons::shutdown_requested() {
            lua_lvgl.release();
        }
        if frames.slide == Slide::Idle {
            if let Some(command) = lua::take_lvgl_command() {
                lua::complete_lvgl(execute_lua_native(&mut lua_lvgl, command));
            }
        }

        // ---- merged expression and haptics ----
        // The fox keeps living off Home (lights still react); it only buzzes
        // where it can be seen.
        // Input drains can spend milliseconds in the brain. Age touch responses
        // against the clock here, rather than the timestamp before those drains.
        let tick_now_ms = now_us() / 1_000;
        if let Some(pattern) = runtime.tick(tick_now_ms) {
            if settings_sync.value.haptics && app.page() == Page::Home {
                haptic::play(pattern);
            }
        }
        lights::show(runtime.expression());
        lights::show_native(runtime.native_expression());
        lights::brain_frame(runtime.brain_leds(), now_ms);
        if let Some(command) = runtime.take_brain_command() {
            match command {
                crate::companion::behavior::affect::SystemCommand::Hub => {
                    app.go(Page::Launcher);
                    ui.mark_dirty();
                }
                crate::companion::behavior::affect::SystemCommand::StartListening
                | crate::companion::behavior::affect::SystemCommand::StopActivity => {}
            }
        }

        frames.render(
            &mut ui,
            &mut app,
            &control,
            &mut port,
            &mut plane,
            elapsed_ms,
            now_ms,
            plane_size,
            &mut pending_touch,
            pending_management.is_some(),
            preferences,
        );

        if last_heartbeat.elapsed() >= HEARTBEAT_PERIOD {
            last_heartbeat = Instant::now();
            HEARTBEATS.fetch_add(1, Ordering::Release);
        }

        // Reuse only this loop's existing idle interval for native Lua RPCs.
        // Every request still runs on this sole LVGL owner. The deadline never
        // moves, and eight requests cap work even if the Lua task floods us.
        // Input, behavior, ticking and rendering stay in the normal loop.
        let deadline = Instant::now() + UI_PERIOD;

        let mut window = crate::apps::lua::native_ui::RpcWindow::until(deadline.as_micros());
        let mut return_to_main = false;
        if frames.slide == Slide::Idle && is_lua_page(app.page()) && !lua::screen_open() {
            while window.available(Instant::now().as_micros()) {
                match select(Timer::at(deadline), lua::wait_lvgl_ready()).await {
                    Either::First(()) => break,
                    Either::Second(()) => {
                        // Stop/release are only observed here, never consumed.
                        // The outer loop owns teardown and page transitions.
                        if lua::lvgl_cleanup_pending()
                            || buttons::shutdown_requested()
                            || crate::services::update::ota::device::active()
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
    }
}

mod app_requests;

pub(crate) mod frames;
mod input;
mod native;
mod settings;
use crate::shell::lifecycle::is_lua_page;
use app_requests::apply_apps_request;

use frames::Slide;
use native::execute_lua_native;
