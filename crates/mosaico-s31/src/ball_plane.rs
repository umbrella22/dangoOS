//! Fixed-position ball plane: the simplified hybrid display path.
//!
//! The ball no longer moves across the panel (drag was dropped by product
//! decision). LVGL still renders the 240x240 canvas with the ball's expression
//! and the in-canvas entrance, and this task uploads that fixed canvas at the
//! panel centre. There is no damage union, no old-position clear, no edge scrub
//! and no per-move trace: the canvas rectangle never changes, so the square
//! boundary artifacts cannot occur.
//!
//! A touch-tagged job bypasses the TE wait to keep the response inside the
//! 50 ms latency gate; animation frames start on the TE edge.

use core::sync::atomic::Ordering;

use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Input;
use log::{info, warn};

use crate::{
    display_task::{
        self, AVG_TRANSFER_US, BRIGHTNESS_RESTORED, FIRST_FRAME, FPS, FRAMES,
        LAST_TRANSFER_MS, LAST_TRANSFER_US, MAX_TRANSFER_MS, TE_BYPASS,
        TE_SYNCS, TE_TIMEOUTS, TE_WAIT_AVG_US, TOUCH_FRAMES,
    },
    drivers::display::Display,
    framebuffer::{DirtyRect, HEIGHT, WIDTH},
    latency,
};

/// Panel-side edge of the ball canvas.
pub const BALL_SIDE: u16 = mosaico_s31::ball_widget::CANVAS_SIZE as u16;
/// Top-left of the fixed canvas on the panel.
pub const BALL_ORIGIN: i32 = (WIDTH as i32 - BALL_SIDE as i32) / 2;
/// Longest wait for a TE edge before transferring anyway.
const TE_TIMEOUT: Duration = Duration::from_millis(30);
/// Rows per transfer chunk; 32 rows is the proven CO5300 burst at 80 MHz.
const CHUNK_ROWS: u16 = 32;
const LOG_EVERY_FRAMES: u32 = 120;
const FPS_WINDOW_US: u64 = 1_000_000;

/// One canvas upload. The canvas is always at [`BALL_ORIGIN`].
#[derive(Clone, Copy)]
pub struct PlaneJob {
    /// The mounted 240x240 canvas, valid until the widget is unmounted.
    pub canvas: &'static [u16],
    /// Touch timestamp of the press this frame responds to, if any.
    pub touch_at_us: Option<u64>,
}

static JOBS: Signal<CriticalSectionRawMutex, PlaneJob> = Signal::new();

pub struct PlaneUi {
    _private: (),
}

pub struct PlaneDisplay {
    _private: (),
}

pub fn new() -> (PlaneUi, PlaneDisplay) {
    (PlaneUi { _private: () }, PlaneDisplay { _private: () })
}

impl PlaneUi {
    /// Publish a canvas. Returns `false` when the previous upload is still in
    /// flight; the caller retries after `DISPLAY_ACTIVE` clears.
    pub fn submit(&mut self, job: PlaneJob) -> bool {
        if display_task::DISPLAY_ACTIVE.load(Ordering::Relaxed) {
            return false;
        }
        display_task::DISPLAY_ACTIVE.store(true, Ordering::Relaxed);
        JOBS.signal(job);
        true
    }
}

impl PlaneDisplay {
    async fn next_job(&mut self) -> PlaneJob {
        JOBS.wait().await
    }

    fn release(&mut self) {
        display_task::DISPLAY_ACTIVE.store(false, Ordering::Relaxed);
    }
}

/// Display task for the fixed ball plane.
#[embassy_executor::task]
pub async fn ball_plane_task(
    mut panel: Display<esp_hal::Async>,
    mut display: PlaneDisplay,
    mut te: Input<'static>,
) {
    info!("ball_plane: fixed canvas path ready");

    let mut window_start = Instant::now();
    let mut window_frames: u32 = 0;
    let mut window_transfer_us: u64 = 0;

    loop {
        let job = display.next_job().await;

        if job.touch_at_us.is_some() {
            TE_BYPASS.fetch_add(1, Ordering::Relaxed);
        } else {
            let wait_started = Instant::now();
            match select(te.wait_for_rising_edge(), Timer::after(TE_TIMEOUT)).await {
                Either::First(()) => {
                    let wait_us = wait_started.elapsed().as_micros().min(u32::MAX as u64) as u32;
                    let old = TE_WAIT_AVG_US.load(Ordering::Relaxed);
                    TE_WAIT_AVG_US.store(
                        if old == 0 { wait_us } else { (old * 7 + wait_us) / 8 },
                        Ordering::Relaxed,
                    );
                    TE_SYNCS.fetch_add(1, Ordering::Relaxed);
                }
                Either::Second(()) => {
                    TE_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let started = Instant::now();
        let mut chunks: u32 = 0;
        let mut completed = true;

        if let Some(at_us) = job.touch_at_us {
            latency::note_first_pixel(at_us);
            TOUCH_FRAMES.fetch_add(1, Ordering::Relaxed);
        }

        let mut y = 0u16;
        while y < BALL_SIDE {
            let y2 = (y + CHUNK_ROWS - 1).min(BALL_SIDE - 1);
            let dst = DirtyRect {
                x0: BALL_ORIGIN as u16,
                y0: BALL_ORIGIN as u16 + y,
                x1: BALL_ORIGIN as u16 + BALL_SIDE - 1,
                y1: BALL_ORIGIN as u16 + y2,
            };
            let src = DirtyRect {
                x0: 0,
                y0: y,
                x1: BALL_SIDE - 1,
                y1: y2,
            };
            let mut result = panel
                .write_canvas_async(dst, src, BALL_SIDE, job.canvas)
                .await;
            if result.is_err() {
                result = panel
                    .write_canvas_async(dst, src, BALL_SIDE, job.canvas)
                    .await;
            }
            match result {
                Ok(()) => chunks += 1,
                Err(error) => {
                    warn!("ball_plane: canvas {dst:?} failed: {error:?}");
                    completed = false;
                    break;
                }
            }
            y = y2 + 1;
        }

        let elapsed_us = started.elapsed().as_micros();
        let elapsed_ms = (elapsed_us / 1000) as u32;
        LAST_TRANSFER_US.store(elapsed_us.min(u32::MAX as u64) as u32, Ordering::Relaxed);
        LAST_TRANSFER_MS.store(elapsed_ms, Ordering::Relaxed);
        MAX_TRANSFER_MS.fetch_max(elapsed_ms, Ordering::Relaxed);
        let frames = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
        FIRST_FRAME.store(true, Ordering::Relaxed);

        if !BRIGHTNESS_RESTORED.load(Ordering::Relaxed) && completed {
            match panel.set_brightness_async(100).await {
                Ok(()) => {
                    BRIGHTNESS_RESTORED.store(true, Ordering::Relaxed);
                    info!("ball_plane: brightness restored after the first canvas");
                }
                Err(error) => warn!("ball_plane: brightness restore failed: {error:?}"),
            }
        }

        window_frames += 1;
        window_transfer_us += elapsed_us;
        let window_us = window_start.elapsed().as_micros();
        if window_us >= FPS_WINDOW_US {
            FPS.store(
                (window_frames as u64 * 1_000_000 / window_us) as u32,
                Ordering::Relaxed,
            );
            AVG_TRANSFER_US.store(
                (window_transfer_us / window_frames as u64) as u32,
                Ordering::Relaxed,
            );
            window_frames = 0;
            window_transfer_us = 0;
            window_start = Instant::now();
        }

        display.release();

        if frames == 1 || frames.is_multiple_of(LOG_EVERY_FRAMES) || !completed {
            info!(
                "ball_plane: frame={frames} chunks={chunks} transfer={elapsed_us}us ok={completed}"
            );
        }
    }
}
