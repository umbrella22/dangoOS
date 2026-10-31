//! S31 clock switching with the HAL clock tree and the NOR core rendezvous.
use super::{ClockStep, Demand, Frequency, WorkBudget};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use esp_hal::clock::{self, ll, ClockConfig, CpuClock};

static ENABLED: AtomicBool = AtomicBool::new(false);
static REQUESTS: AtomicU32 = AtomicU32::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);
static INTERACTION: AtomicU32 = AtomicU32::new(0);
static CHANGES: AtomicU32 = AtomicU32::new(0);

/// Use a common SYS=80 MHz/APB=40 MHz profile from startup. Drivers therefore
/// compute their dividers against the same steady bus rates at 160 and 320.
pub fn boot_clock_config() -> ClockConfig {
    let mut config = ClockConfig::from(CpuClock::_320MHz);
    config.sys_clk = Some(ll::SysClkConfig::new(
        Frequency::Performance.system_divisor(),
    ));
    config
}

pub fn enable() {
    assert_eq!(clock::cpu_clock().as_mhz(), 320);
    assert_eq!(ll::mem_clk_frequency(), 160_000_000);
    assert_eq!(ll::sys_clk_frequency(), 80_000_000);
    assert_eq!(ll::apb_clk_frequency(), 40_000_000);
    ENABLED.store(true, Ordering::Release);
}

pub fn demand() -> Demand {
    Demand {
        requests: REQUESTS.load(Ordering::Acquire),
        generation: GENERATION.load(Ordering::Acquire),
        interaction: INTERACTION.load(Ordering::Acquire),
    }
}

/// A scoped request shared by both cores. It does not renew the interaction
/// hold. After release the next governor sample may select economy.
pub struct PerformanceRequest;

pub fn performance() -> PerformanceRequest {
    REQUESTS.fetch_add(1, Ordering::AcqRel);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    apply(Frequency::Performance, demand());
    PerformanceRequest
}

impl Drop for PerformanceRequest {
    fn drop(&mut self) {
        REQUESTS.fetch_sub(1, Ordering::AcqRel);
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
}

pub fn interaction() {
    INTERACTION.fetch_add(1, Ordering::AcqRel);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    apply(Frequency::Performance, demand());
}

/// Measures only a synchronous closure. Neither a request nor a measurement
/// survives its return, so callers can await timers/UI/DMA without a vote.
pub struct Budget(WorkBudget);

impl Budget {
    pub const fn new(budget_us: u64) -> Self {
        Self(WorkBudget::new(budget_us))
    }

    pub fn run<R>(&mut self, ready: bool, work: impl FnOnce() -> R) -> R {
        let _performance = (ready && self.0.needs_performance()).then(performance);
        let changes = CHANGES.load(Ordering::Acquire);
        let initial_mhz = clock::cpu_clock().as_mhz();
        let started = esp_hal::time::Instant::now();
        let result = work();
        let elapsed_us = started.elapsed().as_micros();
        let frequency = if initial_mhz == 320
            || clock::cpu_clock().as_mhz() == 320
            || CHANGES.load(Ordering::Acquire) != changes
        {
            Frequency::Performance
        } else {
            Frequency::Economy
        };
        self.0.record(elapsed_us, frequency);
        result
    }

    pub fn finish(&mut self) {
        self.0.finish();
    }
}

/// A downshift rechecks atomic requests after taking the clock-tree lock. A
/// racing request either prevents it, or performs its upshift after it ends.
pub fn apply(target: Frequency, observed: Demand) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let changed = ll::ClockTree::with(|clocks| {
        if target == Frequency::Economy
            && (REQUESTS.load(Ordering::Acquire) != 0
                || GENERATION.load(Ordering::Acquire) != observed.generation)
        {
            return false;
        }
        // Check the current rate only while serialized with a downshift. An
        // unlocked fast path could observe 320 just before another core lowers
        // it, letting a performance request return without restoring 320.
        if clock::cpu_clock().as_mhz() == target.mhz() {
            return false;
        }
        // Lock order: clock tree -> shared core rendezvous. Never take the
        // scheduler/global section while parked. NOR uses global section ->
        // rendezvous, and its ROM calls never need the clock-tree lock.
        // SAFETY: only register writes and atomic clock-cache updates occur
        // here. The ROM CPU timing update has no RTOS locks. PLLs, voltage,
        // USB PHY, XTAL/SYSTIMER and TIMG's PLL_F80M source remain unchanged.
        unsafe {
            crate::platform::runtime::cores::with_other_parked(|| {
                for step in target.transition() {
                    match step {
                        ClockStep::Cpu => ll::configure_cpu_clk(
                            clocks,
                            ll::CpuClkConfig::new(target.cpu_divisor()),
                        ),
                        ClockStep::Memory => ll::configure_mem_clk(
                            clocks,
                            ll::MemClkConfig::new(target.memory_divisor()),
                        ),
                        ClockStep::System => ll::configure_sys_clk(
                            clocks,
                            ll::SysClkConfig::new(target.system_divisor()),
                        ),
                    }
                }
            });
        }
        // Validate after the sibling resumes, so a failed check never leaves
        // it intentionally parked. This also catches clock-cache drift.
        let regs = esp_hal::peripherals::HP_SYS_CLKRST::regs();
        assert_eq!(
            regs.cpu_freq_ctrl0().read().cpu_clk_div_num().bits() as u32,
            target.cpu_divisor()
        );
        assert_eq!(
            u32::from(regs.mem_freq_ctrl0().read().mem_clk_div_num().bit()),
            target.memory_divisor()
        );
        assert_eq!(
            regs.sys_freq_ctrl0().read().sys_clk_div_num().bits() as u32,
            target.system_divisor()
        );
        assert_eq!(clock::cpu_clock().as_mhz(), target.mhz());
        assert_eq!(ll::mem_clk_frequency(), 160_000_000);
        assert_eq!(ll::sys_clk_frequency(), 80_000_000);
        assert_eq!(ll::apb_clk_frequency(), 40_000_000);
        true
    });
    if changed {
        CHANGES.fetch_add(1, Ordering::AcqRel);
    }
}
