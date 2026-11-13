//! The panel plane: uploads the fox canvas at a fixed position.
//!
//! The fox widget composes the character, movement and link icons into a
//! full-panel frame in a two-slot
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
//! `clear_outside`. Every Home frame already restores the complete viewport
//! from one lease, including the background and link icons.
//!
//! The panel brightness follows [`set_brightness`] (Settings); the target is
//! applied after the next completed upload.

use super::damage::Damage;
use board_s31::pixels::PixelSource;
#[cfg(feature = "display-metrics")]
use board_s31::pixels::PANEL_FORMAT;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::services::display::canvas_pool::ReadLease;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;
use log::{info, warn};

use crate::services::display::status::{BRIGHTNESS_RESTORED, FIRST_FRAME};

use crate::platform::drivers::display::Display;
#[cfg(feature = "embedscope")]
use crate::shell::navigation::transition::Source;
use {
    crate::services::display::geometry::DirtyRect, crate::services::display::geometry::HEIGHT,
    crate::services::display::geometry::WIDTH,
};
use {
    crate::services::display::lvgl_port::ScreenLease,
    crate::services::display::lvgl_port::SnapshotLease,
};
use {crate::shell::navigation::transition, crate::shell::navigation::transition::Direction};

/// Longest wait for a TE edge before transferring anyway.
const TE_TIMEOUT: Duration = Duration::from_millis(30);

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
    fn pixels(&self, size: (u16, u16)) -> PixelSource<'_> {
        match self {
            Frame::Pool(lease) => lease.source(size),
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
    /// all of it. Expanded to the panel's four-column, two-row windows here.
    pub damage: Option<Damage>,
    /// Restore the complete Home, including its black background and status.
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
                    (Some(mut a), Some(b)) => {
                        a.merge(b);
                        Some(a)
                    }
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
    let mut pending_screen = Damage::new();
    #[cfg(feature = "display-metrics")]
    super::diagnostics::sample_te(&mut te).await;
    #[cfg(feature = "display-metrics")]
    let mut frame_id = 0u32;

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
        let home = matches!(job.canvas, Frame::Pool(_));
        if full_after_wake {
            job.damage = None;
            job.clear_outside = matches!(job.canvas, Frame::Pool(_));
        }
        if matches!(job.canvas, Frame::Screen(_)) {
            if let Some(ref mut damage) = job.damage {
                damage.merge(pending_screen);
            }
        }

        #[cfg(feature = "display-metrics")]
        let frame_started = embassy_time::Instant::now();
        #[cfg(feature = "display-metrics")]
        let started_us = crate::platform::runtime::time::now_us();
        #[cfg(feature = "display-metrics")]
        let _ = panel.take_stats();
        // Native local updates can bypass phase waiting in the opt-in latency
        // experiment. Whole pages, slides and Home keep their normal TE policy.
        let local = matches!(job.canvas, Frame::Screen(_))
            && job.damage.is_some_and(|d| d.pixels() <= WIDTH * HEIGHT / 2);
        if !job.urgent && !(cfg!(feature = "display-low-latency") && local) {
            let _ = select(te.wait_for_rising_edge(), Timer::after(TE_TIMEOUT)).await;
        }
        #[cfg(feature = "display-metrics")]
        let te_us = frame_started.elapsed().as_micros();
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
        // Home movement can reach any edge of the panel. Publish its complete
        // composed viewport, including icons, in one row-major upload.
        let damage = if home {
            Damage::from(full)
        } else {
            job.damage.unwrap_or_else(|| Damage::from(full))
        };

        if let Frame::Slide {
            new,
            old,
            direction,
            offset,
        } = &job.canvas
        {
            // Compose each row from both pages so chunks reach the panel top to
            // bottom at full width, instead of as two full-height column bands.
            let rows = transition::rows(new.pixels(), old.pixels(), *direction, *offset);
            if let Err(error) = panel.write_rows_async(rows).await {
                warn!("plane: slide offset={offset} failed: {error:?}");
                completed = false;
            }
        }

        let slide = matches!(job.canvas, Frame::Slide { .. });
        if completed && !slide {
            for region in damage.regions() {
                let area = clip(*region, full).covering_panel_window();
                if !area.is_empty() {
                    completed = upload(
                        &mut panel,
                        job.canvas.pixels(job.size),
                        (origin_x, origin_y),
                        area,
                    )
                    .await;
                    if !completed {
                        break;
                    }
                }
            }
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
                    for region in damage.regions() {
                        let area = clip(*region, full).covering_panel_window();
                        final_capture.blit_source(
                            lease.source(job.size),
                            area,
                            (origin_x + area.x0, origin_y + area.y0),
                        );
                    }
                    "Home"
                }
                Frame::Screen(lease) => {
                    for region in damage.regions() {
                        let area = clip(*region, full).covering_panel_window();
                        final_capture.blit_source(
                            lease.pixels(),
                            area,
                            (origin_x + area.x0, origin_y + area.y0),
                        );
                    }
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
                        final_capture.blit_source(
                            pixels,
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
        if matches!(job.canvas, Frame::Screen(_)) {
            if completed {
                pending_screen = Damage::new();
            } else {
                pending_screen.merge(job.damage.unwrap_or_else(Damage::full));
            }
        } else if completed {
            // A cached LVGL page must cover the whole panel after Home or a slide.
            pending_screen = Damage::full();
        }
        #[cfg(feature = "display-metrics")]
        {
            let stats = panel.take_stats();
            frame_id = frame_id.wrapping_add(1);
            let (kind, offset, input) = match &job.canvas {
                Frame::Pool(_) => ("home", 0, None),
                Frame::Screen(lease) => ("screen", 0, Some(lease.input)),
                Frame::Slide { new, offset, .. } => ("slide", *offset, Some(new.input)),
            };
            // Each line fits the console's 158-byte text budget at device uptime.
            info!(
                "display.frame id={} kind={} rgb888={} ok={} start={} end={} te={}",
                frame_id,
                kind,
                PANEL_FORMAT.bytes_per_pixel() == 3,
                completed,
                started_us,
                crate::platform::runtime::time::now_us(),
                te_us
            );
            info!("display.cost id={} bytes={} direct={} windows={} pack={} transfer={} pixel={} offset={}",
                frame_id, stats.bytes, stats.direct_bytes, stats.windows, stats.pack_us,
                stats.transfer_us, stats.first_submit_us, offset);
            if let Some(input) = input.filter(|trace| trace.count != 0) {
                info!(
                    "display.input id={} first={} last={} read={} count={}",
                    frame_id, input.first, input.last, input.read, input.count
                );
            }
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

/// Transfer a rectangle with independently addressed DMA chunks. A retry
/// resends the whole rectangle while its immutable source lease remains live.
async fn upload(
    panel: &mut Display<esp_hal::Async>,
    source: PixelSource<'_>,
    at: (u16, u16),
    area: DirtyRect,
) -> bool {
    let dst = DirtyRect {
        x0: at.0 + area.x0,
        y0: at.1 + area.y0,
        x1: at.0 + area.x1,
        y1: at.1 + area.y1,
    };
    let mut result = panel.write_pixels_async(dst, area, source).await;
    if result.is_err() {
        result = panel.write_pixels_async(dst, area, source).await;
    }
    if let Err(error) = result {
        warn!("plane: canvas {dst:?} failed: {error:?}");
        return false;
    }
    true
}

/// Centre the viewport. Full-panel Home frames start at the panel origin;
/// the renderer already places the sprite within that viewport.
fn origin((w, h): (u16, u16)) -> (u16, u16) {
    ((WIDTH as u16 - w) / 2, (HEIGHT as u16 - h) / 2)
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
