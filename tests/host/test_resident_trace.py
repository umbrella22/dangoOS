"""The scheduler trace must finish while another core holds the channel lock."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]

FIXTURE = r'''
extern crate self as esp_hal;
extern crate self as esp_radio_rtos_driver;
extern crate self as esp_sync;
extern crate self as critical_section;
extern crate self as os;
extern crate self as log;
#[macro_export] macro_rules! warn { ($($arg:tt)*) => {{}} }
pub mod platform { pub mod runtime { pub mod scheduling { SCHEDULING_SOURCE } } }
static CHANNEL_LOCK_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
thread_local! { static CORE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub mod system {
    #[derive(Clone, Copy)] pub struct Cpu;
    impl Cpu {
        pub const COUNT: usize = 2;
        pub fn current() -> usize { crate::CORE.with(|core| core.get()) }
    }
}
pub mod time {
    pub struct Instant;
    pub struct Duration;
    impl Instant {
        pub fn now() -> Self { Self }
        pub fn duration_since_epoch(&self) -> Duration { Duration }
    }
    impl Duration {
        pub fn as_micros(&self) -> u64 { crate::WALL.load(std::sync::atomic::Ordering::Relaxed) }
    }
}
pub fn current_task() -> std::ptr::NonNull<()> {
    std::ptr::NonNull::new(((system::Cpu::current() + 1) * 16) as *mut ()).unwrap()
}
pub struct NonReentrantMutex<T>(std::sync::Mutex<T>);
impl<T> NonReentrantMutex<T> {
    pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R { f(&mut self.0.lock().unwrap()) }
}
pub struct Mutex<T>(T);
unsafe impl<T: Send> Sync for Mutex<T> {}
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(value) }
    pub fn borrow(&self, _: CriticalSection) -> &T { &self.0 }
}
#[derive(Clone, Copy)] pub struct CriticalSection;
pub fn with<R>(f: impl FnOnce(CriticalSection) -> R) -> R {
    assert!(!CHANNEL_LOCK_HELD.load(std::sync::atomic::Ordering::Relaxed),
        "scheduler trace tried to acquire the global channel critical section");
    f(CriticalSection)
}
'''

SCENARIO = r'''
#[cfg(test)] mod regression {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;
    fn select(core: usize, wall: u64) {
        crate::CORE.with(|id| id.set(core));
        crate::WALL.store(wall, Relaxed);
    }
    #[test] fn trace_does_not_wait_for_a_channel_holder_on_the_other_core() {
        select(0, 0); init();
        select(1, 100); init();
        // Core1 holds the channel critical section and needs the RTOS lock
        // to wake Core0. Core0 holds the RTOS lock while dispatching trace.
        crate::CHANNEL_LOCK_HELD.store(true, Relaxed);
        select(0, 1_000); switch(0x30);
        select(1, 10_100); assert_eq!(clock_us(), 10_000);
        switch(0);
        select(0, 20_000); assert_eq!(clock_us(), 1_000);
        switch(0x10);
        select(1, 100_100); assert_eq!(clock_us(), 10_000);
        crate::CHANNEL_LOCK_HELD.store(false, Relaxed);
    }
}
'''


class ResidentTraceTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler required")
    def test_scheduler_trace_does_not_acquire_the_global_channel_lock(self):
        source = (ROOT / "crates/os/src/platform/runtime/resident_budget.rs").read_text()
        source = source[:source.index("struct SchedulerTrace;")].replace("//!", "//")
        scheduling = (ROOT / "crates/os/src/platform/runtime/scheduling.rs").read_text().split("#[cfg(test)]")[0]
        fixture = FIXTURE.replace("SCHEDULING_SOURCE", scheduling)
        with tempfile.TemporaryDirectory(prefix="mosaico-resident-trace-") as tmp:
            tmp = pathlib.Path(tmp)
            harness = tmp / "trace.rs"
            harness.write_text(fixture + "\nmod resident {\n" + source + SCENARIO + "\n}\n")
            binary = tmp / "production"
            command = ["rustc", "--edition=2021", "--test", str(harness), "-o", str(binary)]
            build = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
