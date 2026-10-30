//! Scheduler-backed clock for the resident Lua VM only.
//!
//! Each core accounts only for its own trace notifications. esp-rtos identifies
//! tasks by pointer; `init` registers the calling pinned executor thread.

use esp_hal::system::Cpu;
use esp_sync::NonReentrantMutex;
use os::scheduling::ScheduledClocks;

struct TraceState {
    clocks: ScheduledClocks<1>,
    #[cfg(feature = "radio-scheduling-diagnostics")]
    previous: (u64, u64, u64),
}

impl TraceState {
    const fn new() -> Self {
        Self {
            clocks: ScheduledClocks::new(),
            #[cfg(feature = "radio-scheduling-diagnostics")]
            previous: (0, 0, 0),
        }
    }
}

// Scheduler trace runs with the RTOS scheduler lock held. Embassy channels
// wake threads while holding the global critical section, then acquire that
// scheduler lock. Trace must not acquire the global section in reverse order.
// Each core only touches its own slot, so a local IRQ-protected mutex suffices;
// no clock access can wait for the other core. No wake/log/scheduler call runs
// while a slot is borrowed.
static TRACE: [NonReentrantMutex<TraceState>; Cpu::COUNT] =
    [const { NonReentrantMutex::new(TraceState::new()) }; Cpu::COUNT];

fn with_trace<R>(f: impl FnOnce(&mut TraceState) -> R) -> R {
    TRACE[Cpu::current() as usize].with(f)
}

// #region debug-point F:last-scheduled-task
#[cfg(feature = "radio-hang-diagnostics")]
static LAST_TASK: [core::sync::atomic::AtomicU32; Cpu::COUNT] =
    [const { core::sync::atomic::AtomicU32::new(0) }; Cpu::COUNT];

#[cfg(feature = "radio-hang-diagnostics")]
static LAST_DISPATCH_US: [core::sync::atomic::AtomicU32; Cpu::COUNT] =
    [const { core::sync::atomic::AtomicU32::new(0) }; Cpu::COUNT];

#[cfg(feature = "radio-hang-diagnostics")]
static TRACE_CALLS: [core::sync::atomic::AtomicU32; Cpu::COUNT] =
    [const { core::sync::atomic::AtomicU32::new(0) }; Cpu::COUNT];

#[cfg(feature = "radio-hang-diagnostics")]
pub fn diagnostic_task() -> u32 {
    LAST_TASK[Cpu::current() as usize].load(core::sync::atomic::Ordering::Relaxed)
}

/// Local-core diagnostic values, read without acquiring any scheduler/trace lock.
#[cfg(feature = "radio-hang-diagnostics")]
pub fn diagnostic_dispatch() -> (u32, u32) {
    use core::sync::atomic::Ordering::Relaxed;
    let core = Cpu::current() as usize;
    (
        (now() as u32).wrapping_sub(LAST_DISPATCH_US[core].load(Relaxed)),
        TRACE_CALLS[core].load(Relaxed),
    )
}
// #endregion

fn now() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

pub fn init() {
    let target = esp_radio_rtos_driver::current_task().as_ptr() as usize as u32;
    with_trace(|state| state.clocks.register(0, target, now()));
}

fn switch(next: u32) {
    #[cfg(feature = "radio-hang-diagnostics")]
    {
        use core::sync::atomic::Ordering::Relaxed;
        let core = Cpu::current() as usize;
        LAST_TASK[core].store(next, Relaxed);
        LAST_DISPATCH_US[core].store(now() as u32, Relaxed);
        TRACE_CALLS[core].fetch_add(1, Relaxed);
    }
    with_trace(|state| state.clocks.switch(0, next, now()));
}

/// Includes ISR time while the executor thread is selected; excludes time
/// selected for native radio threads or idle. Other firmware clocks use wall time.
pub fn clock_us() -> u64 {
    let scheduled = with_trace(|state| state.clocks.sample(0, now()));
    // #region debug-point D:native-preemption
    #[cfg(feature = "radio-scheduling-diagnostics")]
    {
        let observation = with_trace(|state| {
            let t = now();
            let (last_wall, last_scheduled, last_log) = state.previous;
            let wall = t.saturating_sub(last_wall);
            let running = scheduled.saturating_sub(last_scheduled);
            let gap = wall.saturating_sub(running);
            let emit = wall <= 100_000 && gap >= 10_000 && t.saturating_sub(last_log) >= 1_000_000;
            state.previous = (t, scheduled, if emit { t } else { last_log });
            emit.then_some((wall, running, gap))
        });
        if let Some((wall, running, gap)) = observation {
            log::warn!(
                "brain scheduling: wall_us={wall} scheduled_us={running} preempted_us={gap}"
            );
        }
    }
    // #endregion
    scheduled
}

struct SchedulerTrace;
rtos_trace::global_trace!(SchedulerTrace);
impl rtos_trace::RtosTrace for SchedulerTrace {
    fn start() {}
    fn stop() {}
    fn task_new(_: u32) {}
    fn task_send_info(_id: u32, _info: rtos_trace::TaskInfo) {
        // #region debug-point D-E:classic-audio-routing-task-stacks
        #[cfg(feature = "radio-hang-diagnostics")]
        log::info!("audio routing task: id={_id:#x} name={} priority={} stack={:#x} words={}",
            _info.name, _info.priority, _info.stack_base, _info.stack_size);
        // #endregion
    }
    fn task_new_stackless(_: u32, _: &'static str, _: u32) {}
    fn task_terminate(_: u32) {}
    fn task_exec_begin(id: u32) {
        switch(id)
    }
    fn task_exec_end() {
        switch(0)
    }
    fn task_ready_begin(_: u32) {}
    fn task_ready_end(_: u32) {}
    fn system_idle() {
        switch(0)
    }
    fn isr_enter() {}
    fn isr_exit() {}
    fn isr_exit_to_scheduler() {}
    fn name_marker(_: u32, _: &'static str) {}
    fn marker(_: u32) {}
    fn marker_begin(_: u32) {}
    fn marker_end(_: u32) {}
}
