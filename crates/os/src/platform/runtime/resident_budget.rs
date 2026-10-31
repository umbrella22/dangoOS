//! Scheduler-backed clock for the resident Lua VM only.
//!
//! Each core accounts only for its own trace notifications. esp-rtos identifies
//! tasks by pointer; `init` registers the calling pinned executor thread.

use crate::platform::runtime::scheduling::ScheduledClocks;
use esp_hal::system::Cpu;
use esp_sync::NonReentrantMutex;

struct TraceState {
    clocks: ScheduledClocks<1>,
}

impl TraceState {
    const fn new() -> Self {
        Self {
            clocks: ScheduledClocks::new(),
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
    with_trace(|state| state.clocks.switch(0, next, now()));
}

/// Includes ISR time while the executor thread is selected; excludes time
/// selected for native radio threads or idle. Other firmware clocks use wall time.
pub fn clock_us() -> u64 {
    with_trace(|state| state.clocks.sample(0, now()))
}

struct SchedulerTrace;
rtos_trace::global_trace!(SchedulerTrace);
impl rtos_trace::RtosTrace for SchedulerTrace {
    fn start() {}
    fn stop() {}
    fn task_new(_: u32) {}
    fn task_send_info(_id: u32, _info: rtos_trace::TaskInfo) {}
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
