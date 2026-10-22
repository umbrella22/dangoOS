//! Device adapter for the host-tested updater; no flash work inside a VFS lock.
use crate::{
    drivers::storage::{self, OtaFlash},
    tasks,
};
use alloc::{boxed::Box, format, string::String};
use core::{
    ptr::addr_of_mut,
    sync::atomic::{AtomicBool, Ordering},
};
use embassy_time::{Instant, Timer};
use mosaico_s31::ota::{self, Command, Error, Updater};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static VM_OK: AtomicBool = AtomicBool::new(false);
static AB_LAYOUT: AtomicBool = AtomicBool::new(false);
static HEALTH: critical_section::Mutex<core::cell::RefCell<ota::HealthWindow>> =
    critical_section::Mutex::new(core::cell::RefCell::new(ota::HealthWindow::new()));
static CONFIRMED: AtomicBool = AtomicBool::new(false);
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}
const RTC_MAGIC: u32 = 0x4f544136;
#[repr(C)]
struct Attempts {
    magic: u32,
    slot: u32,
    previous: u32,
    count: u32,
    seq: u32,
    check: u32,
}
unsafe impl esp_hal::Persistable for Attempts {}
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut ATTEMPTS: Attempts = Attempts {
    magic: 0,
    slot: 0,
    previous: 0,
    count: 0,
    seq: 0,
    check: 0,
};
impl Attempts {
    fn valid(&self) -> bool {
        self.magic == RTC_MAGIC
            && self.slot < 2
            && self.previous < 2
            && self.slot != self.previous
            && self.check == self.sum()
    }
    fn sum(&self) -> u32 {
        self.magic ^ self.slot ^ self.previous ^ self.count ^ self.seq ^ 0x594a8327
    }
    fn seal(&mut self) {
        self.check = self.sum();
    }
}
pub fn record_boot_attempt() {
    let a = unsafe { &mut *addr_of_mut!(ATTEMPTS) };
    if a.valid() {
        a.count = a.count.saturating_add(1);
        a.seal();
    }
}
fn arm(slot: u8, previous: u8, seq: u32) {
    let a = unsafe { &mut *addr_of_mut!(ATTEMPTS) };
    *a = Attempts {
        magic: RTC_MAGIC,
        slot: slot as u32,
        previous: previous as u32,
        count: 0,
        seq,
        check: 0,
    };
    a.seal();
}
fn disarm() {
    unsafe {
        (*addr_of_mut!(ATTEMPTS)).magic = 0;
    }
}
/// Pinned esp-hal bb239a0 partitions.rs S31 MMU-entry-zero algorithm.
pub fn booted_slot() -> ota::Result<u8> {
    let physical = critical_section::with(|_| unsafe {
        (0x20500380 as *mut u32).write_volatile(0);
        ((0x2050037c as *const u32).read_volatile() & 0x7ff) << 16
    });
    match physical {
        0x210000 => Ok(0),
        0x510000 => Ok(1),
        _ => Err(Error::State("MMU running partition is not an OTA slot")),
    }
}
pub fn early_boot() {
    log::info!(
        "ota: compatibility={}",
        core::str::from_utf8(ota::AB_LAYOUT_MARKER).unwrap_or("invalid")
    );
    if storage::early_ab().is_err() {
        return;
    }
    AB_LAYOUT.store(true, Ordering::Release);
    let Ok(booted) = booted_slot() else {
        return;
    };
    let a = unsafe { &mut *addr_of_mut!(ATTEMPTS) };
    if !a.valid() {
        return;
    }
    if a.slot != booted as u32 {
        disarm();
        return;
    }
    let Ok(entries) = ota::read_entries(&mut OtaFlash) else {
        return;
    };
    if ota::selected(entries)
        .ok()
        .flatten()
        .is_none_or(|(_, e)| e.seq != a.seq || !matches!(e.state, 0 | 1))
    {
        disarm();
        return;
    }
    if ota::warm_fallback_due(a.count) {
        log::warn!("ota: RTC fallback after {} warm boot attempts", a.count);
        match ota::select_slot(&mut OtaFlash, booted, a.previous as u8, 2) {
            Ok(_) => {
                disarm();
                esp_hal::system::software_reset();
            }
            Err(e) => log::warn!("ota: RTC fallback refused: {e}"),
        }
    }
    #[cfg(feature = "ota-test-panic")]
    panic!("ota-test-panic: unconfirmed A/B candidate before PSRAM/display/VFS");
}
struct ProbeHost;
impl mosaico_lua::Host for ProbeHost {
    fn print(&mut self, _: &[u8]) {}
}
fn exercise_vm() -> bool {
    let mut vm = match mosaico_lua::Vm::new(
        Box::new(ProbeHost),
        mosaico_lua::Limits {
            memory_bytes: 128 * 1024,
            ..Default::default()
        },
        &tasks::lua::PSRAM,
        0x4f5441,
    ) {
        Ok(vm) => vm,
        Err(_) => return false,
    };
    vm.set_callback_budget(20, 100_000);
    if vm
        .load(
            "ota-selftest",
            b"assert(2+2==4); assert(require('json').decode('{\"ok\":true}').ok)",
        )
        .is_err()
    {
        return false;
    }
    matches!(vm.resume(), mosaico_lua::Step::Done)
}
pub fn health() -> String {
    let (system, data) = storage::mounted();
    format!(
        "sys={} data={} frame={} vm={} hb={} valid={}",
        system as u8,
        data as u8,
        mosaico_s31::telemetry::display::FIRST_FRAME.load(Ordering::Acquire) as u8,
        VM_OK.load(Ordering::Acquire) as u8,
        tasks::ui::HEARTBEATS.load(Ordering::Acquire),
        CONFIRMED.load(Ordering::Acquire) as u8
    )
}
/// Called from the real UI loop after frame handoff and native housekeeping.
pub fn selftest() {
    if !AB_LAYOUT.load(Ordering::Acquire) || CONFIRMED.load(Ordering::Acquire) {
        return;
    }
    let (system, data) = storage::mounted();
    let ready = !active()
        && storage::ready()
        && system
        && data
        && !crate::crashlog::skip(crate::crashlog::Stage::Storage)
        && !crate::crashlog::skip(crate::crashlog::Stage::AppScan);
    if ready && !VM_OK.load(Ordering::Acquire) {
        VM_OK.store(exercise_vm(), Ordering::Release);
    }
    let healthy = ready
        && VM_OK.load(Ordering::Acquire)
        && mosaico_s31::telemetry::display::FIRST_FRAME.load(Ordering::Acquire)
        && tasks::ui::HEARTBEATS.load(Ordering::Acquire) >= 2;
    let passed = critical_section::with(|cs| {
        HEALTH
            .borrow(cs)
            .borrow_mut()
            .observe(Instant::now().as_millis(), healthy)
    });
    if !passed {
        return;
    }
    let Ok(layout) = storage::layout() else {
        return;
    };
    if !layout.ab {
        return;
    }
    let Ok(slot) = booted_slot() else {
        return;
    };
    match ota::confirm(&mut OtaFlash, slot) {
        Ok(()) => {
            disarm();
            CONFIRMED.store(true, Ordering::Release);
            log::info!("ota: selftest confirmed {}", health());
        }
        Err(e) => log::warn!("ota: confirmation refused {e}"),
    }
}
pub struct DeviceUpdater {
    inner: Option<Updater>,
    old_haptic: bool,
}
impl DeviceUpdater {
    pub fn new() -> Self {
        Self {
            inner: None,
            old_haptic: true,
        }
    }
    pub fn abort(&mut self) {
        if let Some(up) = self.inner.as_mut() {
            if up.phase != ota::Phase::Activated {
                up.abort();
            }
        }
        if ACTIVE.swap(false, Ordering::AcqRel) {
            tasks::lua::release_management();
            tasks::haptic::set_enabled(self.old_haptic);
        }
    }
    pub async fn execute(&mut self, cmd: Command) -> core::result::Result<(String, bool), String> {
        let verb = cmd.verb();
        let result = self.command(cmd).await;
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                self.abort();
                Err(format!("@err {verb}: {e}"))
            }
        }
    }
    async fn command(&mut self, cmd: Command) -> ota::Result<(String, bool)> {
        match cmd {
            Command::Status => {
                let layout = storage::layout();
                let status = if layout.as_ref().is_ok_and(|l| l.ab) {
                    let status = if let Some(up) = &self.inner {
                        up.status()
                    } else {
                        booted_slot()
                            .and_then(|slot| {
                                Updater::new(layout.as_ref().unwrap(), slot, &mut OtaFlash)
                            })
                            .map(|up| up.status())
                            .unwrap_or_else(|_| String::from("unavailable"))
                    };
                    status
                } else {
                    String::from("unavailable")
                };
                if layout.as_ref().is_ok_and(|l| l.ab) {
                    let slots = ota::status::slot_lines(&mut OtaFlash, booted_slot().ok());
                    Ok((
                        format!(
                            "{}\n{}\n@ok ota-status {status} {}",
                            slots[0],
                            slots[1],
                            health()
                        ),
                        false,
                    ))
                } else {
                    Ok((format!("@ok ota-status {status} {}", health()), false))
                }
            }
            Command::Abort => {
                self.abort();
                Ok(("@ok ota-abort".into(), false))
            }
            Command::Begin { size, sha } => {
                if !CONFIRMED.load(Ordering::Acquire) {
                    return Err(Error::State("boot selftest not confirmed"));
                }
                if active() {
                    return Err(Error::Busy);
                }
                if tasks::apps::is_managing() {
                    return Err(Error::Busy);
                }
                let layout = storage::layout()?;
                let slot = booted_slot()?;
                let mut updater = Updater::new(&layout, slot, &mut OtaFlash)?;
                updater.begin(size, sha)?;
                ACTIVE.store(true, Ordering::Release);
                self.old_haptic = tasks::haptic::enabled();
                tasks::haptic::set_enabled(false);
                match embassy_futures::select::select(tasks::lua::quiesce(), Timer::after_secs(5))
                    .await
                {
                    embassy_futures::select::Either::First(()) => {}
                    _ => return Err(Error::Busy),
                }
                tasks::lights::lua_release();
                tasks::lights::brain_frame(None, 0);
                let reply = format!("@ok ota-begin {} {}", updater.booted(), updater.target());
                self.inner = Some(updater);
                Ok((reply, false))
            }
            Command::Chunk { offset, bytes, crc } => {
                if !active() {
                    return Err(Error::State("no transfer"));
                }
                let up = self.inner.as_mut().ok_or(Error::State("no transfer"))?;
                let offset = up.chunk(&mut OtaFlash, offset, &bytes, crc)?;
                Timer::after_millis(1).await;
                Ok((format!("@ok ota-chunk {offset}"), false))
            }
            Command::End => {
                if !active() {
                    return Err(Error::State("no transfer"));
                }
                let up = self.inner.as_mut().ok_or(Error::State("no transfer"))?;
                up.start_verify()?;
                while !up.verify_step(&mut OtaFlash)? {
                    Timer::after_millis(1).await;
                }
                let entries = ota::read_entries(&mut OtaFlash)?;
                let (_, selected) =
                    ota::selected(entries)?.ok_or(Error::State("activation metadata absent"))?;
                arm(up.target(), up.booted(), selected.seq);
                Ok(("@ok ota-end".into(), true))
            }
        }
    }
}
