//! The single LVGL-owning task.
//!
//! Everything that touches LVGL runs here, once every 5 ms:
//!
//! 1. drain the bounded touch channel into the latest pointer reading;
//! 2. drain `SENSOR_EVENTS` into the [`BallRuntime`] (H2);
//! 3. wait for a writable presentation frame;
//! 4. invalidate the whole screen for the first two frames (two-frame
//!    partial-flush correctness);
//! 5. dispatch the pointer and apply queued UI messages (`Ui::drain`), so a
//!    press is part of the frame rendered in this same iteration;
//! 6. advance the runtime, which merges the affect and personality layers and
//!    forwards haptics;
//! 7. tag the next submitted frame with the press timestamp;
//! 8. advance custom widgets (`Ui::tick`) and run LVGL timers/rendering;
//! 9. log a once-per-second stats line.
//!
//! No other task calls LVGL. `Ui::tick` runs on this task, outside
//! `lv_timer_handler` and outside every C callback.
//!
//! # Screen phases
//!
//! The app mounts the `ikaros` splash first. The runtime starts counting at the
//! first loop iteration; when the splash timeline reports completion (or the
//! user taps it), the task posts `BallMsg::SplashDone`, which swaps the tree to
//! the ball and starts the entrance drop. Touch is held off until both
//! animations are over, so a finger on the glass cannot skip or grab the ball
//! mid-drop.
//!
//! # Profiling
//!
//! The one-second line reports the average and worst `wait_back_free`, the
//! average and worst `Ui::tick` among ticks that actually rasterized a ball
//! frame, the average and worst `Ui::timer_handler` (LVGL render + flush copy
//! + submit), the display task's average QSPI transfer time, FPS, flushed
//! pixels, tagged frames and the latency run summary. Labels are kept short so
//! the line fits the 158-byte text budget of the console logger.

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
    display_task, haptic,
    input_task::{INPUT, INPUT_CAPACITY},
    latency,
    lvgl_port::{DisplayPort, REFRESH_PERIOD_MS},
    pointer_port::{PointerPort, PointerReading},
    present::Presenter,
    sensor_task::SENSOR_EVENTS,
    ui::{theme, BallApp, BallMsg, Screen},
};
use mosaico_s31::ball_widget::BallControl;
use mosaico_s31::runtime::BallRuntime;
use mosaico_s31::splash_widget::SplashControl;

/// UI loop period. Matches the LVGL display refresh period.
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

/// Windowed timings for the once-per-second stats line.
#[derive(Default)]
struct LoopStats {
    wait_us: u64,
    wait_max_us: u32,
    wait_count: u32,
    tick_us: u64,
    tick_max_us: u32,
    tick_count: u32,
    th_us: u64,
    th_max_us: u32,
    th_count: u32,
}

impl LoopStats {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn wait(&mut self, micros: u32) {
        self.wait_us += u64::from(micros);
        self.wait_max_us = self.wait_max_us.max(micros);
        self.wait_count += 1;
    }

    /// Only ticks that rasterized a ball frame are accumulated; engine-only
    /// ticks would dilute the value that the frame budget cares about.
    fn tick(&mut self, micros: u32) {
        self.tick_us += u64::from(micros);
        self.tick_max_us = self.tick_max_us.max(micros);
        self.tick_count += 1;
    }

    fn timer_handler(&mut self, micros: u32) {
        self.th_us += u64::from(micros);
        self.th_max_us = self.th_max_us.max(micros);
        self.th_count += 1;
    }
}

#[inline]
fn average_ms(sum_us: u64, count: u32) -> u32 {
    if count == 0 {
        0
    } else {
        (sum_us / u64::from(count) / 1000) as u32
    }
}

/// LVGL tick: monotonic milliseconds from the same clock as the input task.
unsafe extern "C" fn tick_ms() -> u32 {
    (latency::now_us() / 1000) as u32
}

#[embassy_executor::task]
pub async fn ui_task(presenter: Presenter, mut plane: ball_plane::PlaneUi) {
    lvgl::init();
    unsafe { sys::lv_tick_set_cb(Some(tick_ms)) };

    // SAFETY: this is the only task that calls LVGL.
    let display_port = unsafe { DisplayPort::new(presenter) };
    // SAFETY: the pointer lives on this task and is created after the display.
    let pointer = unsafe { PointerPort::new(&display_port) };

    // The ball widget's profiling counters need a microsecond clock; the
    // library cannot depend on `esp-hal`, so the firmware installs it here.
    mosaico_s31::ball_widget::install_mono_us(latency::now_us);

    let display = lvgl::Display::default();
    let mut ui = Ui::new(&display, theme()).expect("LVGL UI creation failed");
    let control = BallControl::new(BallExpression::Idle);
    // Hybrid path: LVGL renders the 240x240 canvas, the ball plane uploads it.
    control.set_plane_mode(true);
    let splash = SplashControl::new();
    let mut app = BallApp::new(Rc::clone(&control), Rc::clone(&splash));
    ui.mount(&app).expect("initial UI mount failed");

    // The runtime starts with the same clock as the input task: the personality
    // engine's timestamps come from there, and mixing clocks would make the
    // first tap look ancient or future-dated.
    let boot_ms = latency::now_us() / 1000;
    let mut runtime = BallRuntime::new(boot_ms, Rc::clone(&control));
    // Touch is held off through the splash and the entrance: a finger resting
    // on the glass during boot must not skip the wordmark or grab the ball.
    runtime.ignore_touch_until(boot_ms + BallApp::touch_hold_off_ms());
    // The entrance is armed now; the splash screen is still mounted, so the
    // ball canvas does not exist until the swap. `start_entrance` is idempotent
    // and the widget reads it on its first update.
    let splash_swap_at_ms = boot_ms + mosaico_s31::boot_splash::TOTAL_MS;
    let mut splash_done = false;

    info!(
        "ui_task: LVGL ready, screen {}x{}, refresh {}ms, partial buffer {} rows, ball {}x{} @ {}ms, splash {}x{} until +{}ms",
        crate::framebuffer::WIDTH,
        crate::framebuffer::HEIGHT,
        REFRESH_PERIOD_MS,
        crate::lvgl_port::DRAW_LINES,
        mosaico_s31::ball_widget::CANVAS_SIZE,
        mosaico_s31::ball_widget::CANVAS_SIZE,
        mosaico_s31::ball_widget::FRAME_INTERVAL_MS,
        mosaico_s31::boot_splash::SPLASH_WIDTH,
        mosaico_s31::boot_splash::SPLASH_HEIGHT,
        mosaico_s31::boot_splash::TOTAL_MS,
    );

    let mut press_state = false;
    let mut reading = PointerReading::default();
    let mut last_stats = Instant::now();
    let mut last_tick = Instant::now();
    // Animation time that arrived while the display task was uploading. The
    // skipped `Ui::tick` must not slow the widget's 50 ms frame clock.
    let mut pending_ms: u32 = 0;
    // A press edge waiting for the next submitted plane job.
    let mut pending_touch: Option<u64> = None;
    let mut stats = LoopStats::default();
    let mut last_tagged = display_task::TOUCH_FRAMES.load(Ordering::Relaxed);
    // Both retained frames need the full static black field before either can
    // be used for a canvas-only update.
    let mut init_frames = 2u8;
    let mut last_ball: BallExpression = BallExpression::Idle;
    let mut last_screen = app.screen();

    loop {
        // Wall-clock animation step. The loop period is a floor, not a fixed
        // timestep: a long render or a back-pressure wait must slow the engine
        // down by the same amount instead of replaying it faster.
        let now = Instant::now();
        let elapsed_ms = now
            .duration_since(last_tick)
            .as_millis()
            .min(u64::from(MAX_TICK_MS)) as u32;
        last_tick = now;
        pending_ms = pending_ms.saturating_add(elapsed_ms).min(MAX_TICK_MS * 4);
        let now_ms = latency::now_us() / 1000;

        // ---- sensor events -> affect engine (H2) ----
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
        // LVGL still receives the collapsed newest sample (it only needs the
        // current pointer position), but the personality engine receives every
        // event in order: collapsing would swallow the press edge of a tap that
        // starts and ends inside one 5 ms loop, and the grab needs the full
        // path to compute travel.
        let mut received = false;
        let mut press_at_us = None;
        while let Ok(event) = INPUT.try_receive() {
            received = true;
            let pressed_now = event.pressed;
            let was_pressed = press_state;
            press_state = event.pressed;
            reading = PointerReading {
                x: event.x as i32,
                y: event.y as i32,
                pressed: event.pressed,
                timestamp_ms: (event.at_us / 1000) as u32,
                more: false,
            };
            // Only a press the runtime actually acted on is tagged: a touch
            // swallowed by the boot hold-off must not become a latency sample.
            let acted = runtime.on_pointer(PointerSample {
                at_ms: (event.at_us / 1000).max(now_ms),
                x: event.x as i32,
                y: event.y as i32,
                pressed: event.pressed,
            });
            if pressed_now && !was_pressed && acted && press_at_us.is_none() {
                press_at_us = Some(event.at_us);
            }
        }
        if received {
            pointer.set_reading(reading);
        }

        // The splash ends on its own: the widget's own timeline is the source
        // of truth, so its last fade frame is always complete. The boot-time
        // deadline is only a safety net for a stalled widget.
        if last_screen == Screen::Splash
            && !splash_done
            && (splash.finished() || now_ms >= splash_swap_at_ms)
        {
            splash_done = true;
            if let Err(error) = ui.post(BallMsg::SplashDone) {
                warn!("ui_task: splash done post rejected: {error}");
            }
        }

        // ---- merged expression and haptics ----
        if let Some(pattern) = runtime.tick(now_ms) {
            haptic::play(pattern);
        }

        // ---- hybrid ball plane ----
        //
        // LVGL only advances the custom widget (engine + raster) into its
        // 240x240 canvas. The plane task uploads that canvas directly; the
        // full-screen presenter and LVGL flush are not used on this screen.
        if control.plane_mode() {
            if let Some(at_us) = press_at_us {
                if pending_touch.is_none() {
                    pending_touch = Some(at_us);
                }
            }

            if display_task::DISPLAY_ACTIVE.load(Ordering::Relaxed) {
                // The canvas is being uploaded. Do not rasterize into it; the
                // accumulated `pending_ms` keeps the 50 ms frame clock intact.
                Timer::after(UI_PERIOD).await;
                continue;
            }

            let before_frames = control.rendered_frames();
            if let Err(error) = ui.tick(pending_ms) {
                warn!("ui_task: Ui::tick rejected: {error}");
            }
            pending_ms = 0;

            if control.rendered_frames() != before_frames {
                let job = PlaneJob {
                    canvas: control.canvas_slice(),
                    touch_at_us: pending_touch,
                };
                if plane.submit(job) {
                    pending_touch = None;
                }
            }

            if last_stats.elapsed() >= STATS_PERIOD {
                last_stats = Instant::now();
                let (samples, p50, p95, max) = latency::summary();
                let heartbeat = HEARTBEATS.fetch_add(1, Ordering::Relaxed) + 1;
                let fps = display_task::FPS.load(Ordering::Relaxed);
                let transfer_ms = display_task::AVG_TRANSFER_US.load(Ordering::Relaxed) / 1000;
                let ball = runtime.expression();
                info!(
                    "plane p50={p50} p95={p95} max={max} n={samples} tx={transfer_ms} fps={fps} ball={ball:?} q={}/{} hb={heartbeat}",
                    INPUT.len(),
                    INPUT_CAPACITY,
                );
            }

            Timer::after(UI_PERIOD).await;
            continue;
        }

        // Apply the press before rendering so the response is part of this
        // refresh instead of the next one, then tag the frame that will carry
        // it. `Ui::tick` still runs outside `lv_timer_handler` and outside
        // every C callback.
        pointer.dispatch();
        if let Err(error) = ui.drain(&mut app, DRAIN_MAX) {
            warn!("ui_task: Ui::drain rejected: {error}");
        }
        if app.screen() != last_screen {
            if app.screen() == Screen::Ball {
                // `BallApp::enter_ball` already armed the entrance; it is the
                // single owner of that edge. Only the touch hold-off belongs
                // here, because it deals in wall-clock milliseconds.
                runtime.ignore_touch_until(now_ms + mosaico_s31::entrance::TOTAL_MS);
            }
            last_screen = app.screen();
        }
        if let Some(at_us) = press_at_us {
            display_port.tag_next_frame(at_us);
        }

        // The display task owns one frame and is uploading it. Rendering the
        // next frame now would interleave the ~18 ms raster between its chunk
        // DMA awaits and turn a 9 ms upload into a 37 ms one. Skip this loop's
        // render; the next loop picks up the latest state.
        if display_task::DISPLAY_ACTIVE.load(Ordering::Relaxed) {
            Timer::after(UI_PERIOD).await;
            continue;
        }

        // Never render without a writable frame. The first two frames paint the
        // full static black field into both retained frames; later frames only
        // carry the canvas damage produced by the splash or ball widget.
        let wait_started = Instant::now();
        display_port.wait_back_free().await;
        stats.wait(wait_started.elapsed().as_micros().min(u32::MAX as u64) as u32);

        if init_frames > 0 {
            display_port.invalidate_full();
            init_frames -= 1;
        }

        // Advance custom widgets (ball engine + raster) before LVGL renders.
        display_task::UI_RENDERING.store(true, Ordering::Relaxed);
        let rendered_before = control.rendered_frames();
        let tick_started = Instant::now();
        if let Err(error) = ui.tick(pending_ms) {
            warn!("ui_task: Ui::tick rejected: {error}");
        }
        pending_ms = 0;
        let tick_us = tick_started.elapsed().as_micros();
        if control.rendered_frames() != rendered_before {
            stats.tick(tick_us.min(u32::MAX as u64) as u32);
        }

        let handler_started = Instant::now();
        let frames_before = display_port.frame_count();
        ui.timer_handler();
        if display_port.frame_count() != frames_before {
            stats.timer_handler(handler_started.elapsed().as_micros().min(u32::MAX as u64) as u32);
        }
        display_task::UI_RENDERING.store(false, Ordering::Relaxed);

        if last_stats.elapsed() >= STATS_PERIOD {
            last_stats = Instant::now();
            let (samples, p50, p95, max) = latency::summary();
            let flush = display_port.take_flush_stats();
            let heartbeat = HEARTBEATS.fetch_add(1, Ordering::Relaxed) + 1;
            let tagged = display_task::TOUCH_FRAMES.load(Ordering::Relaxed);
            let tag_delta = tagged.wrapping_sub(last_tagged);
            last_tagged = tagged;
            let fps = display_task::FPS.load(Ordering::Relaxed);
            let transfer_ms = display_task::AVG_TRANSFER_US.load(Ordering::Relaxed) / 1000;
            let ball = runtime.expression();
            let reaction = runtime.reaction();
            let affect = runtime.affect_state();
            if ball != last_ball {
                info!(
                    "ui ball {:?} -> {:?} (persona={reaction:?} affect={:?}/{:?})",
                    last_ball, ball, affect.expression, affect.reason
                );
                last_ball = ball;
            }
            info!(
                "ui p50={p50} p95={p95} max={max} n={samples} tick={} tmx={} th={} thx={} wait={} wmx={} tx={transfer_ms} ty={} fps={fps} px={} tag={tag_delta} ball={:?} q={}/{} hb={heartbeat} person={}",
                average_ms(stats.tick_us, stats.tick_count),
                stats.tick_max_us / 1000,
                average_ms(stats.th_us, stats.th_count),
                stats.th_max_us / 1000,
                average_ms(stats.wait_us, stats.wait_count),
                stats.wait_max_us / 1000,
                display_task::TE_YIELDS.load(Ordering::Relaxed),
                flush.pixels,
                ball,
                INPUT.len(),
                INPUT_CAPACITY,
                if control.entrance_active() { "in" } else { "idle" },
            );
            // Split the ball tick so a regression can be attributed: `tick` is
            // `Ui::tick`, which includes engine advance + raster + invalidate.
            info!(
                "ui ball cost: engine={}us raster={}us",
                control.last_engine_us(),
                control.last_raster_us(),
            );
            stats.reset();
        }

        Timer::after(UI_PERIOD).await;
    }
}
