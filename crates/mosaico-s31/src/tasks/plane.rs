//! The panel plane: uploads the fox canvas at a fixed position.
//!
//! The fox never moves across the panel. The fox widget renders its canvas
//! (expression plus the in-canvas entrance) into a two-slot
//! [`CanvasPool`](mosaico_s31::ui::canvas_pool::CanvasPool), and this task
//! uploads the newest frame below the panel centre. There is no damage union
//! and no old-position clear: the rectangle never changes.
//!
//! # Ownership and overlap
//!
//! A [`PlaneJob`] carries a [`ReadLease`]: while this task holds it the
//! renderer cannot write that slot, and dropping the job at the end of the
//! upload returns the slot. The renderer draws the next frame into the other
//! slot at the same time.
//!
//! The task runs on a high-priority interrupt executor (see `main.rs`): the
//! rasterizer is synchronous CPU work in thread mode, and on a shared
//! cooperative executor every chunk's DMA completion had to wait for the
//! raster to finish (a 9 ms upload stretched to 37 ms). Preemption lets each
//! chunk continue as soon as its DMA completes.
//!
//! A touch-tagged job bypasses the TE wait to keep the response inside the
//! 50 ms latency gate; animation frames start on the TE edge.

use core::sync::atomic::Ordering;

use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Input;
use log::{info, warn};
use mosaico_s31::ui::canvas_pool::ReadLease;

use mosaico_s31::telemetry::display::{
    AVG_TRANSFER_US, BRIGHTNESS_RESTORED, FIRST_FRAME, FPS, FRAMES, LAST_TRANSFER_MS,
    LAST_TRANSFER_US, MAX_TRANSFER_MS, TE_BYPASS, TE_SYNCS, TE_TIMEOUTS, TE_WAIT_AVG_US,
    TOUCH_FRAMES,
};
use mosaico_s31::telemetry::latency;

use crate::drivers::display::Display;
use crate::drivers::geometry::{DirtyRect, HEIGHT, WIDTH};

/// Vertical offset of the character canvas below the panel centre. The fox
/// is bottom-anchored in its canvas with headroom for hops; 16 px down keeps
/// the visual centre of the face on the panel centre.
const FOX_DROP_PX: u16 = 16;
/// Longest wait for a TE edge before transferring anyway.
const TE_TIMEOUT: Duration = Duration::from_millis(30);
/// Rows per transfer chunk; 32 rows is the proven CO5300 burst at 80 MHz.
const CHUNK_ROWS: u16 = 32;
const LOG_EVERY_FRAMES: u32 = 120;
const FPS_WINDOW_US: u64 = 1_000_000;

/// One canvas upload.
pub struct PlaneJob {
    /// Exclusive read access to the rendered frame for the whole upload.
    pub canvas: ReadLease,
    /// Canvas width and height in pixels.
    pub size: (u16, u16),
    /// Touch timestamp of the press this frame responds to, if any.
    pub touch_at_us: Option<u64>,
}

static JOBS: Signal<CriticalSectionRawMutex, PlaneJob> = Signal::new();

/// UI-side handle: publishes frames to the upload task.
pub struct PlaneUi {
    _private: (),
}

/// Upload-side handle: owned by [`plane_task`].
pub struct PlaneDisplay {
    _private: (),
}

pub fn new() -> (PlaneUi, PlaneDisplay) {
    (PlaneUi { _private: () }, PlaneDisplay { _private: () })
}

impl PlaneUi {
    /// Publish a frame. Never blocks.
    ///
    /// If the upload task has not picked up the previous job yet, that job is
    /// replaced (its lease drops and the slot returns to the renderer), but
    /// its touch tag is carried over so a press is never lost from the
    /// latency statistics.
    pub fn submit(&mut self, mut job: PlaneJob) {
        if let Some(stale) = JOBS.try_take() {
            job.touch_at_us = match (stale.touch_at_us, job.touch_at_us) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        JOBS.signal(job);
    }
}

impl PlaneDisplay {
    async fn next_job(&mut self) -> PlaneJob {
        JOBS.wait().await
    }
}

/// Moves the async panel driver onto the plane's interrupt executor.
///
/// esp-hal marks `Async` drivers `!Send` because their DMA wakers are bound
/// to the core whose interrupt handlers were installed. The firmware runs on
/// core 0 only and both executors live there, so handing the driver from the
/// thread executor to the interrupt executor never crosses a core.
pub struct PanelOwner(Display<esp_hal::Async>);

impl PanelOwner {
    /// # Safety
    ///
    /// The caller must spawn the plane task on an executor that runs on the
    /// same core that created `panel`.
    pub unsafe fn new(panel: Display<esp_hal::Async>) -> Self {
        Self(panel)
    }
}

// SAFETY: see `PanelOwner::new`; the driver never leaves its creating core.
unsafe impl Send for PanelOwner {}

/// Future wrapper that carries the `PanelOwner` guarantee across awaits.
struct SameCore<F>(F);

// SAFETY: only built by `plane_task` from a `PanelOwner`, whose
// constructor contract keeps the future on the creating core.
unsafe impl<F> Send for SameCore<F> {}

impl<F: core::future::Future> core::future::Future for SameCore<F> {
    type Output = F::Output;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> {
        // SAFETY: structural pin projection; `SameCore` never moves `F`.
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}

/// Display task for the fixed panel plane.
#[embassy_executor::task]
pub async fn plane_task(panel: PanelOwner, display: PlaneDisplay, te: Input<'static>) {
    SameCore(run_plane(panel.0, display, te)).await
}

async fn run_plane(
    mut panel: Display<esp_hal::Async>,
    mut display: PlaneDisplay,
    mut te: Input<'static>,
) {
    info!("plane: fixed canvas path ready");

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
                        if old == 0 {
                            wait_us
                        } else {
                            (old * 7 + wait_us) / 8
                        },
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

        let (canvas_w, canvas_h) = job.size;
        let (origin_x, origin_y) = origin(job.size);
        let mut y = 0u16;
        while y < canvas_h {
            let y2 = (y + CHUNK_ROWS - 1).min(canvas_h - 1);
            let dst = DirtyRect {
                x0: origin_x,
                y0: origin_y + y,
                x1: origin_x + canvas_w - 1,
                y1: origin_y + y2,
            };
            let src = DirtyRect {
                x0: 0,
                y0: y,
                x1: canvas_w - 1,
                y1: y2,
            };
            let mut result = panel
                .write_canvas_async(dst, src, job.size, job.canvas.pixels())
                .await;
            if result.is_err() {
                result = panel
                    .write_canvas_async(dst, src, job.size, job.canvas.pixels())
                    .await;
            }
            match result {
                Ok(()) => chunks += 1,
                Err(error) => {
                    warn!("plane: canvas {dst:?} failed: {error:?}");
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
                    info!("plane: brightness restored after the first canvas");
                }
                Err(error) => warn!("plane: brightness restore failed: {error:?}"),
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

        // Return the slot to the renderer before logging.
        drop(job);

        if frames == 1 || frames.is_multiple_of(LOG_EVERY_FRAMES) || !completed {
            info!("plane: frame={frames} chunks={chunks} transfer={elapsed_us}us ok={completed}");
        }
    }
}

/// Top-left of the canvas on the panel: centred horizontally, [`FOX_DROP_PX`]
/// below centre vertically.
fn origin((w, h): (u16, u16)) -> (u16, u16) {
    let x = (WIDTH as u16 - w) / 2;
    let y = ((HEIGHT as u16 - h) / 2 + FOX_DROP_PX).min(HEIGHT as u16 - h);
    (x, y)
}
