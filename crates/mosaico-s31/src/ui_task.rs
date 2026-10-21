//! The single LVGL-owning task.
//!
//! Everything that touches LVGL runs here, once every 5 ms ([`UI_PERIOD`]):
//!
//! 1. drain `SENSOR_EVENTS` into the [`BallRuntime`];
//! 2. drain the bounded touch channel into the runtime's personality engine;
//! 3. apply queued UI messages (`Ui::drain`);
//! 4. advance the runtime, which merges the affect and personality layers and
//!    forwards haptics;
//! 5. advance custom widgets (`Ui::tick`): the ball widget rasterizes into a
//!    free canvas-pool slot on its 50 ms cadence (or immediately after a touch);
//! 6. hand the newest frame to the ball plane, tagged with the press it
//!    answers;
//! 7. log a once-per-second stats line.
//!
//! No other task calls LVGL. `Ui::tick` runs on this task, outside
//! `lv_timer_handler` and outside every C callback.
//!
//! # Display path
//!
//! The device uses only the fixed ball plane: LVGL hosts the custom widget's
//! lifecycle, but never renders or flushes the screen (`lv_timer_handler` is
//! not called), so the display port is headless. The full-screen presenter
//! path that used to live here could not run on the device (its consumer task
//! was never spawned) and has been removed. Rendering no longer waits for the
//! upload: the canvas pool gives the renderer the second slot while the first
//! one is on the wire.
//!
//! The LVGL splash therefore cannot be shown on this path; the compile-time
//! check below keeps `ui::SHOW_SPLASH` from silently freezing the boot screen.

use alloc::rc::Rc;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_time::{Duration, Instant, Timer};
use grok_ball::personality::PointerSample;
use grok_ball::types::BallExpression;
use log::{info, warn};
use lvgl::sys;
use lvgl_ui::Ui;
use mosaico_s31::runtime::is_ball_event;

use crate::{
    ball_plane::{self, PlaneJob},
    display_stats, haptic,
    input_task::{INPUT, INPUT_CAPACITY},
    latency,
    lvgl_port::DisplayPort,
    sensor_task::SENSOR_EVENTS,
    ui::{theme, BallApp, Screen, SHOW_SPLASH},
};
use mosaico_s31::ball_widget::BallControl;
use mosaico_s31::runtime::BallRuntime;
use mosaico_s31::splash_widget::SplashControl;

const _: () = assert!(
    !SHOW_SPLASH,
    "the ball plane cannot display the LVGL splash; upload the splash canvas through the plane before enabling it"
);

/// UI loop period.
pub const UI_PERIOD: Duration = Duration::from_millis(5);
/// Largest animation step passed to `Ui::tick`. Longer stalls are clamped so a
/// log backlog cannot make the ball jump by seconds.
pub const MAX_TICK_MS: u32 = 1000;
/// Console stats line period.
pub const STATS_PERIOD: Duration = Duration::from_secs(1);
/// Maximum messages drained per loop.
const DRAIN_MAX: usize = 32;
/// Maximum sensor events consumed per UI loop. The queue is capacity 64; a
/// larger backlog is left for the next loop so the UI never stalls on sensors.
const SENSOR_DRAIN_MAX: u32 = 24;

/// Number of one-second UI stats intervals processed since boot.
pub static HEARTBEATS: AtomicU32 = AtomicU32::new(0);

/// LVGL tick: monotonic milliseconds from the same clock as the input task.
unsafe extern "C" fn tick_ms() -> u32 {
    (latency::now_us() / 1000) as u32
}

#[embassy_executor::task]
pub async fn ui_task(mut plane: ball_plane::PlaneUi) {
    lvgl::init();
    unsafe { sys::lv_tick_set_cb(Some(tick_ms)) };

    // LVGL needs a registered display to create objects, even though this
    // path never renders through it.
    // SAFETY: this is the only task that calls LVGL.
    let _display_port = unsafe { DisplayPort::new() };

    // The ball widget's profiling counters need a microsecond clock; the
    // library cannot depend on `esp-hal`, so the firmware installs it here.
    mosaico_s31::ball_widget::install_mono_us(latency::now_us);

    let display = lvgl::Display::default();
    let mut ui = Ui::new(&display, theme()).expect("LVGL UI creation failed");
    let control = BallControl::new(BallExpression::Idle);
    // Must precede the mount: the widget hides its native canvas and leaves
    // both pool slots to the renderer.
    control.set_plane_mode(true);
    let splash = SplashControl::new();
    let mut app = BallApp::new(Rc::clone(&control), Rc::clone(&splash));
    ui.mount(&app).expect("initial UI mount failed");

    // The runtime starts with the same clock as the input task: the personality
    // engine's timestamps come from there, and mixing clocks would make the
    // first tap look ancient or future-dated.
    let boot_ms = latency::now_us() / 1000;
    let mut runtime = BallRuntime::new(boot_ms, Rc::clone(&control));
    // Touch is held off through the entrance: a finger resting on the glass
    // during boot must not interrupt the drop.
    runtime.ignore_touch_until(boot_ms + BallApp::touch_hold_off_ms());

    info!(
        "ui_task: plane path, ball {}x{} @ {}ms, 2 canvas slots",
        mosaico_s31::ball_widget::CANVAS_SIZE,
        mosaico_s31::ball_widget::CANVAS_SIZE,
        mosaico_s31::ball_widget::FRAME_INTERVAL_MS,
    );

    let mut press_state = false;
    let mut last_stats = Instant::now();
    let mut last_tick = Instant::now();
    // A press edge waiting for the next submitted frame.
    let mut pending_touch: Option<u64> = None;
    let mut last_ball: BallExpression = BallExpression::Idle;
    let mut last_screen = app.screen();

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
        while let Ok(event) = SENSOR_EVENTS.try_receive() {
            sensor_count = sensor_count.saturating_add(1);
            if is_ball_event(event.kind) {
                runtime.on_sensor(event);
            }
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
        while let Ok(event) = INPUT.try_receive() {
            let was_pressed = press_state;
            press_state = event.pressed;
            // Only a press the runtime actually acted on is tagged: a touch
            // swallowed by the boot hold-off must not become a latency sample.
            let acted = runtime.on_pointer(PointerSample {
                at_ms: (event.at_us / 1000).max(now_ms),
                x: event.x as i32,
                y: event.y as i32,
                pressed: event.pressed,
            });
            if event.pressed && !was_pressed && acted && pending_touch.is_none() {
                pending_touch = Some(event.at_us);
            }
        }

        // ---- queued UI messages ----
        if let Err(error) = ui.drain(&mut app, DRAIN_MAX) {
            warn!("ui_task: Ui::drain rejected: {error}");
        }
        if app.screen() != last_screen {
            if app.screen() == Screen::Ball {
                runtime.ignore_touch_until(now_ms + mosaico_s31::entrance::TOTAL_MS);
            }
            last_screen = app.screen();
        }

        // ---- merged expression and haptics ----
        if let Some(pattern) = runtime.tick(now_ms) {
            haptic::play(pattern);
        }

        // ---- render into a free slot, then hand the frame to the plane ----
        if let Err(error) = ui.tick(elapsed_ms) {
            warn!("ui_task: Ui::tick rejected: {error}");
        }
        if let Some(canvas) = control.take_frame() {
            plane.submit(PlaneJob {
                canvas,
                touch_at_us: pending_touch.take(),
            });
        }

        if last_stats.elapsed() >= STATS_PERIOD {
            last_stats = Instant::now();
            let (samples, p50, p95, max) = latency::summary();
            let heartbeat = HEARTBEATS.fetch_add(1, Ordering::Relaxed) + 1;
            let fps = display_stats::FPS.load(Ordering::Relaxed);
            let transfer_ms = display_stats::AVG_TRANSFER_US.load(Ordering::Relaxed) / 1000;
            let ball = runtime.expression();
            if ball != last_ball {
                let affect = runtime.affect_state();
                info!(
                    "ui ball {:?} -> {:?} (persona={:?} affect={:?}/{:?})",
                    last_ball,
                    ball,
                    runtime.reaction(),
                    affect.expression,
                    affect.reason
                );
                last_ball = ball;
            }
            info!(
                "plane p50={p50} p95={p95} max={max} n={samples} tx={transfer_ms} fps={fps} eng={} ras={} drop={} ball={ball:?} q={}/{} hb={heartbeat}",
                control.last_engine_us(),
                control.last_raster_us(),
                control.dropped_frames(),
                INPUT.len(),
                INPUT_CAPACITY,
            );
        }

        Timer::after(UI_PERIOD).await;
    }
}
