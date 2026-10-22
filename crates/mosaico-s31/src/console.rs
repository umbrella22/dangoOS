//! USB CDC-ACM console: a bounded log queue and the pump that drains it.
//!
//! Every task logs through [`install_logger`]'s `log::Log` implementation,
//! which formats into a fixed 160-byte record and drops it when the queue is
//! full, so logging never blocks a task. [`pump`] waits for the host to open
//! the port, replays a status line, then forwards records; a status line every
//! [`CONSOLE_STATUS_PERIOD`] makes a dropped connection visible.
//!
//! The other direction carries the app push protocol
//! (`mosaico_s31::hub::push`): [`receive`] feeds host bytes to it and queues
//! its `@ok`/`@err` replies with the log lines.

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicU32, Ordering},
};

use alloc::vec::Vec;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use embassy_usb::{
    class::cdc_acm::{Receiver, Sender},
    driver::EndpointError,
};
use esp_hal::usb::otg::embassy_usb_device::Driver;
use log::{info, warn};
use mosaico_s31::hub::push::{Effect, Session};
use mosaico_s31::telemetry::{display as display_stats, latency};

use crate::tasks::{apps, input, ui};

/// Fixed capacity of one console line, including the trailing CRLF.
const LOG_LINE_BYTES: usize = 160;
/// Text capacity leaves room for the two-byte CRLF terminator.
const LOG_TEXT_BYTES: usize = LOG_LINE_BYTES - 2;
/// Bounded log queue. New records are dropped when the CDC pump is behind.
const LOG_CHANNEL_DEPTH: usize = 96;
/// How often the console pump reports connection state and drop counters.
const CONSOLE_STATUS_PERIOD: Duration = Duration::from_secs(5);

/// One formatted log line owned by the channel.
struct LogRecord {
    reboot_fence: bool,
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
static LOG_CHANNEL: Channel<CriticalSectionRawMutex, LogRecord, LOG_CHANNEL_DEPTH> = Channel::new();
/// The host has the port open.
static CONNECTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
// Only the OTA reboot acknowledgement carries this fence.
static REBOOT_ACK_SENT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

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
pub async fn pump<'a>(class: &mut Sender<'a, Driver<'a>>) {
    loop {
        class.wait_connection().await;
        // Hold the queue until a host opens the port (DTR), so boot lines
        // are not sent into a port nobody reads.
        while !class.dtr() {
            Timer::after_millis(20).await;
        }
        CONNECTED.store(true, Ordering::Relaxed);
        let mut next_status = embassy_time::Instant::now() + CONSOLE_STATUS_PERIOD;
        let status = status_record(true);
        let _ = class
            .write_packet(&status.bytes[..status.len as usize])
            .await;

        loop {
            let result = match select(LOG_CHANNEL.receive(), Timer::at(next_status)).await {
                Either::First(record) => {
                    let result = class
                        .write_packet(&record.bytes[..record.len as usize])
                        .await;
                    if result.is_ok() && record.reboot_fence {
                        REBOOT_ACK_SENT.store(true, Ordering::Release);
                    }
                    result
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
                    CONNECTED.store(false, Ordering::Relaxed);
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

/// Queue one protocol reply. Unlike log records it waits for room: the host
/// blocks on every reply.
async fn reply(text: &str) {
    for text in text.lines() {
        let mut line = LogLine::new();
        let _ = line.write_str(text);
        LOG_CHANNEL.send(line.finish()).await;
    }
}

/// Bounded queue+send fence. Endpoint completion is transport delivery, not a
/// claim that the host process parsed the ACK. Timeout leaves activation intact.
async fn reboot_reply_sent(text: &str) -> bool {
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
        match mosaico_s31::ota::reboot_ack(
            REBOOT_ACK_SENT.load(Ordering::Acquire),
            start.elapsed().as_millis(),
        ) {
            mosaico_s31::ota::RebootAck::Sent => return true,
            mosaico_s31::ota::RebootAck::TimedOut => return false,
            mosaico_s31::ota::RebootAck::Waiting => Timer::after_millis(5).await,
        }
    }
}

/// Feed host bytes to the app push protocol and carry out its effects.
///
/// File writes happen inside `feed`, synchronously; meanwhile the USB core
/// NAKs further OUT packets, which is the flow control.
pub async fn receive<'a>(class: &mut Receiver<'a, Driver<'a>>) {
    let mut buf = [0u8; 512];
    let mut session = Session::new();
    let mut ota = crate::ota_runtime::DeviceUpdater::new();
    let mut no_mounts = mosaico_fs::Vfs::new();
    loop {
        class.wait_connection().await;
        loop {
            let n = match class.read_packet(&mut buf).await {
                Ok(n) => n,
                Err(EndpointError::Disabled) => break,
                Err(EndpointError::BufferOverflow) => continue,
            };
            // Preserve whole USB payload chunks, stopping only at a deferred
            // command boundary before processing the packet's remaining bytes.
            let mut consumed = 0;
            while consumed < n {
                session.set_management_busy(apps::is_managing());
                let mut effects = Vec::new();
                let fed = mosaico_fs::global::with(|vfs| {
                    Ok(session
                        .feed_until_deferred(vfs, &buf[consumed..n], &mut |e| effects.push(e)))
                });
                consumed += match fed {
                    Ok(n) => n,
                    Err(_) => {
                        session.feed_until_deferred(&mut no_mounts, &buf[consumed..n], &mut |e| {
                            effects.push(e)
                        })
                    }
                };
                for effect in effects {
                    match effect {
                        Effect::OtaCancel => {
                            ota.abort();
                            session.set_ota_active(false);
                        }
                        Effect::Ota(command) => {
                            let result = ota.execute(command).await;
                            session.set_ota_active(crate::ota_runtime::active());
                            match result {
                                Ok((text, reboot)) => {
                                    if reboot {
                                        if reboot_reply_sent(&text).await {
                                            esp_hal::system::software_reset();
                                        } else {
                                            // The candidate is selected, but no successful ACK
                                            // delivery is established: leave reboot to the operator.
                                            warn!("ota: reboot ACK delivery timed out; candidate selected, automatic reset withheld");
                                            ota.abort();
                                            session.set_ota_active(false);
                                        }
                                    } else {
                                        reply(&text).await;
                                    }
                                }
                                Err(text) => reply(&text).await,
                            }
                        }
                        Effect::Manage(op) => {
                            let id = op.identity();
                            let result = apps::manage(op).await;
                            let text = match result {
                                Ok(()) => alloc::format!("@ok {} {}", op.verb(), id.id()),
                                Err(e) => alloc::format!("@err {} {}: {e}", op.verb(), id.id()),
                            };
                            reply(&text).await;
                        }
                        Effect::Reply(text) => reply(&text).await,
                        Effect::Reload { run } => {
                            info!(
                                "push: reload{}",
                                run.as_deref()
                                    .map(|id| alloc::format!(", run {id}"))
                                    .unwrap_or_default()
                            );
                            apps::request(true, run.map(apps::Action::Run));
                        }
                        Effect::Run(id) => apps::request(false, Some(apps::Action::Run(id))),
                        Effect::Stop => apps::request(false, Some(apps::Action::Stop)),
                        Effect::SkinsChanged => crate::tasks::skins::request_reload(),
                        Effect::Brain(action) => {
                            let verb = action.verb();
                            let text = match crate::tasks::brain_control::execute(action).await {
                                Ok(status) => alloc::format!("@ok {verb} {status}"),
                                Err(e) => alloc::format!(
                                    "@err {verb}: {}",
                                    mosaico_s31::hub::brain_store::error_line(&e)
                                ),
                            };
                            reply(text.trim_end()).await;
                        }
                        Effect::Reboot => {
                            warn!("push: reboot requested");
                            // Let the reply and the log line go out first.
                            Timer::after_millis(300).await;
                            esp_hal::system::software_reset();
                        }
                    }
                }
            }
        }
        ota.abort();
        // The host went away mid-push: drop the staged files.
        let _ = mosaico_fs::global::with(|vfs| {
            session.reset(vfs);
            Ok(())
        });
    }
}
