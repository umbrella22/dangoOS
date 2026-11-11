//! USB CDC-ACM console: a bounded log queue and the pump that drains it.
//!
//! Every task logs through [`install_logger`]'s `log::Log` implementation,
//! which formats into a fixed 160-byte record and drops it when the queue is
//! full, so logging never blocks a task. [`pump`] waits for the host to open
//! the port, then forwards records.
//!
//! The other direction carries the app push protocol
//! (`crate::console::protocol`): [`receive`] feeds host bytes to it and queues
//! its `@ok`/`@err` replies with the log lines.

use core::sync::atomic::Ordering;
use esp_hal::usb::otg::embassy_usb_device::Driver;

/// The host has the port open.
static CONNECTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
// Only the OTA reboot acknowledgement carries this fence.
static REBOOT_ACK_SENT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

mod effects;
mod logging;
mod receive;
mod transmit;

pub use logging::install_logger;
pub use receive::receive;
pub use transmit::pump;
