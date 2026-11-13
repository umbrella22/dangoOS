//! Structured fan-out before the existing console's text formatting.
use alloc::boxed::Box;
use core::{
    fmt::{self, Write},
    sync::atomic::{AtomicU32, Ordering},
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, once_lock::OnceLock,
};

pub(super) struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Text<N> {
    fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut count = text.len().min(N - self.len);
        while !text.is_char_boundary(count) {
            count -= 1;
        }
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}
pub(super) struct Record {
    pub at_ms: u64,
    pub level: u8,
    pub target: Text<64>,
    pub message: Text<256>,
}
type LogQueue = Channel<CriticalSectionRawMutex, Record, 16>;
static LOGS: OnceLock<&'static LogQueue> = OnceLock::new();
pub(super) fn init() {
    assert!(LOGS.init(Box::leak(Box::new(LogQueue::new()))).is_ok());
}
pub(super) fn queue() -> &'static LogQueue {
    LOGS.try_get()
        .expect("EmbedScope log queue initialized at boot")
}
pub(super) static DROPPED: AtomicU32 = AtomicU32::new(0);
pub(crate) fn record(record: &log::Record<'_>) {
    if !super::transport::connected() {
        return;
    }
    let mut target = Text::new();
    let _ = target.write_str(record.target());
    let mut message = Text::new();
    let _ = write!(message, "{}", record.args());
    let level = match record.level() {
        log::Level::Error => 1,
        log::Level::Warn => 2,
        log::Level::Info => 3,
        log::Level::Debug => 4,
        log::Level::Trace => 5,
    };
    let item = Record {
        at_ms: crate::platform::runtime::time::now_us() / 1000,
        level,
        target,
        message,
    };
    if queue().try_send(item).is_err() {
        let mut old = DROPPED.load(Ordering::Relaxed);
        loop {
            match DROPPED.compare_exchange_weak(
                old,
                old.saturating_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(value) => old = value,
            }
        }
    }
}
