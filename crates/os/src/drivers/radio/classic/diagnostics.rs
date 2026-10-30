//! The most recent lifecycle phase survives a manual reset after a stalled
//! vendor call. Work diagnostics retain internal task/code addresses, with no
//! peer identifiers or bond data.
use core::{
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicU32, Ordering},
};
const MAGIC: u32 = 0x4254_5031;
#[repr(C)]
#[derive(Clone, Copy)]
struct Mark {
    magic: u32,
    phase: u32,
    inverse: u32,
}
unsafe impl esp_hal::Persistable for Mark {}
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut LAST: Mark = Mark {
    magic: 0,
    phase: 0,
    inverse: 0,
};
static CURRENT: AtomicU32 = AtomicU32::new(0);

pub fn phase(value: u32) {
    CURRENT.store(value, Ordering::Relaxed);
    unsafe {
        addr_of_mut!(LAST.magic).write_volatile(0);
        addr_of_mut!(LAST.phase).write_volatile(value);
        addr_of_mut!(LAST.inverse).write_volatile(!value);
        addr_of_mut!(LAST.magic).write_volatile(MAGIC);
    }
    log::info!("classic: lifecycle phase={value}");
}
pub fn current() -> u32 {
    CURRENT.load(Ordering::Relaxed)
}
pub fn report() {
    #[cfg(feature = "radio-hang-diagnostics")]
    report_work();
    #[cfg(feature = "radio-hang-diagnostics")]
    report_hci_rx();
    let previous = unsafe { addr_of!(LAST).read_volatile() };
    unsafe { addr_of_mut!(LAST.magic).write_volatile(0) };
    if previous.magic == MAGIC
        && previous.inverse == !previous.phase
        && previous.phase != 0
        && previous.phase != 100
    {
        log::warn!(
            "classic: previous boot ended in lifecycle phase={}",
            previous.phase
        );
    }
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_phase(value: u32) {
    phase(value);
}

// #region debug-point Q-R-S:native-work-item
#[cfg(feature = "radio-hang-diagnostics")]
const WORK_MAGIC: u32 = 0x4257_4b31;
#[cfg(feature = "radio-hang-diagnostics")]
#[repr(C)]
#[derive(Clone, Copy)]
struct Work {
    magic: u32,
    task: u32,
    phase: u32,
    outer: u32,
    inner: u32,
    context: u32,
    starts: u32,
    finishes: u32,
    at_us: u32,
}
#[cfg(feature = "radio-hang-diagnostics")]
impl Work {
    const EMPTY: Self = Self {
        magic: 0, task: 0, phase: 0, outer: 0, inner: 0, context: 0,
        starts: 0, finishes: 0, at_us: 0,
    };
}
#[cfg(feature = "radio-hang-diagnostics")]
unsafe impl esp_hal::Persistable for Work {}
#[cfg(feature = "radio-hang-diagnostics")]
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut WORK: [Work; 3] = [Work::EMPTY; 3];
#[cfg(feature = "radio-hang-diagnostics")]
static WORK_TASKS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];

#[cfg(feature = "radio-hang-diagnostics")]
pub(super) fn register_work_task(task: u32, name: &str) {
    let slot = match name {
        "BTC_TASK" => 0,
        "hciT" => 1,
        "BTU_TASK" => 2,
        _ => return,
    };
    WORK_TASKS[slot].store(task, Ordering::Release);
}

#[cfg(feature = "radio-hang-diagnostics")]
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_work_trace(task: u32, function: u32, context: u32, phase: u32) {
    let Some(slot) = WORK_TASKS.iter().position(|id| id.load(Ordering::Acquire) == task && task != 0) else {
        return;
    };
    // All host workers are pinned to Core0. Each slot has exactly one writer;
    // report_work runs at boot before task creation. Volatile fields retain the
    // work boundary across a watchdog reset without logging or scheduler calls.
    unsafe {
        let ptr = addr_of_mut!(WORK).cast::<Work>().add(slot);
        let mut mark = ptr.read_volatile();
        if mark.magic != WORK_MAGIC || mark.task != task {
            mark = Work::EMPTY;
        }
        mark.magic = WORK_MAGIC;
        mark.task = task;
        mark.phase = phase;
        mark.at_us = esp_hal::time::Instant::now().duration_since_epoch().as_micros() as u32;
        match phase {
            1 => { mark.outer = function; mark.inner = 0; mark.context = context; mark.starts = mark.starts.wrapping_add(1); }
            2 => { mark.finishes = mark.finishes.wrapping_add(1); }
            3 | 4 => { mark.inner = function; mark.context = context; }
            _ => {}
        }
        // Phase0 makes an interrupted multiword write recognizable on reboot.
        core::ptr::write_volatile(addr_of_mut!((*ptr).phase), 0);
        let final_phase = mark.phase;
        mark.phase = 0;
        ptr.write_volatile(mark);
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        core::ptr::write_volatile(addr_of_mut!((*ptr).phase), final_phase);
    }
}

#[cfg(feature = "radio-hang-diagnostics")]
fn report_work() {
    for slot in 0..WORK_TASKS.len() {
        let mark = unsafe { addr_of!(WORK).cast::<Work>().add(slot).read_volatile() };
        if mark.magic == WORK_MAGIC {
            // Each USB log record has 158 text bytes. Split identity and
            // counters so a retained record never loses its completion count.
            log::info!("classic work previous slot={slot} task={:#x} phase={} outer={:#x} inner={:#x}", mark.task, mark.phase, mark.outer, mark.inner);
            log::info!("classic work counts slot={slot} context={:#x} starts={} finishes={} at_us={}", mark.context, mark.starts, mark.finishes, mark.at_us);
        }
        unsafe { addr_of_mut!((*addr_of_mut!(WORK).cast::<Work>().add(slot)).magic).write_volatile(0) };
    }
}
// #endregion

// #region debug-point T-U-V:hci-receive-progress
#[cfg(feature = "radio-hang-diagnostics")]
const RX_MAGIC: u32 = 0x4252_5831;
#[cfg(feature = "radio-hang-diagnostics")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HciRx {
    magic: u32,
    phase: u32,
    calls: u32,
    queued: u32,
    processed: u32,
    pending: u32,
    packets: u32,
    nulls: u32,
    empty_calls: u32,
    acl: u32,
    events: u32,
    header: u32,
}
#[cfg(feature = "radio-hang-diagnostics")]
unsafe impl esp_hal::Persistable for HciRx {}
#[cfg(feature = "radio-hang-diagnostics")]
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut HCI_RX: HciRx = HciRx {
    magic: 0, phase: 0, calls: 0, queued: 0, processed: 0, pending: 0,
    packets: 0, nulls: 0, empty_calls: 0, acl: 0, events: 0, header: 0,
};

#[cfg(feature = "radio-hang-diagnostics")]
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_hci_rx_trace(phase: u32, value: u32) {
    // The hciT worker is the only writer; boot reports before task creation.
    // No clock, logger, allocator, queue, semaphore or scheduler calls here.
    unsafe {
        let ptr = addr_of_mut!(HCI_RX);
        let mut mark = ptr.read_volatile();
        if mark.magic != RX_MAGIC {
            mark = HciRx::default();
        }
        mark.magic = RX_MAGIC;
        match phase {
            1 => {
                mark.calls = mark.calls.wrapping_add(1);
                mark.queued = value;
                mark.processed = 0;
                mark.pending = 0;
            }
            2 => {
                mark.processed = mark.processed.wrapping_add(1);
                mark.packets = mark.packets.wrapping_add(1);
                mark.header = value;
                match value & 0xff {
                    2 => mark.acl = mark.acl.wrapping_add(1),
                    4 => mark.events = mark.events.wrapping_add(1),
                    _ => {}
                }
            }
            3 => mark.nulls = mark.nulls.wrapping_add(1),
            4 => {
                mark.pending = value;
                if mark.processed == 0 {
                    mark.empty_calls = mark.empty_calls.wrapping_add(1);
                }
            }
            _ => {}
        }
        core::ptr::write_volatile(addr_of_mut!((*ptr).phase), 0);
        mark.phase = 0;
        ptr.write_volatile(mark);
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        core::ptr::write_volatile(addr_of_mut!((*ptr).phase), phase);
    }
}

#[cfg(feature = "radio-hang-diagnostics")]
fn report_hci_rx() {
    let m = unsafe { addr_of!(HCI_RX).read_volatile() };
    if m.magic == RX_MAGIC {
        log::info!("classic rx previous phase={} calls={} queued={} processed={} pending={}", m.phase, m.calls, m.queued, m.processed, m.pending);
        log::info!("classic rx totals packets={} nulls={} empty={} acl={} events={}", m.packets, m.nulls, m.empty_calls, m.acl, m.events);
        log::info!("classic rx header={:#x}", m.header);
    }
    unsafe { addr_of_mut!(HCI_RX.magic).write_volatile(0) };
}
// #endregion
