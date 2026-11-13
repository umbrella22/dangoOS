//! Shared exclusion for NOR ROM calls and CPU clock changes that park a core.
use core::sync::atomic::{AtomicBool, Ordering};
use esp_hal::system::{Cpu, CpuControl};
use esp_sync::NonReentrantMutex;

static MULTICORE: AtomicBool = AtomicBool::new(false);
static RENDEZVOUS: NonReentrantMutex<()> = NonReentrantMutex::new(());

pub fn enable_multicore() {
    MULTICORE.store(true, Ordering::Release);
}

/// Acquire every lock needed by `f` before entering this rendezvous. While the
/// sibling is parked, `f` must not allocate, log, yield, or acquire any lock
/// that the sibling could own. Reentry into a lock already owned here is valid.
/// The rendezvous masks local IRQs and serializes the CPU_CTRL register use.
pub unsafe fn with_other_parked<R>(f: impl FnOnce() -> R) -> R {
    RENDEZVOUS.with(|_| {
        if !MULTICORE.load(Ordering::Acquire) {
            return f();
        }
        let other = Cpu::other().next().expect("S31 second core");
        // CPU_CTRL is otherwise consumed only during second-core startup.
        // NOR and frequency changes share this rendezvous for later access.
        let mut control = CpuControl::new(unsafe { esp_hal::peripherals::CPU_CTRL::steal() });
        unsafe { control.park_core(other) };
        let result = f();
        resume_core(other);
        result
    })
}

/// Release software stall and wake an idle core before waiting for it. S31's
/// CORESTALLED flag can remain set while the idle task waits in WFI.
fn resume_core(other: esp_hal::system::Cpu) {
    use esp_hal::{
        peripherals::{HP_SYS, PMU},
        system::Cpu,
    };
    PMU::regs().cpu_stall_sw().modify(|_, w| unsafe {
        match other {
            Cpu::ProCpu => w.hpcore0_sw_stall_code().bits(0xff),
            Cpu::AppCpu => w.hpcore1_sw_stall_code().bits(0xff),
        }
    });
    let status = HP_SYS::regs().cpu_corestalled_st();
    while match other {
        Cpu::ProCpu => status.read().reg_core0_corestalled_st().bit_is_set(),
        Cpu::AppCpu => status.read().reg_core1_corestalled_st().bit_is_set(),
    } {
        // Raises CLINT MSIP without taking the scheduler or global lock.
        esp_hal::interrupt::__rtos_implementation::request_context_switch(other);
        core::hint::spin_loop();
    }
}
