//! Default-off USB observations retained across RTC-preserving resets.
//! This module never changes USB configuration, framing, retries or timeouts.
use core::fmt::{self, Write};

const MAGIC: u32 = 0x5553_4431;
const VERSION: u32 = 2;
const RING: usize = 8;
pub const REPORT_LINES: usize = 11 + RING;
const EVENTS: usize = 30;

/// Counter indices and last-phase codes in the bounded retained report.
#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum Event {
    Enabled,
    Disabled,
    Reset,
    Addressed,
    Configured,
    Unconfigured,
    Suspended,
    Resumed,
    RxWait,
    RxReady,
    RxRead,
    RxPacket,
    RxDisabled,
    RxOverflow,
    Feed,
    Fed,
    SessionReset,
    SessionResetOk,
    SessionResetError,
    ReplyWait,
    ReplyQueued,
    TxWait,
    TxReady,
    DtrWait,
    DtrReady,
    TxWrite,
    TxWritten,
    TxDisabled,
    TxOverflow,
    ControlLineState,
}
impl Event {
    fn rare(self) -> bool {
        matches!(
            self,
            Self::Enabled
                | Self::Disabled
                | Self::Reset
                | Self::Addressed
                | Self::Configured
                | Self::Unconfigured
                | Self::Suspended
                | Self::Resumed
                | Self::DtrWait
                | Self::DtrReady
                | Self::RxDisabled
                | Self::RxOverflow
                | Self::TxDisabled
                | Self::TxOverflow
                | Self::SessionReset
                | Self::SessionResetOk
                | Self::SessionResetError
                | Self::ControlLineState
        )
    }
    fn lane(self) -> usize {
        match self {
            Self::Enabled
            | Self::Disabled
            | Self::Reset
            | Self::Addressed
            | Self::Configured
            | Self::Unconfigured
            | Self::Suspended
            | Self::Resumed
            | Self::ControlLineState => 0,
            Self::RxWait
            | Self::RxReady
            | Self::RxRead
            | Self::RxPacket
            | Self::RxDisabled
            | Self::RxOverflow
            | Self::Feed
            | Self::Fed
            | Self::SessionReset
            | Self::SessionResetOk
            | Self::SessionResetError
            | Self::ReplyWait
            | Self::ReplyQueued => 1,
            _ => 2,
        }
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
struct Progress {
    event: u32,
    at_ms: u32,
    value: u32,
    sequence: u32,
}
#[derive(Clone, Copy)]
#[repr(C)]
struct Rare {
    event: u32,
    at_ms: u32,
    value: u32,
    ordinal: u32,
    commit: u32,
}
const EMPTY_RARE: Rare = Rare {
    event: 0,
    at_ms: 0,
    value: 0,
    ordinal: 0,
    commit: 0,
};
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Record {
    magic: u32,
    version: u32,
    bytes: u32,
    epoch: u32,
    sequence: u32,
    counters: [u32; EVENTS],
    progress: [Progress; 3], // bus, RX, TX; the sampler never overwrites these
    rare: [Rare; RING],
    ring_head: u32,
    ring_total: u32,
    report: [u32; 3], // attempted index, successfully completed lines, terminal (0 pending, 1 complete, 2 error)
    rx_bytes: u32,
    tx_bytes: u32,
    sample: [u32; 5], // ticks, low32 monotonic ms, UI heartbeat, plane frames, log drops
}
const EMPTY: Record = Record {
    magic: 0,
    version: 0,
    bytes: 0,
    epoch: 0,
    sequence: 0,
    counters: [0; EVENTS],
    progress: [Progress {
        event: u32::MAX,
        at_ms: 0,
        value: 0,
        sequence: 0,
    }; 3],
    rare: [EMPTY_RARE; RING],
    ring_head: 0,
    ring_total: 0,
    report: [u32::MAX, 0, 0],
    rx_bytes: 0,
    tx_bytes: 0,
    sample: [0; 5],
};
fn store(word: &mut u32, value: u32) {
    // Volatile stores ensure each bounded observation reaches retained RAM.
    unsafe { core::ptr::write_volatile(word, value) };
}
impl Record {
    fn recognized(&self) -> bool {
        self.magic == MAGIC
            && self.version == VERSION
            && self.bytes == core::mem::size_of::<Self>() as u32
    }
    fn fresh(epoch: u32) -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            bytes: core::mem::size_of::<Self>() as u32,
            epoch,
            ..EMPTY
        }
    }
    fn begin(&mut self) -> u32 {
        let done = self.sequence.wrapping_add(2) & !1;
        store(&mut self.sequence, done.wrapping_sub(1));
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        done
    }
    fn end(&mut self, done: u32) {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        store(&mut self.sequence, done);
    }
    fn observe(&mut self, event: Event, at_ms: u32, value: u32) {
        let done = self.begin();
        let count = &mut self.counters[event as usize];
        store(count, count.saturating_add(1));
        let p = &mut self.progress[event.lane()];
        store(&mut p.event, event as u32);
        store(&mut p.at_ms, at_ms);
        store(&mut p.value, value);
        store(&mut p.sequence, done);
        match event {
            Event::RxPacket => {
                let n = self.rx_bytes.saturating_add(value);
                store(&mut self.rx_bytes, n);
            }
            Event::TxWritten => {
                let n = self.tx_bytes.saturating_add(value);
                store(&mut self.tx_bytes, n);
            }
            _ => {}
        }
        if event.rare() {
            let slot = (self.ring_head as usize) % RING;
            let next = self.ring_total.saturating_add(1);
            let r = &mut self.rare[slot];
            store(&mut r.commit, 0);
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            store(&mut r.event, event as u32);
            store(&mut r.at_ms, at_ms);
            store(&mut r.value, value);
            store(&mut r.ordinal, next);
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            store(&mut r.commit, done); // last: zero is never a usable committed entry
            store(&mut self.ring_head, ((slot + 1) % RING) as u32);
            store(&mut self.ring_total, next);
        }
        self.end(done);
    }
    fn report_progress(&mut self, index: u32, completed: u32, terminal: u32) {
        let done = self.begin();
        for (word, value) in self.report.iter_mut().zip([index, completed, terminal]) {
            store(word, value);
        }
        self.end(done);
    }
    fn sample(&mut self, now: u32, ui: u32, frames: u32, drops: u32) {
        let done = self.begin();
        let n = self.sample[0].saturating_add(1);
        store(&mut self.sample[0], n);
        for (word, value) in self.sample[1..].iter_mut().zip([now, ui, frames, drops]) {
            store(word, value);
        }
        self.end(done);
    }
}
/// ASCII formatting is bounded independently of the console logging queue.
/// Every generated line is at most 158 bytes; CRLF is added by the sender.
pub struct Line {
    bytes: [u8; 158],
    len: usize,
}
impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; 158],
            len: 0,
        }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.len + text.len() > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}
/// Fixed report records, including an explicit unavailable header.
/// Counters are grouped by documented Event index; no field is interpreted as
/// a real enum before validation. RESET during an update is labelled partial.
pub fn report_line(record: &Record, index: usize) -> Option<Line> {
    let mut out = Line::new();
    if index == 0 {
        write!(
            out,
            "[usb-prev] valid={} epoch={} seq={} partial={} bytes={}",
            record.recognized() as u8,
            record.epoch,
            record.sequence,
            (record.sequence & 1) as u8,
            record.bytes
        )
        .ok()?;
    } else if !record.recognized() {
        return None;
    } else if index <= 3 {
        let p = record.progress[index - 1];
        write!(
            out,
            "[usb-prev] lane={} event={} ms={} value={} seq={}",
            index - 1,
            p.event,
            p.at_ms,
            p.value,
            p.sequence
        )
        .ok()?;
    } else if index <= 6 {
        let start = (index - 4) * 10;
        write!(out, "[usb-prev] counters{}=", start).ok()?;
        for (i, n) in record.counters[start..start + 10].iter().enumerate() {
            write!(out, "{}{n}", if i == 0 { "" } else { "," }).ok()?;
        }
    } else if index == 7 {
        write!(
            out,
            "[usb-prev] rx_bytes={} tx_bytes={} sample_ticks={} sample_ms={}",
            record.rx_bytes, record.tx_bytes, record.sample[0], record.sample[1]
        )
        .ok()?;
    } else if index == 8 {
        write!(
            out,
            "[usb-prev] ui_hb={} frames={} ldrop={}",
            record.sample[2], record.sample[3], record.sample[4]
        )
        .ok()?;
    } else if index == 9 {
        write!(
            out,
            "[usb-prev] ring_head={} ring_total={} capacity={} truncated={}",
            record.ring_head,
            record.ring_total,
            RING,
            (record.ring_total > RING as u32) as u8
        )
        .ok()?;
    } else if index == 10 {
        write!(
            out,
            "[usb-prev] report_index={} completed={} terminal={}",
            record.report[0], record.report[1], record.report[2]
        )
        .ok()?;
    } else if index < REPORT_LINES {
        let slot = index - 11;
        let r = record.rare[slot];
        write!(
            out,
            "[usb-prev] slot={} event={} ms={} value={} ordinal={} commit={} usable={}",
            slot,
            r.event,
            r.at_ms,
            r.value,
            r.ordinal,
            r.commit,
            (r.commit != 0 && r.commit & 1 == 0) as u8
        )
        .ok()?;
    } else {
        return None;
    }
    Some(out)
}

#[cfg(all(feature = "usb-session-diagnostics", target_os = "none"))]
mod device {
    use super::*;
    use core::ptr::{addr_of, addr_of_mut};
    use core::sync::atomic::{AtomicBool, Ordering};
    // All fields are integers; every RTC power-on bit pattern is representable.
    unsafe impl esp_hal::Persistable for Record {}
    #[esp_hal::ram(unstable(rtc_fast, persistent))]
    static mut USB_RECORD: Record = EMPTY;
    static mut PREVIOUS: Record = EMPTY;
    static REPORTED: AtomicBool = AtomicBool::new(false);
    /// Exactly once, before HAL/task startup. No RTC state controls boot policy.
    pub fn begin_boot() {
        // SAFETY: main calls this before any diagnostic task/handler can run.
        unsafe {
            let old = core::ptr::read_volatile(addr_of!(USB_RECORD));
            core::ptr::write(addr_of_mut!(PREVIOUS), old);
            let epoch = if old.recognized() {
                old.epoch.saturating_add(1)
            } else {
                1
            };
            // Invalidate first and publish the new header last: RESET during
            // initialization reports unavailable rather than a mixed epoch.
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*addr_of_mut!(USB_RECORD)).magic),
                0,
            );
            let mut fresh = Record::fresh(epoch);
            fresh.magic = 0;
            core::ptr::write_volatile(addr_of_mut!(USB_RECORD), fresh);
            core::sync::atomic::compiler_fence(Ordering::SeqCst);
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*addr_of_mut!(USB_RECORD)).magic),
                MAGIC,
            );
        }
    }
    pub fn observe(event: Event, value: u32) {
        let at = embassy_time::Instant::now().as_millis() as u32;
        critical_section::with(|_| {
            // SAFETY: all post-boot accesses are serialized here; no borrow escapes.
            unsafe {
                (&mut *addr_of_mut!(USB_RECORD)).observe(event, at, value);
            }
        });
    }
    pub fn sample(ui: u32, frames: u32, drops: u32) {
        let at = embassy_time::Instant::now().as_millis() as u32;
        critical_section::with(|_| {
            // SAFETY: same single-record access protocol as observe.
            unsafe {
                (&mut *addr_of_mut!(USB_RECORD)).sample(at, ui, frames, drops);
            }
        });
    }
    pub fn report_progress(index: u32, completed: u32, terminal: u32) {
        critical_section::with(|_| unsafe {
            (&mut *addr_of_mut!(USB_RECORD)).report_progress(index, completed, terminal);
        });
    }
    pub fn take_previous() -> Option<Record> {
        if REPORTED.swap(true, Ordering::Relaxed) {
            return None;
        }
        // SAFETY: PREVIOUS was initialized before tasks, and is never modified.
        Some(unsafe { core::ptr::read(addr_of!(PREVIOUS)) })
    }
    pub struct Handler;
    impl embassy_usb::Handler for Handler {
        fn enabled(&mut self, on: bool) {
            observe(if on { Event::Enabled } else { Event::Disabled }, on as u32);
        }
        fn reset(&mut self) {
            observe(Event::Reset, 0);
        }
        fn addressed(&mut self, address: u8) {
            observe(Event::Addressed, address as u32);
        }
        fn configured(&mut self, on: bool) {
            observe(
                if on {
                    Event::Configured
                } else {
                    Event::Unconfigured
                },
                on as u32,
            );
        }
        fn suspended(&mut self, on: bool) {
            observe(
                if on { Event::Suspended } else { Event::Resumed },
                on as u32,
            );
        }
        fn control_out(
            &mut self,
            req: embassy_usb::control::Request,
            _data: &[u8],
        ) -> Option<embassy_usb::control::OutResponse> {
            if req.request_type == embassy_usb::control::RequestType::Class
                && req.recipient == embassy_usb::control::Recipient::Interface
                && req.request == 0x22
            {
                observe(
                    Event::ControlLineState,
                    ((req.index as u32) << 16) | req.value as u32,
                );
            }
            None // Observation only; CDC owns acceptance and DTR semantics.
        }
    }
}
#[cfg(all(feature = "usb-session-diagnostics", target_os = "none"))]
pub use device::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_and_snapshot_validity_do_not_accept_arbitrary_rtc() {
        assert!(!EMPTY.recognized());
        let current = Record::fresh(7);
        assert!(current.recognized());
        let mut wrong = current;
        wrong.version += 1;
        assert!(!wrong.recognized());
        wrong = current;
        wrong.bytes += 4;
        assert!(!wrong.recognized());
        let previous = current;
        let mut next = Record::fresh(previous.epoch + 1);
        next.observe(Event::Reset, 100, 0);
        assert_eq!(previous.epoch, 7);
        assert_eq!(previous.counters[Event::Reset as usize], 0);
        assert_eq!(next.epoch, 8);
        assert_eq!(next.counters[Event::Reset as usize], 1);
        assert!(report_line(&EMPTY, 0).is_some());
        assert!(report_line(&EMPTY, 1).is_none());
    }
    #[test]
    fn pending_phases_completion_saturation_and_sampler_are_independent() {
        let mut r = Record::fresh(1);
        r.observe(Event::RxRead, 10, 0);
        r.observe(Event::TxWrite, 11, 140);
        r.sample(20, 5, 100, 3);
        assert_eq!(r.progress[1].event, Event::RxRead as u32);
        assert_eq!(r.progress[2].event, Event::TxWrite as u32);
        assert_eq!(r.counters[Event::TxWritten as usize], 0);
        r.observe(Event::RxPacket, 21, 512);
        r.observe(Event::TxWritten, 22, 140);
        assert_eq!(r.rx_bytes, 512);
        assert_eq!(r.tx_bytes, 140);
        r.counters[Event::Reset as usize] = u32::MAX;
        r.observe(Event::Reset, 23, 0);
        assert_eq!(r.counters[Event::Reset as usize], u32::MAX);
        assert_eq!(r.sequence & 1, 0);
    }
    #[test]
    fn report_is_fixed_bounded_and_labels_interrupted_updates() {
        let mut r = Record::fresh(u32::MAX);
        r.counters = [u32::MAX; EVENTS];
        r.rx_bytes = u32::MAX;
        r.tx_bytes = u32::MAX;
        r.sample = [u32::MAX; 5];
        r.sequence = u32::MAX;
        for p in &mut r.progress {
            *p = Progress {
                event: u32::MAX,
                at_ms: u32::MAX,
                value: u32::MAX,
                sequence: u32::MAX,
            };
        }
        r.rare = [Rare {
            event: u32::MAX,
            at_ms: u32::MAX,
            value: u32::MAX,
            ordinal: u32::MAX,
            commit: u32::MAX,
        }; RING];
        r.ring_head = u32::MAX;
        r.ring_total = u32::MAX;
        r.report = [u32::MAX; 3];
        for i in 0..REPORT_LINES {
            let line = report_line(&r, i).unwrap();
            assert!(line.bytes().len() <= 158);
            assert!(line.bytes().is_ascii());
            if i == 0 {
                assert!(core::str::from_utf8(line.bytes())
                    .unwrap()
                    .contains("partial=1"));
            }
        }
        assert!(report_line(&r, REPORT_LINES).is_none());
    }
    #[test]
    fn rare_ring_is_bounded_commit_last_and_report_progress_is_independent() {
        let mut r = Record::fresh(1);
        for i in 0..12 {
            r.observe(Event::Reset, i, i);
        }
        assert_eq!(r.ring_total, 12);
        assert_eq!(r.ring_head, 4);
        assert_eq!(r.rare[3].ordinal, 12);
        assert_eq!(r.rare[4].ordinal, 5);
        assert!(r.rare.iter().all(|v| v.commit != 0 && v.commit & 1 == 0));
        let head = r.ring_head;
        r.observe(Event::RxPacket, 50, 512);
        r.report_progress(5, 5, 0);
        r.sample(60, 7, 8, 0);
        assert_eq!(r.ring_head, head);
        assert_eq!(r.report, [5, 5, 0]);
        r.report_progress(5, 5, 2);
        assert_eq!(r.report[2], 2);
        r.rare[3].commit = 0; // simulated reset before commit
        assert!(core::str::from_utf8(report_line(&r, 14).unwrap().bytes())
            .unwrap()
            .contains("usable=0"));
    }
}
