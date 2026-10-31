//! Elapsed time for one scheduled thread, excluding other threads and idle.
//!
//! Interrupts that run while the thread is selected remain charged to it.
//! This is a budget clock, not an instruction profiler or a wall clock.

#[derive(Clone, Copy)]
pub struct ScheduledClock {
    running: bool,
    since_us: u64,
    elapsed_us: u64,
}

impl ScheduledClock {
    pub const fn new(now_us: u64, running: bool) -> Self {
        Self {
            running,
            since_us: now_us,
            elapsed_us: 0,
        }
    }

    /// Account for the old selection before changing the scheduled thread.
    pub fn switch(&mut self, now_us: u64, running: bool) {
        self.elapsed_us = self.sample(now_us);
        self.since_us = now_us;
        self.running = running;
    }

    pub fn sample(&self, now_us: u64) -> u64 {
        if self.running {
            self.elapsed_us
                .saturating_add(now_us.saturating_sub(self.since_us))
        } else {
            self.elapsed_us
        }
    }
}

/// Independent resident clocks for executor threads pinned to different cores.
/// Trace notifications from one core must never change another core's selection.
#[derive(Clone, Copy)]
pub struct ScheduledClocks<const CORES: usize> {
    targets: [u32; CORES],
    clocks: [ScheduledClock; CORES],
}

impl<const CORES: usize> ScheduledClocks<CORES> {
    pub const fn new() -> Self {
        Self {
            targets: [0; CORES],
            clocks: [ScheduledClock::new(0, false); CORES],
        }
    }

    pub fn register(&mut self, core: usize, task: u32, now_us: u64) {
        self.targets[core] = task;
        self.clocks[core] = ScheduledClock::new(now_us, true);
    }

    pub fn switch(&mut self, core: usize, task: u32, now_us: u64) {
        self.clocks[core].switch(
            now_us,
            self.targets[core] != 0 && task == self.targets[core],
        );
    }

    pub fn sample(&self, core: usize, now_us: u64) -> u64 {
        self.clocks[core].sample(now_us)
    }
}

impl<const CORES: usize> Default for ScheduledClocks<CORES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_core_switches_do_not_spend_or_pause_the_ui_core_budget() {
        let mut clocks = ScheduledClocks::<2>::new();
        clocks.register(0, 10, 0);
        clocks.register(1, 20, 100);
        clocks.switch(0, 30, 1_000); // native radio task on Core0
        clocks.switch(0, 0, 2_000); // Core0 idle
        assert_eq!(clocks.sample(1, 10_100), 10_000);
        clocks.switch(1, 0, 10_100);
        clocks.switch(0, 10, 100_000);
        clocks.switch(0, 30, 110_000);
        assert_eq!(clocks.sample(1, 1_010_100), 10_000);
        clocks.switch(1, 20, 1_010_100);
        assert_eq!(clocks.sample(1, 1_020_100), 20_000);
        assert_eq!(clocks.sample(0, 1_020_100), 11_000);
    }

    #[test]
    fn registering_second_executor_preserves_the_first_clock() {
        let mut clocks = ScheduledClocks::<2>::new();
        clocks.register(0, 10, 100);
        clocks.switch(0, 0, 1_100);
        clocks.register(1, 20, 50_000);
        assert_eq!(clocks.sample(0, 60_000), 1_000);
        assert_eq!(clocks.sample(1, 60_000), 10_000);
    }

    #[test]
    fn other_threads_and_idle_do_not_spend_the_callback_budget() {
        let mut clock = ScheduledClock::new(100, true);
        assert_eq!(clock.sample(1_100), 1_000);
        clock.switch(1_100, false);
        assert_eq!(clock.sample(501_100), 1_000);
        clock.switch(501_100, false); // another thread followed by idle
        clock.switch(1_001_100, true);
        assert_eq!(clock.sample(1_002_100), 2_000);
        assert_eq!(clock.sample(1_002_100), 2_000); // reads do not charge twice
    }

    #[test]
    fn repeated_selection_preserves_elapsed_time() {
        let mut clock = ScheduledClock::new(0, true);
        clock.switch(10_000, true);
        assert_eq!(clock.sample(20_000), 20_000);
        clock.switch(30_000, false);
        clock.switch(40_000, true);
        assert_eq!(clock.sample(50_000), 40_000);
    }

    #[test]
    fn resident_vm_excludes_preemption_but_still_exhausts_execution_time() {
        use alloc::boxed::Box;
        use core::cell::Cell;
        use lua_engine::{Host, Limits, Step, Vm, GLOBAL_ALLOCATOR};
        std::thread_local! {
            static STATE: Cell<(u64, ScheduledClock)> = const { Cell::new((0, ScheduledClock::new(0, true))) };
        }
        fn clock() -> u64 {
            STATE.with(|s| {
                let (now, c) = s.get();
                c.sample(now)
            })
        }
        fn switch(now: u64, running: bool) {
            STATE.with(|s| {
                let (_, mut c) = s.get();
                c.switch(now, running);
                s.set((now, c));
            });
        }
        struct Quiet;
        impl Host for Quiet {
            fn print(&mut self, _: &[u8]) {}
        }
        let mut vm = Vm::new(
            Box::new(Quiet),
            Limits {
                clock: Some(clock),
                ..Limits::default()
            },
            &GLOBAL_ALLOCATOR,
            0,
        )
        .unwrap();
        vm.load("scheduled-budget", b"coroutine.yield(); local s=0; for i=1,1000 do s=s+i end; coroutine.yield(); while true do end").unwrap();
        vm.set_callback_budget(2_000, 20_000);
        assert_eq!(vm.resume(), Step::Slice);
        switch(1_000, false);
        switch(1_001_000, true); // one second in native radio threads
        assert_eq!(vm.resume(), Step::Slice);
        STATE.with(|s| {
            let (_, c) = s.get();
            s.set((1_021_001, c));
        });
        assert!(
            matches!(vm.resume(), Step::Error(ref e) if e.contains("callback budget exhausted (wall;"))
        );
    }
}
