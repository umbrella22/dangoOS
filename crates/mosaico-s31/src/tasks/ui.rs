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
//! Off Home the runtime keeps running, so the mood lights still react to
//! modules and sensors; haptics play only on Home, and only when enabled in
//! Settings. A finished countdown buzzes and opens the timer page.

use alloc::rc::Rc;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};
use lvgl::sys;
use lvgl_ui::Ui;
use mosaico_s31::behavior::affect::HapticPattern;
use mosaico_s31::behavior::personality::PointerSample;
use mosaico_s31::behavior::{Expression, Runtime};
use mosaico_s31::fox::entrance;
use mosaico_s31::hub::gesture::{EdgeGestures, Route};
use mosaico_s31::hub::{AppId, Page};
use mosaico_s31::telemetry::{display as display_stats, latency};
use mosaico_s31::ui::fox_widget::{self, FoxControl};
use mosaico_s31::ui::splash::SplashControl;
use mosaico_s31::ui::{theme, AppMsg, MosaicoApp, Screen, SHOW_SPLASH};

use super::{
    haptic,
    input::{INPUT, INPUT_CAPACITY},
    lights,
    plane::{self, Frame, PlaneJob, PlaneUi},
    sensors::SENSOR_EVENTS,
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

/// Number of one-second UI stats intervals processed since boot.
pub static HEARTBEATS: AtomicU32 = AtomicU32::new(0);

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
    ui.mount(&app).expect("initial UI mount failed");

    // The runtime starts with the same clock as the input task: the personality
    // engine's timestamps come from there, and mixing clocks would make the
    // first tap look ancient or future-dated.
    let boot_ms = latency::now_us() / 1000;
    let mut runtime = Runtime::new(boot_ms, Rc::clone(&control));
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

    loop {
        // Wall-clock animation step. The loop period is a floor, not a fixed
        // timestep: a long render must slow the engine down by the same amount
        // instead of replaying it faster.
        let now = Instant::now();
        let elapsed_ms = now
            .duration_since(last_tick)
            .as_millis()
            .min(u64::from(MAX_TICK_MS)) as u32;
        last_tick = now;
        let now_ms = latency::now_us() / 1000;

        // ---- sensor events -> affect engine ----
        // Drained before the render so a shake that happened during the last
        // frame is part of this one. The channel is bounded and the producer
        // drops the oldest non-edge event, so this loop always terminates.
        let mut sensor_count = 0u32;
        let mut sensors_changed = false;
        while let Ok(event) = SENSOR_EVENTS.try_receive() {
            sensor_count = sensor_count.saturating_add(1);
            sensors_changed |= app.hub_mut().snapshot.observe(&event);
            runtime.on_sensor(event);
            if sensor_count >= SENSOR_DRAIN_MAX {
                break;
            }
        }
        if sensor_count >= SENSOR_DRAIN_MAX {
            warn!("ui_task: sensor drain hit {SENSOR_DRAIN_MAX} events in one loop");
        }

        // ---- pointer queue -> personality engine ----
        // Every event is delivered in order: collapsing would swallow the press
        // edge of a tap that starts and ends inside one 5 ms loop.
        // Edge swipes go to the shell; the rest to the fox (Home) or LVGL.
        while let Ok(event) = INPUT.try_receive() {
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
                        pending_touch = Some(event.at_us);
                    }
                }
                Route::Pass => {
                    if event.pressed && !press_state {
                        // Using an app keeps her awake, without a wake face.
                        runtime.on_interaction(sample.at_ms);
                    }
                    press_state = event.pressed;
                    lvgl_port::set_pointer(sample.x, sample.y, sample.pressed);
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
        if app.screen() != last_screen {
            if app.screen() == Screen::Fox {
                runtime.ignore_touch_until(now_ms + entrance::TOTAL_MS);
            }
            last_screen = app.screen();
        }
        if app.page() != last_page {
            info!("hub: {:?} -> {:?}", last_page, app.page());
            gestures.reset_pass();
            lvgl_port::set_pointer(0, 0, false);
            if app.page() == Page::Home {
                // The entrance replays; a finger still on the glass from the
                // swipe must not interrupt it.
                runtime.ignore_touch_until(now_ms + entrance::TOTAL_MS);
                port.discard_damage();
                clear_on_fox = true;
            } else {
                port.invalidate_all();
            }
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
            }
            settings = current;
        }

        // ---- merged expression and haptics ----
        // The fox keeps living off Home (lights still react); it only buzzes
        // where it can be seen.
        if let Some(pattern) = runtime.tick(now_ms) {
            if settings.haptics && app.page() == Page::Home {
                haptic::play(pattern);
            }
        }
        lights::show(runtime.expression());

        // ---- render into a free slot, then hand the frame to the plane ----
        if let Err(error) = ui.tick(elapsed_ms) {
            warn!("ui_task: Ui::tick rejected: {error}");
        }
        if app.page() == Page::Home {
            if let Some(canvas) = control.take_frame() {
                plane.submit(PlaneJob {
                    canvas: Frame::Pool(canvas),
                    size: plane_size,
                    at: None,
                    damage: None,
                    clear_outside: core::mem::take(&mut clear_on_fox),
                    touch_at_us: pending_touch.take(),
                });
            }
        } else if !port.is_busy() {
            ui.timer_handler();
            // Callbacks fired by the timer handler only queue messages.
            if let Err(error) = ui.drain(&mut app, DRAIN_MAX) {
                warn!("ui_task: Ui::drain rejected: {error}");
            }
            if let Some((screen, damage)) = port.take_frame() {
                plane.submit(PlaneJob {
                    canvas: Frame::Screen(screen),
                    size: (WIDTH as u16, HEIGHT as u16),
                    at: Some((0, 0)),
                    damage: Some(damage),
                    clear_outside: false,
                    touch_at_us: None,
                });
            }
        }

        if last_stats.elapsed() >= STATS_PERIOD {
            last_stats = Instant::now();
            let (samples, p50, p95, max) = latency::summary();
            let heartbeat = HEARTBEATS.fetch_add(1, Ordering::Relaxed) + 1;
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

        Timer::after(UI_PERIOD).await;
    }
}
