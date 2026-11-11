//! The panel plane: uploads the fox canvas at a fixed position.
//!
//! The fox widget renders its full panel viewport (expression, entrance,
//! movement and link icons) into a two-slot
//! [`CanvasPool`](crate::services::display::canvas_pool::CanvasPool), and this task
//! uploads the newest frame at the panel origin. There is no damage union
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
//! A touch response bypasses the TE wait; animation frames start on the TE edge.
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

use crate::services::display::canvas_pool::ReadLease;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;
use log::{info, warn};

use crate::services::display::status::{BRIGHTNESS_RESTORED, FIRST_FRAME};

use crate::platform::drivers::display::Display;
use {
    crate::services::display::geometry::DirtyRect, crate::services::display::geometry::HEIGHT,
    crate::services::display::geometry::WIDTH,
};
use {
    crate::services::display::lvgl_port::ScreenLease,
    crate::services::display::lvgl_port::SnapshotLease,
};
use {
    crate::shell::navigation::transition, crate::shell::navigation::transition::Direction,
    crate::shell::navigation::transition::Source,
};

/// Longest wait for a TE edge before transferring anyway.
const TE_TIMEOUT: Duration = Duration::from_millis(30);
/// Rows per transfer chunk; 32 rows is the proven CO5300 burst at 80 MHz.
const CHUNK_ROWS: u16 = 32;

/// Rows of the black source used by `clear_outside`.
const CLEAR_ROWS: u16 = 8;
/// A black `WIDTH x CLEAR_ROWS` canvas (7.5 KiB of internal RAM).
static BLACK: [u16; WIDTH * CLEAR_ROWS as usize] = [0; WIDTH * CLEAR_ROWS as usize];

/// Panel brightness target in percent (Settings).
static BRIGHTNESS_TARGET: AtomicU8 = AtomicU8::new(100);
static BRIGHTNESS_APPLIED: AtomicU8 = AtomicU8::new(0);
static BRIGHTNESS_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub fn brightness() -> Option<u8> {
    match BRIGHTNESS_APPLIED.load(Ordering::Acquire) {
        0 => None,
        value => Some(value),
    }
}

/// Set panel brightness between transfers. Startup/resume waits for its first frame.
pub fn set_brightness(percent: u8) {
    BRIGHTNESS_TARGET.store(percent.clamp(1, 100), Ordering::Relaxed);
    BRIGHTNESS_CHANGED.signal(());
}

/// The pixels a job uploads.
pub enum Frame {
    /// A complete Home frame with character and link icons already composed.
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
    /// Only the full terminal warning can acknowledge the reserve countdown.
    pub shutdown_low_battery: bool,
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
    /// Bypass the frame-boundary wait for an immediate touch response.
    pub urgent: bool,
}

static JOBS: Signal<CriticalSectionRawMutex, PlaneJob> = Signal::new();
static POWER_REQUEST: Signal<CriticalSectionRawMutex, bool> = Signal::new();
static POWER_DONE: Signal<CriticalSectionRawMutex, bool> = Signal::new();

/// Only the UI power coordinator calls this, serially. The plane services it
/// between completed uploads, so commands never race a live DMA transfer.
pub async fn set_sleep(sleep: bool) -> bool {
    POWER_DONE.reset();
    POWER_REQUEST.signal(sleep);
    POWER_DONE.wait().await
}

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
    /// its urgency is carried over so a touch response is not delayed.
    pub fn submit(&mut self, mut job: PlaneJob) {
        if let Some(stale) = JOBS.try_take() {
            job.urgent |= stale.urgent;
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
/// to the core whose interrupt handlers were installed. Panel initialization
/// and its interrupt executor both stay on Core0. Core1 only submits canvas
/// leases; the driver never crosses a core.
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

    #[cfg(feature = "embedscope")]
    let mut final_capture = crate::embedscope::FinalCapture::new();
    let mut applied_brightness: Option<u8> = None;
    let mut sleeping = false;
    let mut full_after_wake = false;
    let mut restored_cycle = 0;

    loop {
        #[cfg(feature = "embedscope")]
        let next = select(
            POWER_REQUEST.wait(),
            select(
                BRIGHTNESS_CHANGED.wait(),
                select(
                    crate::embedscope::FinalCapture::request(),
                    display.next_job(),
                ),
            ),
        )
        .await;
        #[cfg(not(feature = "embedscope"))]
        let next = select(
            POWER_REQUEST.wait(),
            select(BRIGHTNESS_CHANGED.wait(), display.next_job()),
        )
        .await;
        let mut job = match next {
            Either::First(sleep) => {
                // Return every queued canvas lease before acknowledging standby.
                JOBS.reset();
                let cycle = crate::services::power::coordinator::cycle();
                let result = if sleep {
                    panel.sleep_in().await
                } else {
                    panel.sleep_out().await
                };
                // Failed entry/resume leaves panel state uncertain. Suppress all
                // uploads until the coordinator explicitly restores it.
                sleeping = sleep || result.is_err();
                if !sleep && result.is_ok() {
                    restored_cycle = cycle;
                    crate::services::power::coordinator::panel_ready(cycle);
                    full_after_wake = true;
                    applied_brightness = None;
                }
                if let Err(error) = &result {
                    warn!("plane: standby={sleep} failed: {error:?}");
                }
                POWER_DONE.signal(result.is_ok());
                continue;
            }
            Either::Second(Either::First(())) => {
                if !sleeping && !full_after_wake && FIRST_FRAME.load(Ordering::Acquire) {
                    apply_brightness(&mut panel, &mut applied_brightness, restored_cycle).await;
                }
                continue;
            }
            #[cfg(not(feature = "embedscope"))]
            Either::Second(Either::Second(job)) => job,
            #[cfg(feature = "embedscope")]
            Either::Second(Either::Second(Either::First(request))) => {
                final_capture.serve(request);
                continue;
            }
            #[cfg(feature = "embedscope")]
            Either::Second(Either::Second(Either::Second(job))) => job,
        };
        if sleeping {
            continue;
        }
        if full_after_wake {
            job.damage = None;
            job.clear_outside = matches!(job.canvas, Frame::Pool(_));
        }

        if !job.urgent {
            let _ = select(te.wait_for_rising_edge(), Timer::after(TE_TIMEOUT)).await;
        }
        #[cfg(feature = "embedscope")]
        let upload_started = embassy_time::Instant::now();
        let mut completed = true;

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
                        Ok(()) => {}
                        Err(error) => {
                            warn!("plane: slide {dst:?} failed: {error:?}");
                            completed = false;
                        }
                    }
                    y = y2 + 1;
                }
            }
        }

        let slide = matches!(job.canvas, Frame::Slide { .. });
        if completed && !slide {
            completed = upload(
                &mut panel,
                job.canvas.pixels(),
                job.size,
                (origin_x, origin_y),
                area,
            )
            .await;
        }
        #[cfg(feature = "embedscope")]
        if completed {
            // Copy while the original leases still protect every source, then
            // release them normally. USB capture never retains those leases.
            if job.clear_outside {
                final_capture.clear_outside((origin_x, origin_y), job.size);
            }
            let source = match &job.canvas {
                Frame::Pool(lease) => {
                    final_capture.blit(
                        lease.pixels(),
                        canvas_w as usize,
                        area,
                        (origin_x + area.x0, origin_y + area.y0),
                    );
                    "Home"
                }
                Frame::Screen(lease) => {
                    final_capture.blit(
                        lease.pixels(),
                        canvas_w as usize,
                        area,
                        (origin_x + area.x0, origin_y + area.y0),
                    );
                    "Screen"
                }
                Frame::Slide {
                    new,
                    old,
                    direction,
                    offset,
                } => {
                    let (bands, count) = transition::bands(*direction, *offset, WIDTH as u16);
                    for band in bands.iter().take(count).flatten() {
                        let pixels = match band.source {
                            Source::New => new.pixels(),
                            Source::Old => old.pixels(),
                        };
                        final_capture.blit(
                            pixels,
                            WIDTH,
                            DirtyRect {
                                x0: band.src_x,
                                y0: 0,
                                x1: band.src_x + band.width - 1,
                                y1: HEIGHT as u16 - 1,
                            },
                            (band.dst_x, 0),
                        );
                    }
                    "Slide"
                }
            };
            final_capture.complete(
                upload_started
                    .elapsed()
                    .as_millis()
                    .max(1)
                    .min(u32::MAX as u64) as u32,
                source,
            );
        }

        crate::services::power::button_task::frame_uploaded(job.shutdown_sleep, completed);
        if job.shutdown_low_battery {
            crate::services::power::button_task::warning_uploaded(completed);
        }
        if completed {
            if full_after_wake {
                crate::services::power::coordinator::frame_done(restored_cycle);
            }
            full_after_wake = false;
        }

        if completed {
            FIRST_FRAME.store(true, Ordering::Release);
        }

        if completed {
            apply_brightness(&mut panel, &mut applied_brightness, restored_cycle).await;
        }

        // Return the slot to the renderer after the completed transfer.
        drop(job);
    }
}

async fn apply_brightness(
    panel: &mut Display<esp_hal::Async>,
    applied: &mut Option<u8>,
    cycle: u32,
) {
    let target = BRIGHTNESS_TARGET.load(Ordering::Relaxed);
    if *applied == Some(target) {
        return;
    }
    match panel.set_brightness_async(target).await {
        Ok(()) => {
            *applied = Some(target);
            BRIGHTNESS_APPLIED.store(target, Ordering::Release);
            crate::services::power::coordinator::brightness_done(cycle);
            if !BRIGHTNESS_RESTORED.swap(true, Ordering::Relaxed) {
                info!("plane: brightness restored after the first canvas");
            } else {
                info!("plane: brightness {target}%");
            }
        }
        Err(error) => warn!("plane: brightness {target}% failed: {error:?}"),
    }
}

/// Transfer one rectangle in bounded bursts, retrying a failed burst once.
async fn upload(
    panel: &mut Display<esp_hal::Async>,
    pixels: &[u16],
    size: (u16, u16),
    at: (u16, u16),
    area: DirtyRect,
) -> bool {
    let mut y = area.y0;
    while y <= area.y1 {
        let y2 = (y + CHUNK_ROWS - 1).min(area.y1);
        let dst = DirtyRect {
            x0: at.0 + area.x0,
            y0: at.1 + y,
            x1: at.0 + area.x1,
            y1: at.1 + y2,
        };
        let src = DirtyRect {
            y0: y,
            y1: y2,
            ..area
        };
        let mut result = panel.write_canvas_async(dst, src, size, pixels).await;
        if result.is_err() {
            result = panel.write_canvas_async(dst, src, size, pixels).await;
        }
        if let Err(error) = result {
            warn!("plane: canvas {dst:?} failed: {error:?}");
            return false;
        }
        y = y2 + 1;
    }
    true
}

/// Top-left of a centred viewport. Character placement is part of rendering.
fn origin((w, h): (u16, u16)) -> (u16, u16) {
    let x = (WIDTH as u16 - w) / 2;
    let y = (HEIGHT as u16 - h) / 2;
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
