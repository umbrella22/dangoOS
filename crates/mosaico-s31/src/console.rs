//! USB CDC-ACM console: a bounded log queue and the pump that drains it.
//!
//! Every task logs through [`install_logger`]'s `log::Log` implementation,
//! which formats into a fixed 160-byte record and drops it when the queue is
//! full, so logging never blocks a task. [`pump`] waits for the host to open
//! the port, replays a status line, then forwards records; a status line every
//! [`CONSOLE_STATUS_PERIOD`] makes a dropped connection visible.

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use embassy_usb::{class::cdc_acm::CdcAcmClass, driver::EndpointError};
use esp_hal::usb::otg::embassy_usb_device::Driver;
use mosaico_s31::telemetry::{display as display_stats, latency};

use crate::tasks::{input, ui};

/// Fixed capacity of one console line, including the trailing CRLF.
const LOG_LINE_BYTES: usize = 160;
/// Text capacity leaves room for the two-byte CRLF terminator.
const LOG_TEXT_BYTES: usize = LOG_LINE_BYTES - 2;
/// Bounded log queue. New records are dropped when the CDC pump is behind.
const LOG_CHANNEL_DEPTH: usize = 32;
/// How often the console pump reports connection state and drop counters.
const CONSOLE_STATUS_PERIOD: Duration = Duration::from_secs(5);

/// One formatted log line owned by the channel.
struct LogRecord {
    len: u8,
    bytes: [u8; LOG_LINE_BYTES],
}

/// A fixed-buffer `core::fmt::Write` sink. Formatting truncates at
/// `LOG_TEXT_BYTES`; the trailing CRLF is always appended by `finish`.
struct LogLine {
    bytes: [u8; LOG_LINE_BYTES],
    len: usize,
}

impl LogLine {
    const fn new() -> Self {
        Self {
            bytes: [0; LOG_LINE_BYTES],
            len: 0,
        }
    }

    fn finish(mut self) -> LogRecord {
        self.bytes[self.len] = b'\r';
        self.bytes[self.len + 1] = b'\n';
        self.len += 2;
        LogRecord {
            len: self.len as u8,
            bytes: self.bytes,
        }
    }
}

impl core::fmt::Write for LogLine {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = LOG_TEXT_BYTES.saturating_sub(self.len);
        let count = core::cmp::min(available, text.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

/// The bounded queue between `log` producers (all tasks) and the USB pump.
static LOG_CHANNEL: Channel<CriticalSectionRawMutex, LogRecord, LOG_CHANNEL_DEPTH> = Channel::new();
/// Records dropped because the bounded queue was full. Never reset.
static LOG_DROPS: AtomicU32 = AtomicU32::new(0);

/// `log::Log` implementation that formats into a fixed 160-byte record.
struct UsbLogger;

impl log::Log for UsbLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut line = LogLine::new();
        let _ = write!(line, "[{}] {}", record.level(), record.args());
        if LOG_CHANNEL.try_send(line.finish()).is_err() {
            LOG_DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {}
}

static USB_LOGGER: UsbLogger = UsbLogger;

/// Route every `log` record into the bounded console queue.
///
/// Replaces the default esp-println logger: its output goes to
/// USB-Serial-JTAG, which is not wired to the USB-C connector on this board.
pub fn install_logger() {
    let _ = log::set_logger(&USB_LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

/// One status line for the CDC port.
fn status_record(connected: bool) -> LogRecord {
    let (samples, p50, p95, max) = latency::summary();
    let mut line = LogLine::new();
    let _ = write!(
        line,
        "[boot] R1 cdc={connected} disp={} br={} frames={} p50={p50} p95={p95} max={max} n={samples} transfer_ms={} fps={} te={}/{}/{} ty={} tew={} q={} drops={} hb={} ldrop={}",
        display_stats::FIRST_FRAME.load(Ordering::Relaxed) as u8,
        display_stats::BRIGHTNESS_RESTORED.load(Ordering::Relaxed) as u8,
        display_stats::FRAMES.load(Ordering::Relaxed),
        display_stats::LAST_TRANSFER_MS.load(Ordering::Relaxed),
        display_stats::FPS.load(Ordering::Relaxed),
        display_stats::TE_SYNCS.load(Ordering::Relaxed),
        display_stats::TE_TIMEOUTS.load(Ordering::Relaxed),
        display_stats::TE_BYPASS.load(Ordering::Relaxed),
        display_stats::TE_YIELDS.load(Ordering::Relaxed),
        display_stats::TE_WAIT_AVG_US.load(Ordering::Relaxed) / 1000,
        input::INPUT.len(),
        input::INPUT_DROPS.load(Ordering::Relaxed),
        ui::HEARTBEATS.load(Ordering::Relaxed),
        LOG_DROPS.load(Ordering::Relaxed),
    );
    line.finish()
}

/// Pump queued log lines to the CDC-ACM class.
///
/// Waits for the host to open the port, drains the bounded channel, and sends a
/// status line every `CONSOLE_STATUS_PERIOD` so a disconnect is detected even
/// when no logs are produced.
pub async fn pump<'a>(class: &mut CdcAcmClass<'a, Driver<'a>>) {
    loop {
        class.wait_connection().await;
        let mut next_status = embassy_time::Instant::now() + CONSOLE_STATUS_PERIOD;
        let status = status_record(true);
        let _ = class
            .write_packet(&status.bytes[..status.len as usize])
            .await;

        loop {
            let result = match select(LOG_CHANNEL.receive(), Timer::at(next_status)).await {
                Either::First(record) => {
                    class
                        .write_packet(&record.bytes[..record.len as usize])
                        .await
                }
                Either::Second(()) => {
                    next_status += CONSOLE_STATUS_PERIOD;
                    let record = status_record(true);
                    class
                        .write_packet(&record.bytes[..record.len as usize])
                        .await
                }
            };
            match result {
                Ok(()) => {}
                Err(EndpointError::Disabled) => {
                    // Cable removed or bus reset. Wait for the next connection;
                    // queued records stay bounded while disconnected.
                    break;
                }
                Err(EndpointError::BufferOverflow) => {
                    // A single record exceeded the endpoint packet size. Drop it
                    // rather than treating it as a disconnect.
                    LOG_DROPS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}
