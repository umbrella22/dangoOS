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
//!
//! # Hub pages (RFC 0006)
//!
//! Off the home page the frame is LVGL's full-screen shadow
//! ([`Frame::Screen`]) placed at the panel origin, and only its damaged
//! rectangle is uploaded. Coming back home, the first fox job asks for
//! `clear_outside`, which paints the panel around the fox canvas black so
//! nothing of the previous page stays visible.
//!
//! The panel brightness follows [`set_brightness`] (Settings); the target is
//! applied after the next completed upload.

use core::sync::atomic::{AtomicU8, Ordering};

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
use crate::drivers::lvgl_port::{ScreenLease, SnapshotLease};
use mosaico_s31::hub::transition::{self, Direction, Source};

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

/// Rows of the black source used by `clear_outside`.
const CLEAR_ROWS: u16 = 8;
/// A black `WIDTH x CLEAR_ROWS` canvas (7.5 KiB of internal RAM).
static BLACK: [u16; WIDTH * CLEAR_ROWS as usize] = [0; WIDTH * CLEAR_ROWS as usize];

/// Panel brightness target in percent (Settings).
static BRIGHTNESS_TARGET: AtomicU8 = AtomicU8::new(100);

/// Set the panel brightness, in percent. Applied after the next upload.
pub fn set_brightness(percent: u8) {
    BRIGHTNESS_TARGET.store(percent.clamp(1, 100), Ordering::Relaxed);
}

/// The pixels a job uploads.
pub enum Frame {
    /// A fox canvas slot.
    Pool(ReadLease),
    /// LVGL's full-screen shadow frame (hub pages).
    Screen(ScreenLease),
    /// One frame of a page slide: bands of the new page (the shadow) and
    /// the old one (the snapshot), see `hub::transition`. Always uploads the
    /// whole panel; `damage` is ignored.
    Slide {
        new: ScreenLease,
        old: SnapshotLease,
        direction: Direction,
        offset: u16,
    },
}

impl Frame {
    fn pixels(&self) -> &[u16] {
        match self {
            Frame::Pool(lease) => lease.pixels(),
            Frame::Screen(lease) => lease.pixels(),
            Frame::Slide { new, .. } => new.pixels(),
        }
    }
}

/// One canvas upload.
pub struct PlaneJob {
    /// Set only for Sleep frames after the entrance finished during shutdown.
    pub shutdown_sleep: bool,
    /// Exclusive read access to the rendered frame for the whole upload.
    pub canvas: Frame,
    /// Canvas width and height in pixels.
    pub size: (u16, u16),
    /// Panel position of the canvas' top-left pixel; `None` places the fox
    /// canvas (see [`origin`]).
    pub at: Option<(u16, u16)>,
    /// Region of the canvas to upload, in canvas coordinates; `None` uploads
    /// all of it. Widened to the panel's 4-pixel column alignment here.
    pub damage: Option<DirtyRect>,
    /// Paint the panel outside the canvas black before the upload.
    pub clear_outside: bool,
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
            job.clear_outside |= stale.clear_outside;
            // A dropped shadow job's damage must still reach the panel.
            if let (Frame::Screen(_), Frame::Screen(_)) = (&stale.canvas, &job.canvas) {
                job.damage = match (stale.damage, job.damage) {
                    (Some(a), Some(b)) => Some(union(a, b)),
                    _ => None,
                };
            }
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
    let mut applied_brightness: Option<u8> = None;

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
        let (origin_x, origin_y) = job.at.unwrap_or_else(|| origin(job.size));
        let full = DirtyRect {
            x0: 0,
            y0: 0,
            x1: canvas_w - 1,
            y1: canvas_h - 1,
        };
        let area = align4(job.damage.map_or(full, |d| clip(d, full)), canvas_w);

        if job.clear_outside {
            let inner = DirtyRect {
                x0: origin_x,
                y0: origin_y,
                x1: origin_x + canvas_w - 1,
                y1: origin_y + canvas_h - 1,
            };
            for band in outside(inner) {
                if !clear(&mut panel, band).await {
                    completed = false;
                }
            }
        }

        if let Frame::Slide {
            new,
            old,
            direction,
            offset,
        } = &job.canvas
        {
            let (bands, count) = transition::bands(*direction, *offset, WIDTH as u16);
            for band in bands.iter().take(count).flatten() {
                let pixels = match band.source {
                    Source::New => new.pixels(),
                    Source::Old => old.pixels(),
                };
                let mut y = 0u16;
                while completed && y < HEIGHT as u16 {
                    let y2 = (y + CHUNK_ROWS - 1).min(HEIGHT as u16 - 1);
                    let dst = DirtyRect {
                        x0: band.dst_x,
                        y0: y,
                        x1: band.dst_x + band.width - 1,
                        y1: y2,
                    };
                    let src = DirtyRect {
                        x0: band.src_x,
                        y0: y,
                        x1: band.src_x + band.width - 1,
                        y1: y2,
                    };
                    match panel.write_canvas_async(dst, src, job.size, pixels).await {
                        Ok(()) => chunks += 1,
                        Err(error) => {
                            warn!("plane: slide {dst:?} failed: {error:?}");
                            completed = false;
                        }
                    }
                    y = y2 + 1;
                }
            }
        }

        let mut y = area.y0;
        let slide = matches!(job.canvas, Frame::Slide { .. });
        while completed && !slide && y <= area.y1 {
            let y2 = (y + CHUNK_ROWS - 1).min(area.y1);
            let dst = DirtyRect {
                x0: origin_x + area.x0,
                y0: origin_y + y,
                x1: origin_x + area.x1,
                y1: origin_y + y2,
            };
            let src = DirtyRect {
                x0: area.x0,
                y0: y,
                x1: area.x1,
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

        super::buttons::frame_uploaded(job.shutdown_sleep, completed);

        let elapsed_us = started.elapsed().as_micros();
        let elapsed_ms = (elapsed_us / 1000) as u32;
        LAST_TRANSFER_US.store(elapsed_us.min(u32::MAX as u64) as u32, Ordering::Relaxed);
        LAST_TRANSFER_MS.store(elapsed_ms, Ordering::Relaxed);
        MAX_TRANSFER_MS.fetch_max(elapsed_ms, Ordering::Relaxed);
        let frames = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
        FIRST_FRAME.store(true, Ordering::Relaxed);

        let target = BRIGHTNESS_TARGET.load(Ordering::Relaxed);
        if completed && applied_brightness != Some(target) {
            match panel.set_brightness_async(target).await {
                Ok(()) => {
                    applied_brightness = Some(target);
                    if !BRIGHTNESS_RESTORED.swap(true, Ordering::Relaxed) {
                        info!("plane: brightness restored after the first canvas");
                    } else {
                        info!("plane: brightness {target}%");
                    }
                }
                Err(error) => warn!("plane: brightness {target}% failed: {error:?}"),
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

/// Smallest rectangle covering both.
fn union(a: DirtyRect, b: DirtyRect) -> DirtyRect {
    DirtyRect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

/// `rect` limited to `bounds`.
fn clip(rect: DirtyRect, bounds: DirtyRect) -> DirtyRect {
    DirtyRect {
        x0: rect.x0.max(bounds.x0),
        y0: rect.y0.max(bounds.y0),
        x1: rect.x1.min(bounds.x1),
        y1: rect.y1.min(bounds.y1),
    }
}

/// Widen `rect` to whole 4-pixel columns (the panel's window granularity).
/// Canvas widths are multiples of 4, so the result stays inside the canvas.
fn align4(rect: DirtyRect, canvas_w: u16) -> DirtyRect {
    DirtyRect {
        x0: rect.x0 & !3,
        x1: ((rect.x1 | 3).min(canvas_w - 1)),
        ..rect
    }
}

/// The four panel bands around `inner` (some may be empty).
fn outside(inner: DirtyRect) -> [DirtyRect; 4] {
    let right = WIDTH as u16 - 1;
    let bottom = HEIGHT as u16 - 1;
    let empty = DirtyRect {
        x0: 1,
        y0: 1,
        x1: 0,
        y1: 0,
    };
    let top = if inner.y0 > 0 {
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: right,
            y1: inner.y0 - 1,
        }
    } else {
        empty
    };
    let below = if inner.y1 < bottom {
        DirtyRect {
            x0: 0,
            y0: inner.y1 + 1,
            x1: right,
            y1: bottom,
        }
    } else {
        empty
    };
    let left = if inner.x0 > 0 {
        DirtyRect {
            x0: 0,
            y0: inner.y0,
            x1: inner.x0 - 1,
            y1: inner.y1,
        }
    } else {
        empty
    };
    let side = if inner.x1 < right {
        DirtyRect {
            x0: inner.x1 + 1,
            y0: inner.y0,
            x1: right,
            y1: inner.y1,
        }
    } else {
        empty
    };
    [top, below, left, side]
}

/// Paint `band` black in [`CLEAR_ROWS`] chunks. Returns false on an SPI error.
async fn clear(panel: &mut Display<esp_hal::Async>, band: DirtyRect) -> bool {
    if band.is_empty() {
        return true;
    }
    let mut y = band.y0;
    while y <= band.y1 {
        let y2 = (y + CLEAR_ROWS - 1).min(band.y1);
        let dst = DirtyRect {
            x0: band.x0,
            y0: y,
            x1: band.x1,
            y1: y2,
        };
        let src = DirtyRect {
            x0: 0,
            y0: 0,
            x1: band.x1 - band.x0,
            y1: y2 - y,
        };
        if let Err(error) = panel
            .write_canvas_async(dst, src, (WIDTH as u16, CLEAR_ROWS), &BLACK)
            .await
        {
            warn!("plane: clear {dst:?} failed: {error:?}");
            return false;
        }
        y = y2 + 1;
    }
    true
}
