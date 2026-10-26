//! Display telemetry shared by the panel plane, the UI task and the console.
//!
//! Written by the firmware's plane task (which owns the CO5300 panel) and read by
//! the UI stats line and the boot status record. Every value is a relaxed
//! atomic: these are counters for humans, not synchronization. Frame ownership
//! between the renderer and the upload is handled by
//! [`crate::ui::canvas_pool`], not by flags in this module.

use core::sync::atomic::{AtomicBool, AtomicU32};

/// Uploaded plane frames, never reset.
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
/// Set once the first frame has been presented and released.
pub static FIRST_FRAME: AtomicBool = AtomicBool::new(false);
/// Set once the panel brightness has been restored after the first frame.
///
/// `display::init` leaves the panel at brightness 0 so bootloader leftovers
/// cannot flash before the UI has painted; this flag records the restore.
pub static BRIGHTNESS_RESTORED: AtomicBool = AtomicBool::new(false);

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
