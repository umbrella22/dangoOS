//! Bounded log records, logger installation and protocol-reply fences.
use super::REBOOT_ACK_SENT;
use core::{fmt::Write as _, sync::atomic::Ordering};
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Timer;

/// Fixed capacity of one console line, including the trailing CRLF.
const LOG_LINE_BYTES: usize = 160;
/// Text capacity leaves room for the two-byte CRLF terminator.
const LOG_TEXT_BYTES: usize = LOG_LINE_BYTES - 2;
/// Bounded log queue. New records are dropped when the CDC pump is behind.
const LOG_CHANNEL_DEPTH: usize = 96;

/// One formatted log line owned by the channel.
pub(super) struct LogRecord {
    pub(super) reboot_fence: bool,
    pub(super) len: u8,
    pub(super) bytes: [u8; LOG_LINE_BYTES],
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
            reboot_fence: false,
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
pub(super) static LOG_CHANNEL: Channel<CriticalSectionRawMutex, LogRecord, LOG_CHANNEL_DEPTH> =
    Channel::new();

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
        let _ = LOG_CHANNEL.try_send(line.finish());
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

/// Queue one protocol reply. Unlike log records it waits for room: the host
/// blocks on every reply.
pub(super) async fn reply(text: &str) {
    for text in text.lines() {
        let mut line = LogLine::new();
        let _ = line.write_str(text);

        LOG_CHANNEL.send(line.finish()).await;
    }
}

/// Bounded queue+send fence. Endpoint completion is transport delivery, not a
/// claim that the host process parsed the ACK. Timeout leaves activation intact.
pub(super) async fn reboot_reply_sent(text: &str) -> bool {
    REBOOT_ACK_SENT.store(false, Ordering::Release);
    let mut line = LogLine::new();
    let _ = line.write_str(text);
    let mut record = line.finish();
    record.reboot_fence = true;
    let start = embassy_time::Instant::now();
    if matches!(
        select(LOG_CHANNEL.send(record), Timer::after_secs(3)).await,
        Either::Second(())
    ) {
        return false;
    }
    loop {
        match crate::services::update::ota::reboot_ack(
            REBOOT_ACK_SENT.load(Ordering::Acquire),
            start.elapsed().as_millis(),
        ) {
            crate::services::update::ota::RebootAck::Sent => return true,
            crate::services::update::ota::RebootAck::TimedOut => return false,
            crate::services::update::ota::RebootAck::Waiting => Timer::after_millis(5).await,
        }
    }
}
