"""Run the production frequency backend against a serialized host clock model.

The forced interleaving covers a performance request arriving after a downshift
has checked demand but before the hardware divider changes. It does not model
real S31 clock timing, DMA, or core parking; those require device acceptance.
"""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]

HARNESS = r'''
#![allow(dead_code, non_camel_case_types)]
extern crate alloc;
extern crate self as esp_hal;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
static CPU_DIV: AtomicU32 = AtomicU32::new(0);
static MEM_DIV: AtomicU32 = AtomicU32::new(1);
static SYS_DIV: AtomicU32 = AtomicU32::new(3);
static CLOCK_ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static PAUSE_ARMED: AtomicBool = AtomicBool::new(false);
static DOWN_PAUSED: AtomicBool = AtomicBool::new(false);
static REQUEST_RETURNED: AtomicBool = AtomicBool::new(false);
static DOWN_FINISHED: AtomicBool = AtomicBool::new(false);

fn wait_until(f: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !f() {
        assert!(started.elapsed().as_secs() < 2, "clock-model rendezvous timed out");
        std::thread::yield_now();
    }
}

pub mod clock {
    pub struct Rate(pub u32);
    impl Rate { pub fn as_mhz(&self) -> u32 { self.0 / 1_000_000 } }
    pub fn cpu_clock() -> Rate { Rate(ll::cpu_clk_frequency()) }
    pub enum CpuClock { _320MHz }
    pub struct ClockConfig { pub sys_clk: Option<ll::SysClkConfig> }
    impl From<CpuClock> for ClockConfig {
        fn from(_: CpuClock) -> Self { Self { sys_clk: Some(ll::SysClkConfig::new(3)) } }
    }
    pub mod ll {
        use super::super::*;
        pub struct ClockTree;
        static TREE: std::sync::Mutex<ClockTree> = std::sync::Mutex::new(ClockTree);
        impl ClockTree {
            pub fn with<R>(f: impl FnOnce(&mut ClockTree) -> R) -> R {
                CLOCK_ATTEMPTS.fetch_add(1, SeqCst);
                f(&mut *TREE.lock().unwrap())
            }
        }
        macro_rules! config {
            ($name:ident) => {
                pub struct $name(u32);
                impl $name { pub fn new(divisor: u32) -> Self { Self(divisor) } }
            };
        }
        config!(CpuClkConfig);
        config!(MemClkConfig);
        config!(SysClkConfig);
        pub fn configure_cpu_clk(_: &mut ClockTree, config: CpuClkConfig) {
            CPU_DIV.store(config.0, SeqCst);
            MEM_DIV.store(u32::from(cpu_clk_frequency() > 160_000_000), SeqCst);
        }
        pub fn configure_mem_clk(_: &mut ClockTree, config: MemClkConfig) { MEM_DIV.store(config.0, SeqCst); }
        pub fn configure_sys_clk(_: &mut ClockTree, config: SysClkConfig) { SYS_DIV.store(config.0, SeqCst); }
        pub fn cpu_clk_frequency() -> u32 { 320_000_000 / (CPU_DIV.load(SeqCst) + 1) }
        pub fn mem_clk_frequency() -> u32 { cpu_clk_frequency() / (MEM_DIV.load(SeqCst) + 1) }
        pub fn sys_clk_frequency() -> u32 { cpu_clk_frequency() / (SYS_DIV.load(SeqCst) + 1) }
        pub fn apb_clk_frequency() -> u32 { sys_clk_frequency() / 2 }
    }
}

pub mod time {
    pub struct Instant(std::time::Instant);
    pub struct Duration(std::time::Duration);
    impl Instant {
        pub fn now() -> Self { Self(std::time::Instant::now()) }
        pub fn elapsed(&self) -> Duration { Duration(self.0.elapsed()) }
        pub fn duration_since_epoch(&self) -> Duration {
            Duration(std::time::Duration::from_millis(u64::MAX))
        }
    }
    impl Duration {
        pub fn as_micros(&self) -> u64 { self.0.as_micros() as u64 }
        pub fn as_millis(&self) -> u64 { self.0.as_millis() as u64 }
    }
}

pub mod peripherals {
    use super::*;
    pub struct HP_SYS_CLKRST;
    pub struct Registers;
    pub struct Register(&'static AtomicU32);
    pub struct Reader(u32);
    pub struct Field(u32);
    impl HP_SYS_CLKRST { pub fn regs() -> Registers { Registers } }
    impl Registers {
        pub fn cpu_freq_ctrl0(&self) -> Register { Register(&CPU_DIV) }
        pub fn mem_freq_ctrl0(&self) -> Register { Register(&MEM_DIV) }
        pub fn sys_freq_ctrl0(&self) -> Register { Register(&SYS_DIV) }
    }
    impl Register { pub fn read(&self) -> Reader { Reader(self.0.load(SeqCst)) } }
    impl Reader {
        pub fn cpu_clk_div_num(&self) -> Field { Field(self.0) }
        pub fn mem_clk_div_num(&self) -> Field { Field(self.0) }
        pub fn sys_clk_div_num(&self) -> Field { Field(self.0) }
    }
    impl Field {
        pub fn bits(&self) -> u8 { self.0 as u8 }
        pub fn bit(&self) -> bool { self.0 != 0 }
    }
}

pub mod platform {
    pub mod runtime {
        pub mod cores {
            pub unsafe fn with_other_parked<R>(f: impl FnOnce() -> R) -> R {
                use crate::*;
                if PAUSE_ARMED.swap(false, SeqCst) {
                    DOWN_PAUSED.store(true, SeqCst);
                    // A correct requester attempts the locked path. With the
                    // regression it returns early instead. Allow either, then
                    // let the real downshift code change the divider.
                    wait_until(|| CLOCK_ATTEMPTS.load(SeqCst) >= 2 || REQUEST_RETURNED.load(SeqCst));
                }
                f()
            }
        }
    }
}
'''

TESTS = r'''
#[test]
fn budget_skips_waiting_workers_and_scoped_work_does_not_renew_touch_hold() {
    enable();
    let interaction_before = demand().interaction;
    let mut budget = Budget::new(1);
    budget.run(true, || std::thread::sleep(std::time::Duration::from_micros(100)));
    budget.finish();
    apply(Frequency::Economy, demand());
    budget.run(false, || {
        assert_eq!(demand().requests, 0, "idle worker poll acquired a request");
        assert_eq!(clock::cpu_clock().as_mhz(), 160);
    });
    budget.finish();
    budget.run(true, || {
        assert_eq!(demand().requests, 1);
        assert_eq!(clock::cpu_clock().as_mhz(), 320);
    });
    assert_eq!(demand().requests, 0, "work request escaped its synchronous scope");
    assert_eq!(demand().interaction, interaction_before, "work renewed touch hold");
    let mut policy = super::Policy::new(0);
    assert_eq!(policy.target(3000, demand(), false), Frequency::Economy);
    apply(Frequency::Economy, demand());
    apply(Frequency::Performance, demand());
}

#[test]
fn performance_request_cannot_be_lost_between_demand_check_and_divider_write() {
    use crate::*;
    enable();
    CLOCK_ATTEMPTS.store(0, SeqCst);
    PAUSE_ARMED.store(true, SeqCst);
    let down = std::thread::spawn(|| {
        apply(Frequency::Economy, demand());
        DOWN_FINISHED.store(true, SeqCst);
    });
    wait_until(|| DOWN_PAUSED.load(SeqCst));
    let request = performance();
    REQUEST_RETURNED.store(true, SeqCst);
    wait_until(|| DOWN_FINISHED.load(SeqCst));
    down.join().unwrap();
    assert_eq!(clock::cpu_clock().as_mhz(), 320, "performance request lost to concurrent downshift");
    assert_eq!(demand().requests, 1);
    drop(request);
    assert_eq!(demand().requests, 0);
    apply(Frequency::Economy, demand());
    assert_eq!(clock::cpu_clock().as_mhz(), 160);
    apply(Frequency::Performance, demand());
}

'''


def without_module_docs(source):
    return "\n".join(line for line in source.splitlines() if not line.startswith("//!"))


class CpuFrequencyTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler required")
    def test_serialized_request_and_unlocked_fast_path_mutation(self):
        policy = without_module_docs((ROOT / "crates/os/src/platform/runtime/frequency.rs").read_text())
        backend = without_module_docs((ROOT / "crates/os/src/platform/runtime/frequency/device.rs").read_text())
        with tempfile.TemporaryDirectory(prefix="mosaico-cpu-frequency-") as tmp:
            tmp = pathlib.Path(tmp)
            for unlocked_fast_path in (False, True):
                with self.subTest(unlocked_fast_path=unlocked_fast_path):
                    code = backend
                    if unlocked_fast_path:
                        code = code.replace(
                            "if !ENABLED.load(Ordering::Acquire) {",
                            "if !ENABLED.load(Ordering::Acquire) || clock::cpu_clock().as_mhz() == target.mhz() {",
                        )
                        self.assertNotEqual(code, backend)
                    script = tmp / "frequency.rs"
                    script.write_text(HARNESS + "\nmod frequency {\n" + policy + "\npub mod device {\n" + code + TESTS + "\n}\n}\n")
                    binary = tmp / "frequency"
                    build = subprocess.run(["rustc", "--edition=2021", "--test", str(script), "-o", str(binary)], capture_output=True, text=True)
                    self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
                    run = subprocess.run([str(binary), "--test-threads=1"], capture_output=True, text=True, timeout=10)
                    if unlocked_fast_path:
                        self.assertNotEqual(run.returncode, 0)
                        self.assertIn("performance request lost to concurrent downshift", run.stdout + run.stderr)
                    else:
                        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
