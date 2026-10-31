//! CPU frequency policy and the S31 bus-divider transition plan.
//! Both cores share one frequency. Voltage and PLL sources stay unchanged.

#[cfg(target_os = "none")]
pub(crate) mod device;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    Economy,
    Performance,
}

impl Frequency {
    pub const fn mhz(self) -> u32 {
        match self {
            Self::Economy => 160,
            Self::Performance => 320,
        }
    }

    pub const fn cpu_divisor(self) -> u32 {
        match self {
            Self::Economy => 1,
            Self::Performance => 0,
        }
    }

    pub const fn memory_divisor(self) -> u32 {
        match self {
            Self::Economy => 0,
            Self::Performance => 1,
        }
    }

    pub const fn system_divisor(self) -> u32 {
        match self {
            Self::Economy => 1,
            Self::Performance => 3,
        }
    }

    /// The CPU setter also adjusts the hardware memory divider. Synchronize
    /// its clock-tree entry explicitly, and slow buses before raising CPU.
    pub const fn transition(self) -> [ClockStep; 3] {
        match self {
            Self::Economy => [ClockStep::Cpu, ClockStep::Memory, ClockStep::System],
            Self::Performance => [ClockStep::System, ClockStep::Memory, ClockStep::Cpu],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ClockStep {
    Cpu,
    Memory,
    System,
}

pub const HOLD_MS: u64 = 500;
pub const STARTUP_HOLD_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug, Default)]
pub struct Demand {
    pub requests: u32,
    /// Changes to requests, used to reject a racing downshift.
    pub generation: u32,
    /// Interactive activity alone renews the 500 ms hold.
    pub interaction: u32,
}

pub struct Policy {
    interaction: u32,
    performance_until_ms: u64,
}

impl Policy {
    pub fn new(now_ms: u64) -> Self {
        Self {
            interaction: 0,
            performance_until_ms: now_ms.saturating_add(STARTUP_HOLD_MS),
        }
    }

    /// `now_ms` is monotonic wall time, independent of CPU cycle frequency.
    pub fn target(&mut self, now_ms: u64, demand: Demand, background_busy: bool) -> Frequency {
        if demand.interaction != self.interaction || background_busy {
            self.performance_until_ms = self
                .performance_until_ms
                .max(now_ms.saturating_add(HOLD_MS));
        }
        self.interaction = demand.interaction;
        if demand.requests != 0 || now_ms < self.performance_until_ms {
            Frequency::Performance
        } else {
            Frequency::Economy
        }
    }
}

/// Predict the cost of the next synchronous work window at 160 MHz. This is
/// a conservative wall-time estimate, not a CPU-utilization measurement:
/// preemption and memory stalls remain included. Idle waits are never added.
/// Accumulate Lua slices until a real wait; finish each actual raster frame.
pub struct WorkBudget {
    budget_us: u64,
    work_us: u64,
    performance: bool,
    light_windows: u8,
    boosted_windows: u8,
    economy_window: bool,
}

impl WorkBudget {
    pub const fn new(budget_us: u64) -> Self {
        assert!(budget_us > 0);
        Self {
            budget_us,
            work_us: 0,
            performance: false,
            light_windows: 0,
            boosted_windows: 0,
            economy_window: true,
        }
    }

    pub fn needs_performance(&self) -> bool {
        // Clock scaling is only an estimate (PSRAM/buses do not scale).
        // Recheck at economy after at most 16 completed boosted windows so a
        // conservative estimate in the hysteresis band cannot pin high forever.
        (self.performance && self.boosted_windows < 16) || self.work_us >= self.budget_us
    }

    pub fn record(&mut self, elapsed_us: u64, frequency: Frequency) {
        self.economy_window &= frequency == Frequency::Economy;
        // At 320 MHz use twice the measured time as a conservative estimate
        // of 160 MHz work. This avoids oscillating merely because boost made
        // the same work faster. Buses do not scale, so this is an upper bound.
        let factor = frequency.mhz() / Frequency::Economy.mhz();
        self.work_us = self
            .work_us
            .saturating_add(elapsed_us.saturating_mul(factor as u64));
    }

    pub fn finish(&mut self) {
        if self.work_us >= self.budget_us {
            self.performance = true;
            self.light_windows = 0;
            self.boosted_windows = if self.boosted_windows >= 16 && self.economy_window {
                0
            } else {
                self.boosted_windows.saturating_add(1)
            };
        } else if self.work_us <= self.budget_us * 3 / 4 {
            self.light_windows = self.light_windows.saturating_add(1);
            if self.light_windows >= 3 {
                self.performance = false;
            }
        } else {
            self.light_windows = 0;
        }
        if self.work_us < self.budget_us && self.performance {
            self.boosted_windows = self.boosted_windows.saturating_add(1);
        }
        if !self.performance {
            self.boosted_windows = 0;
        }
        self.work_us = 0;
        self.economy_window = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_keep_performance_without_adding_a_tail() {
        let mut policy = Policy::new(100);
        assert_eq!(
            policy.target(2099, Demand::default(), false),
            Frequency::Performance
        );
        assert_eq!(
            policy.target(2100, Demand::default(), false),
            Frequency::Economy
        );
        let mut demand = Demand::default();
        // Frequent 10 ms work / 40 ms waits must not renew the 500 ms touch hold.
        for now in (3000..6000).step_by(50) {
            demand.requests = 2;
            demand.generation = demand.generation.wrapping_add(1);
            assert_eq!(policy.target(now, demand, false), Frequency::Performance);
            demand.requests = 1;
            assert_eq!(
                policy.target(now + 5, demand, false),
                Frequency::Performance
            );
            demand.requests = 0;
            demand.generation = demand.generation.wrapping_add(1);
            assert_eq!(policy.target(now + 10, demand, false), Frequency::Economy);
        }
    }

    #[test]
    fn touch_and_background_holds_survive_work_request_release() {
        let mut policy = Policy::new(0);
        let mut demand = Demand {
            interaction: 1,
            ..Demand::default()
        };
        assert_eq!(policy.target(3000, demand, false), Frequency::Performance);
        demand.requests = 1;
        policy.target(3100, demand, false);
        demand.requests = 0;
        demand.generation = 20;
        assert_eq!(policy.target(3499, demand, false), Frequency::Performance);
        assert_eq!(policy.target(3500, demand, false), Frequency::Economy);
        demand.interaction = 2;
        assert_eq!(policy.target(4000, demand, false), Frequency::Performance);
        assert_eq!(policy.target(4499, demand, false), Frequency::Performance);
        assert_eq!(policy.target(4500, demand, false), Frequency::Economy);
        assert_eq!(policy.target(5000, demand, true), Frequency::Performance);
        assert_eq!(policy.target(20000, demand, true), Frequency::Performance);
        assert_eq!(policy.target(20499, demand, false), Frequency::Performance);
        assert_eq!(policy.target(20500, demand, false), Frequency::Economy);
    }

    #[test]
    fn interaction_wrap_is_still_activity_but_request_wrap_is_not() {
        let mut policy = Policy::new(0);
        policy.target(
            3000,
            Demand {
                interaction: u32::MAX,
                ..Demand::default()
            },
            false,
        );
        assert_eq!(
            policy.target(4000, Demand::default(), false),
            Frequency::Performance
        );
        assert_eq!(
            policy.target(
                4500,
                Demand {
                    generation: u32::MAX,
                    ..Demand::default()
                },
                false
            ),
            Frequency::Economy
        );
        assert_eq!(
            policy.target(4600, Demand::default(), false),
            Frequency::Economy
        );
    }

    #[test]
    fn short_lua_slices_accumulate_until_a_real_wait() {
        let mut budget = WorkBudget::new(8000);
        for _ in 0..7 {
            budget.record(1000, Frequency::Economy);
            assert!(!budget.needs_performance());
        }
        budget.record(1000, Frequency::Economy);
        assert!(budget.needs_performance());
        budget.finish();
        assert!(budget.needs_performance());
        // Long timer/input/frame waits add no work and acquire no request.
        // Three light bursts later, the prediction recovers as well.
        for index in 0..3 {
            budget.record(500, Frequency::Performance);
            budget.finish();
            assert_eq!(budget.needs_performance(), index < 2);
        }
    }

    #[test]
    fn light_frames_stay_at_economy_and_boost_does_not_fake_a_recovery() {
        let mut home = WorkBudget::new(25000);
        for _ in 0..100 {
            home.record(22000, Frequency::Economy);
            home.finish();
            assert!(!home.needs_performance());
        }
        home.record(28000, Frequency::Economy);
        home.finish();
        assert!(home.needs_performance());
        for index in 0..100 {
            home.record(14000, Frequency::Performance);
            home.finish();
            assert_eq!(home.needs_performance(), index < 14);
        }
        for _ in 0..3 {
            home.record(8000, Frequency::Performance);
            home.finish();
        }
        assert!(!home.needs_performance());
    }

    #[test]
    fn conservative_estimate_cannot_pin_a_light_periodic_renderer_high() {
        let mut home = WorkBudget::new(25000);
        home.record(27000, Frequency::Economy);
        home.finish();
        for index in 0..15 {
            home.record(12000, Frequency::Performance); // estimate 24 ms: deadband
            home.finish();
            assert_eq!(home.needs_performance(), index < 14);
        }
        for _ in 0..100 {
            home.record(22000, Frequency::Economy);
            home.finish();
            assert!(!home.needs_performance());
        }
        home.record(30000, Frequency::Economy);
        home.finish();
        assert!(home.needs_performance());
    }

    #[test]
    fn economy_probe_waits_for_an_actual_economy_measurement() {
        let mut budget = WorkBudget::new(1000);
        for _ in 0..16 {
            budget.record(600, Frequency::Performance);
            budget.finish();
        }
        for _ in 0..5 {
            assert!(!budget.needs_performance());
            // The governor may not have sampled yet, or the other core can
            // hold a request. Measuring at 320 is not an economy probe.
            budget.record(600, Frequency::Performance);
            budget.finish();
        }
        budget.record(1200, Frequency::Economy);
        budget.finish();
        assert!(budget.needs_performance());
    }

    #[test]
    fn borderline_work_uses_hysteresis_and_long_work_saturates() {
        let mut budget = WorkBudget::new(1000);
        budget.record(u64::MAX, Frequency::Performance);
        budget.finish();
        assert!(budget.needs_performance());
        for _ in 0..4 {
            budget.record(400, Frequency::Performance); // 800 at 160: in the deadband
            budget.finish();
            assert!(budget.needs_performance());
        }
        budget.record(100, Frequency::Economy);
        budget.finish();
        budget.record(800, Frequency::Economy); // break the light streak
        budget.finish();
        for _ in 0..2 {
            budget.record(100, Frequency::Economy);
            budget.finish();
        }
        assert!(budget.needs_performance());
        budget.finish();
        assert!(!budget.needs_performance());
    }

    #[test]
    fn all_transition_steps_preserve_bus_limits_and_finish_at_fixed_bus_rates() {
        // Model the S31 hardware: configuring CPU also chooses MEM /1 or /2.
        // Start at either supported profile and repeatedly cross both ways.
        for initial in [Frequency::Economy, Frequency::Performance] {
            let mut cpu = initial.mhz();
            let mut memory_divisor = initial.memory_divisor();
            let mut system_divisor = initial.system_divisor();
            for target in [Frequency::Economy, Frequency::Performance]
                .into_iter()
                .cycle()
                .take(20)
            {
                for step in target.transition() {
                    match step {
                        ClockStep::Cpu => {
                            cpu = target.mhz();
                            memory_divisor = u32::from(cpu > 160);
                        }
                        ClockStep::Memory => memory_divisor = target.memory_divisor(),
                        ClockStep::System => system_divisor = target.system_divisor(),
                    }
                    assert!(cpu / (memory_divisor + 1) <= 160);
                    assert!(cpu / (system_divisor + 1) <= 80);
                    assert!(cpu / (system_divisor + 1) / 2 <= 40);
                }
                assert_eq!(cpu / (memory_divisor + 1), 160);
                assert_eq!(cpu / (system_divisor + 1), 80);
            }
        }
    }
}
