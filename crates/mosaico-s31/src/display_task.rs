//! Asynchronous panel presentation task.
//!
//! `display_task` exclusively owns the CO5300 QSPI driver. It waits for a
//! [`FrameJob`] from the presenter, splits the (already 4-pixel-aligned)
//! damage into `MAX_CHUNK_ROWS` (32-row) windows, transfers each window from
//! the published front frame with `write_chunk_async`, and releases the frame
//! so the UI task can reuse it.
//!
//! The loop performs no allocation and never calls LVGL. A failed chunk is
//! retried once; if the retry also fails the rest of the frame is dropped and
//! the frame is released anyway, so the presenter can never deadlock. The next
//! UI refresh invalidates the full screen, which repairs any partial transfer.
//!
//! The first chunk of a *tagged* job calls [`latency::note_first_pixel`] with
//! the job's own touch timestamp, converting exactly the response frame into
//! one P95 sample. Untagged animation frames never consume a sample.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Input;
use log::{info, warn};

use crate::{
    drivers::display::{Display, MAX_CHUNK_ROWS},
    framebuffer::DirtyRect,
    latency,
    present::{DisplayPresenter, FrameJob},
};

/// Presented jobs (including empty-damage submissions), never reset.
pub static FRAMES: AtomicU32 = AtomicU32::new(0);
/// Transfer duration of the most recent job, in microseconds.
pub static LAST_TRANSFER_US: AtomicU32 = AtomicU32::new(0);
/// Average transfer duration over the last FPS window, in microseconds.
pub static AVG_TRANSFER_US: AtomicU32 = AtomicU32::new(0);
/// Transfer duration of the most recent job, in milliseconds.
pub static LAST_TRANSFER_MS: AtomicU32 = AtomicU32::new(0);
/// Longest observed job transfer, in milliseconds.
pub static MAX_TRANSFER_MS: AtomicU32 = AtomicU32::new(0);
/// Frames per second measured over the last roughly one-second window.
pub static FPS: AtomicU32 = AtomicU32::new(0);
/// Tagged jobs that started a transfer (one latency sample each), never reset.
pub static TOUCH_FRAMES: AtomicU32 = AtomicU32::new(0);
/// Tagged jobs with no damage, which therefore produced no sample. Never reset.
pub static TOUCH_EMPTY: AtomicU32 = AtomicU32::new(0);
/// Set once the first frame has been presented and released.
pub static FIRST_FRAME: AtomicBool = AtomicBool::new(false);
/// Set once the panel brightness has been restored after the first frame.
///
/// `display::init` leaves the panel at brightness 0 so bootloader leftovers
/// cannot flash before the UI has painted; this flag records the restore.
pub static BRIGHTNESS_RESTORED: AtomicBool = AtomicBool::new(false);

/// Panel brightness applied once the first frame is on the glass.
const PANEL_BRIGHTNESS_PCT: u8 = 100;

/// Animation frames that started on a TE edge. Never reset.
pub static TE_SYNCS: AtomicU32 = AtomicU32::new(0);
/// Reserved for a future TE-supervised transfer. Kept so the console format
/// stays stable; zero is expected when the TE signal is present.
pub static TE_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
/// Touch frames dispatched without waiting for a frame boundary. Never reset.
pub static TE_BYPASS: AtomicU32 = AtomicU32::new(0);
/// Animation frames released so a newer queued job could run first. Never reset.
pub static TE_YIELDS: AtomicU32 = AtomicU32::new(0);
/// Exponentially averaged wait from job-ready to the TE edge, in microseconds.
///
/// A uniform 0..frame interval has a mean of half the panel period: ~8.3 ms at
/// 60 Hz, ~4.2 ms at 120 Hz. This is how the console tells the actual panel
/// rate without a second task competing for the TE pin.
pub static TE_WAIT_AVG_US: AtomicU32 = AtomicU32::new(0);
/// True from the moment a job is selected until its frame is released.
///
/// The UI task checks this before starting a repaint. Without it the UI's
/// ~18 ms raster can run between the display task's chunk DMA awaits and
/// stretch a 9 ms transfer to 37-39 ms — three panel frames, which tears no
/// matter how the transfer is aligned.
pub static DISPLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
/// True while the UI task is inside `Ui::tick` or `ui.timer_handler`.
///
/// The display task waits for this to clear before arming the TE edge, so a
/// transfer that begins on a frame boundary is not immediately preempted by a
/// raster that was already in flight.
pub static UI_RENDERING: AtomicBool = AtomicBool::new(false);
/// Longest wait for a frame edge before transferring anyway.
///
/// The panel frame is 16.7 ms. A wait longer than that means the TE signal is
/// missing or stuck; transferring late is better than stalling the display and
/// the UI behind a frame that never starts.
const TE_TIMEOUT: Duration = Duration::from_millis(30);

/// Wait for work, then start an animation transfer on the next panel frame.
///
/// The CO5300 drives its TE signal on `GPIO43`; the vendor init already turns
/// the signal on with `0x35 0x00` (V-blank only). The panel scans out at 60 Hz
/// while a ball frame takes ~12 ms of QSPI. If a transfer starts in the middle
/// of a scanout it straddles the boundary and the ball is briefly drawn in two
/// places — the tearing seen while it bounces or is dragged.
///
/// # Why the wait comes after the job, not before it
///
/// An earlier version armed the edge wait while the display task was idle and
/// then blocked for a job. The edge almost always fired first, so the transfer
/// still started at an arbitrary phase; `te=` counted edges, not aligned
/// transfers. This version waits for the job first, then arms the edge. The
/// wait is bounded by [`TE_TIMEOUT`] (30 ms) and the display task has ~35 ms of
/// slack at 19 fps, so it does not lower the frame rate.
///
/// # Touch frames and yielding
///
/// A touch-response frame is dispatched immediately: up to 16.7 ms of latency
/// is worse than a single torn frame. While an animation frame waits for its
/// edge, a newer job may arrive (normally the touch response). The animation
/// frame is released unread and the newer job is served at once, so the edge
/// wait can never delay a touch. [`TE_YIELDS`] records that path.
async fn next_job_at_frame_start(
    te: &mut Input<'static>,
    display: &mut DisplayPresenter,
) -> FrameJob {
    // A queued animation frame is aligned too. Skipping the edge for queued
    // jobs is what produced the remaining torn drag frames: fast drags queue
    // an urgent frame behind the previous animation frame, and the old fast
    // path dispatched it mid-scanout.
    let mut job = match display.try_next_job() {
        Some(job) => job,
        None => display.next_job().await,
    };

    loop {
        if job.touch_at_us.is_some() {
            // Latency wins over alignment for the response frame.
            TE_BYPASS.fetch_add(1, Ordering::Relaxed);
            return job;
        }

        // A raster that was already running when the job arrived has to
        // finish; arming TE before it would only move the edge under it.
        while UI_RENDERING.load(Ordering::Relaxed) {
            Timer::after(Duration::from_micros(200)).await;
        }

        let wait_started = Instant::now();
        let next = select(
            te.wait_for_rising_edge(),
            select(display.next_job(), Timer::after(TE_TIMEOUT)),
        )
        .await;
        match next {
            Either::First(()) => {
                // A raster may have started between the check above and this
                // edge. Returning now would let it interleave with the chunk
                // DMA waits, so miss this frame and arm the next edge instead.
                if UI_RENDERING.load(Ordering::Relaxed) {
                    continue;
                }
                // Top of a panel frame: the whole region is written before the
                // scanout reaches it. The caller sets `DISPLAY_ACTIVE` before
                // its first await, so no repaint can start now.
                let wait_us = wait_started.elapsed().as_micros().min(u32::MAX as u64) as u32;
                let old = TE_WAIT_AVG_US.load(Ordering::Relaxed);
                TE_WAIT_AVG_US.store(
                    if old == 0 { wait_us } else { (old * 7 + wait_us) / 8 },
                    Ordering::Relaxed,
                );
                TE_SYNCS.fetch_add(1, Ordering::Relaxed);
                return job;
            }
            Either::Second(Either::First(newer)) => {
                // Drop the unread animation frame so the UI can reuse it and
                // serve the newer job without waiting for the edge.
                display.release(job.id);
                TE_YIELDS.fetch_add(1, Ordering::Relaxed);
                job = newer;
            }
            Either::Second(Either::Second(())) => {
                // No edge within a panel frame: the signal is missing. Move the
                // frame anyway instead of stalling the display and the UI.
                TE_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                return job;
            }
        }
    }
}

/// One flush-log interval, in presented jobs.
const LOG_EVERY_FRAMES: u32 = 120;
/// FPS measurement window.
const FPS_WINDOW_US: u64 = 1_000_000;

#[embassy_executor::task]
pub async fn display_task(
    mut panel: Display<esp_hal::Async>,
    mut display: DisplayPresenter,
    mut te: Input<'static>,
) {
    info!(
        "display_task: async QSPI presenter ready ({} row chunks)",
        MAX_CHUNK_ROWS
    );

    let mut window_start = Instant::now();
    let mut window_frames: u32 = 0;
    let mut window_transfer_us: u64 = 0;

    loop {
        let job = next_job_at_frame_start(&mut te, &mut display).await;
        // The job is aligned (animation) or latency-critical (touch); hold the
        // UI off only for the upload itself. The caller is synchronous between
        // the return above and this store, so a repaint cannot slip in.
        DISPLAY_ACTIVE.store(true, Ordering::Relaxed);
        let damage = job.damage;
        let touch_at_us = job.touch_at_us;
        let started = Instant::now();
        let mut chunks: u32 = 0;
        let mut completed = true;

        if damage.is_empty() {
            // Nothing to transfer; still complete the handshake. A tagged job
            // with no damage produces no sample: no QSPI byte was sent.
            if touch_at_us.is_some() {
                TOUCH_EMPTY.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            let mut first_chunk = true;
            let rows = MAX_CHUNK_ROWS;
            let mut y = damage.y0;
            while y <= damage.y1 {
                let y2 = core::cmp::min(y.saturating_add(rows - 1), damage.y1);
                let chunk = DirtyRect {
                    x0: damage.x0,
                    y0: y,
                    x1: damage.x1,
                    y1: y2,
                };
                if first_chunk {
                    // Only the job that carries the tag can produce a sample;
                    // older animation frames cannot consume it.
                    if let Some(at_us) = touch_at_us {
                        latency::note_first_pixel(at_us);
                        TOUCH_FRAMES.fetch_add(1, Ordering::Relaxed);
                    }
                    first_chunk = false;
                }

                let mut result = panel
                    .write_chunk_async(chunk, display.front_pixels(job.id))
                    .await;
                if result.is_err() {
                    warn!(
                        "display_task: frame {:?} chunk ({},{})-({},{}) failed: {:?}; retrying",
                        job.id, chunk.x0, chunk.y0, chunk.x1, chunk.y1, result
                    );
                    result = panel
                        .write_chunk_async(chunk, display.front_pixels(job.id))
                        .await;
                }
                match result {
                    Ok(()) => chunks += 1,
                    Err(error) => {
                        warn!(
                            "display_task: frame {:?} retry failed: {error:?}; dropping remainder",
                            job.id
                        );
                        completed = false;
                        break;
                    }
                }
                y = y2 + 1;
            }
        }

        let elapsed_us = started.elapsed().as_micros();
        let elapsed_ms = (elapsed_us / 1000) as u32;
        LAST_TRANSFER_US.store(elapsed_us.min(u32::MAX as u64) as u32, Ordering::Relaxed);
        LAST_TRANSFER_MS.store(elapsed_ms, Ordering::Relaxed);
        MAX_TRANSFER_MS.fetch_max(elapsed_ms, Ordering::Relaxed);
        let frames = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
        FIRST_FRAME.store(true, Ordering::Relaxed);

        // `display::init` holds the panel at brightness 0 so nothing left in
        // GRAM can flash during boot. Once a frame with real pixels has been
        // transferred, lighting the panel shows our own content and nothing
        // else. An empty-damage job does not count: it put no pixels on the
        // glass.
        if !damage.is_empty() && !BRIGHTNESS_RESTORED.load(Ordering::Relaxed) {
            match panel.set_brightness_async(PANEL_BRIGHTNESS_PCT).await {
                Ok(()) => {
                    BRIGHTNESS_RESTORED.store(true, Ordering::Relaxed);
                    info!(
                        "display_task: panel brightness restored to {PANEL_BRIGHTNESS_PCT}% after the first frame"
                    );
                }
                Err(error) => warn!("display_task: brightness restore failed: {error:?}"),
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

        // Release even after an error: the presenter must never starve.
        display.release(job.id);
        DISPLAY_ACTIVE.store(false, Ordering::Relaxed);

        if frames == 1 || frames.is_multiple_of(LOG_EVERY_FRAMES) || !completed {
            info!(
                "display_task: frame={frames} id={:?} damage=({},{})-({},{}) chunks={chunks} transfer={elapsed_us}us ok={completed}",
                job.id, damage.x0, damage.y0, damage.x1, damage.y1
            );
        }
    }
}
