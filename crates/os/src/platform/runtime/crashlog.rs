//! Panic record that survives the reset.
//!
//! The only console is USB CDC, which is not up during early boot and stops
//! when the firmware panics, so a panic message would never be seen. The
//! panic handler writes it into RTC fast memory (kept across a software
//! reset) and resets; the next boot logs it once with the reset reason.
//!
//! Risky boot steps mark themselves with [`enter`]. If the last boot
//! panicked inside one, [`skip`] tells this boot to leave it out once, so the
//! board comes up far enough for USB to show the record instead of looping.

use core::fmt::Write;
use core::panic::PanicInfo;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use log::{info, warn};

const MAGIC: u32 = 0x4D53_5031; // "MSP1"
#[cfg(feature = "embedscope")]
const SCOPE_MAGIC: u32 = 0x4553_4332; // "ESC2": metadata bound to a text marker.

const TEXT: usize = 248;

#[repr(C)]
struct Record {
    magic: u32,
    len: u32,
    /// Consecutive panics without a clean boot in between.
    count: u32,
    /// The [`Stage`] the panic happened in.
    stage: u32,
    text: [u8; TEXT],
    #[cfg(feature = "embedscope")]
    scope_magic: u32,
    #[cfg(feature = "embedscope")]
    scope_build: [u8; 64],
    #[cfg(feature = "embedscope")]
    scope_build_len: u32,
    #[cfg(feature = "embedscope")]
    scope_boot: u32,
}

// SAFETY: only integers and byte arrays; any bit pattern is a valid Record
// (the magic word tells real records from power-on garbage).
unsafe impl esp_hal::Persistable for Record {}

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RECORD: Record = Record {
    magic: 0,
    len: 0,
    count: 0,
    stage: 0,
    text: [0; TEXT],
    #[cfg(feature = "embedscope")]
    scope_magic: 0,
    #[cfg(feature = "embedscope")]
    scope_build: [0; 64],
    #[cfg(feature = "embedscope")]
    scope_build_len: 0,
    #[cfg(feature = "embedscope")]
    scope_boot: 0,
};

/// Boot steps that [`skip`] can leave out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Stage {
    Running = 0,
    Storage = 1,
    AppScan = 2,
}

impl Stage {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Storage,
            2 => Self::AppScan,
            _ => Self::Running,
        }
    }
}

static CURRENT: AtomicU32 = AtomicU32::new(Stage::Running as u32);
// A second panic, including an interrupt on the recording core, cannot borrow
// the RTC record while the first panic formats its message and resets.
static RECORDING: AtomicBool = AtomicBool::new(false);
/// Stage of the previous boot's panic; `u32::MAX` when it did not panic.
static LAST: AtomicU32 = AtomicU32::new(u32::MAX);

/// Mark the start (or, with `Stage::Running`, the end) of a risky step.
pub fn enter(stage: Stage) {
    CURRENT.store(stage as u32, Ordering::Relaxed);
}

static PANICKED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether the previous boot ended in a panic.
pub fn last_boot_panicked() -> bool {
    PANICKED.load(Ordering::Relaxed)
}

/// Whether the previous boot panicked inside `stage`.
pub fn skip(stage: Stage) -> bool {
    LAST.load(Ordering::Relaxed) == stage as u32
}

struct Sink<'a> {
    buf: &'a mut [u8; TEXT],
    len: usize,
}

impl Write for Sink<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(TEXT - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if RECORDING.swap(true, Ordering::AcqRel) {
        loop {
            core::hint::spin_loop();
        }
    }
    // SAFETY: RECORDING admits only one panicking context across both cores.
    // Boot reads RECORD before either core starts concurrent tasks.
    let rec = unsafe { &mut *addr_of_mut!(RECORD) };
    // `count` is only meaningful while the record is uninitialised RAM at
    // power-on; cap it so garbage reads as "many".
    let count = rec.count.saturating_add(1).min(99);
    let mut sink = Sink {
        buf: &mut rec.text,
        len: 0,
    };
    #[cfg(feature = "embedscope")]
    let _ =
        crate::embedscope::panic_record::write_marker(&mut sink, env!("DANGO_EMBEDSCOPE_BUILD_ID"));
    // The message first (it carries the PC for CPU exceptions), then the
    // location without its long directory part.
    let _ = write!(sink, "{}", info.message());
    if let Some(loc) = info.location() {
        let file = loc.file().rsplit('/').next().unwrap_or(loc.file());
        let _ = write!(sink, " @ {file}:{}", loc.line());
    }
    rec.len = sink.len as u32;
    rec.count = count;
    rec.stage = CURRENT.load(Ordering::Relaxed);
    #[cfg(feature = "embedscope")]
    {
        let build = env!("DANGO_EMBEDSCOPE_BUILD_ID").as_bytes();
        rec.scope_build[..build.len()].copy_from_slice(build);
        rec.scope_build_len = build.len() as u32;
        rec.scope_boot = DIAGNOSTIC_BOOT.load(Ordering::Relaxed);
        rec.scope_magic = SCOPE_MAGIC;
    }
    rec.magic = MAGIC;
    esp_hal::system::software_reset()
}

/// Log the previous panic, if any, and the reset reason. Call after the
/// logger is installed.
pub fn report_last_boot() {
    let reason = esp_hal::system::reset_reason();
    // SAFETY: called once at boot, before any other task runs.
    let rec = unsafe { &mut *addr_of_mut!(RECORD) };
    if rec.magic == MAGIC {
        #[cfg(feature = "embedscope")]
        save_diagnostic(rec);
        let len = (rec.len as usize).min(TEXT);
        let text = core::str::from_utf8(&rec.text[..len]).unwrap_or("<not UTF-8>");
        PANICKED.store(true, Ordering::Relaxed);
        let stage = Stage::from_u32(rec.stage);
        warn!(
            "[boot] reset {reason:?} after panic #{} in {stage:?}:",
            rec.count
        );
        // Log lines are short; print the record in pieces.
        let mut rest = text;
        while !rest.is_empty() {
            let mut n = rest.len().min(100);
            while !rest.is_char_boundary(n) {
                n -= 1;
            }
            warn!("[boot]   {}", &rest[..n]);
            rest = &rest[n..];
        }
        if stage != Stage::Running {
            warn!("[boot] skipping {stage:?} this boot");
            LAST.store(stage as u32, Ordering::Relaxed);
        }
        // Report once; the count survives so repeated panics stay visible.
        rec.magic = 0;
    } else {
        info!("[boot] reset {reason:?}");
        rec.count = 0;
    }
    // Never leave an attribution tail for an older firmware to inherit.
    #[cfg(feature = "embedscope")]
    {
        rec.scope_magic = 0;
        rec.scope_build_len = 0;
        rec.scope_boot = 0;
    }
}

#[cfg(feature = "embedscope")]
static DIAGNOSTIC_BOOT: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "embedscope")]
struct DiagnosticCrash {
    present: bool,
    message: [u8; TEXT],
    len: usize,
    build: [u8; 64],
    build_len: usize,
    boot_id: u32,
}
#[cfg(feature = "embedscope")]
static mut DIAGNOSTIC_CRASH: DiagnosticCrash = DiagnosticCrash {
    present: false,
    message: [0; TEXT],
    len: 0,
    build: [0; 64],
    build_len: 0,
    boot_id: 0,
};
#[cfg(feature = "embedscope")]
pub(crate) fn set_diagnostic_boot(id: u32) {
    DIAGNOSTIC_BOOT.store(id, Ordering::Relaxed);
}
#[cfg(feature = "embedscope")]
fn save_diagnostic(record: &Record) {
    // report_last_boot is the sole writer, before either executor starts.
    let saved = unsafe { &mut *core::ptr::addr_of_mut!(DIAGNOSTIC_CRASH) };
    saved.present = true;
    saved.len = (record.len as usize).min(TEXT);
    saved.message[..saved.len].copy_from_slice(&record.text[..saved.len]);
    saved.build_len = 0;
    saved.boot_id = 0;
    let build_len = record.scope_build_len as usize;
    if record.scope_magic == SCOPE_MAGIC
        && build_len <= record.scope_build.len()
        && crate::embedscope::panic_record::matches_build(
            &record.text[..saved.len],
            &record.scope_build[..build_len],
        )
    {
        saved.build_len = build_len;
        saved.build[..saved.build_len].copy_from_slice(&record.scope_build[..saved.build_len]);
        saved.boot_id = record.scope_boot;
    }
}
#[cfg(feature = "embedscope")]
pub(crate) fn diagnostic_summary() -> escope_protocol::CrashInfo<'static> {
    // Immutable after report_last_boot completed, before tasks were spawned.
    let saved = unsafe { &*core::ptr::addr_of!(DIAGNOSTIC_CRASH) };
    escope_protocol::CrashInfo {
        present: saved.present,
        reason: if saved.present { "panic" } else { "none" },
        build_id: core::str::from_utf8(&saved.build[..saved.build_len]).unwrap_or("unknown"),
        message: core::str::from_utf8(&saved.message[..saved.len]).unwrap_or("<not UTF-8>"),
        boot_id: saved.boot_id,
    }
}
