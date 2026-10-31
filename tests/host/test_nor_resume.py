"""Exercise the production S31 resume path with an idle-core register model.

This checks the wake protocol and register preservation, not real S31 timing.
The board's full OTA/readback check remains necessary.
"""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


def resume_function():
    source = (ROOT / "crates/os/src/platform/runtime/cores.rs").read_text()
    start = source.index("fn resume_core(")
    body = source.index("{", start)
    depth, end = 1, body + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


HARNESS = r'''
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
static STALL_CODE: AtomicU32 = AtomicU32::new(0);
static STALLED: AtomicU32 = AtomicU32::new(0);
static READS: AtomicU32 = AtomicU32::new(0);
static KICKS: AtomicU32 = AtomicU32::new(0);
mod esp_hal {
    pub mod system {
        #[derive(Clone, Copy, Debug)]
        pub enum Cpu { ProCpu, AppCpu }
    }
    pub mod peripherals {
        use super::super::*;
        pub struct PMU;
        pub struct HP_SYS;
        pub struct PmuRegs;
        pub struct HpRegs;
        pub struct StallReg;
        pub struct StatusReg;
        pub struct Writer { value: u32 }
        pub struct Field<'a> { writer: &'a mut Writer, shift: u32 }
        pub struct Reader(u32);
        pub struct Bit(u32);
        impl PMU { pub fn regs() -> PmuRegs { PmuRegs } }
        impl HP_SYS { pub fn regs() -> HpRegs { HpRegs } }
        impl PmuRegs { pub fn cpu_stall_sw(&self) -> StallReg { StallReg } }
        impl HpRegs { pub fn cpu_corestalled_st(&self) -> StatusReg { StatusReg } }
        impl StallReg {
            pub fn modify(&self, f: impl for<'a> FnOnce((), &'a mut Writer) -> &'a mut Writer) {
                let mut writer = Writer { value: STALL_CODE.load(SeqCst) };
                f((), &mut writer);
                STALL_CODE.store(writer.value, SeqCst);
            }
        }
        impl Writer {
            pub fn hpcore0_sw_stall_code(&mut self) -> Field<'_> { Field { writer: self, shift: 24 } }
            pub fn hpcore1_sw_stall_code(&mut self) -> Field<'_> { Field { writer: self, shift: 16 } }
        }
        impl<'a> Field<'a> {
            pub unsafe fn bits(self, value: u8) -> &'a mut Writer {
                self.writer.value = (self.writer.value & !(0xff << self.shift)) | ((value as u32) << self.shift);
                self.writer
            }
        }
        impl StatusReg {
            pub fn read(&self) -> Reader {
                assert!(READS.fetch_add(1, SeqCst) < 8, "stalled wait made no wake progress");
                Reader(STALLED.load(SeqCst))
            }
        }
        impl Reader {
            pub fn reg_core0_corestalled_st(&self) -> Bit { Bit(self.0 & 1) }
            pub fn reg_core1_corestalled_st(&self) -> Bit { Bit(self.0 & 2) }
        }
        impl Bit { pub fn bit_is_set(&self) -> bool { self.0 != 0 } }
    }
    pub mod interrupt {
        pub mod __rtos_implementation {
            use super::super::super::*;
            use super::super::system::Cpu;
            pub fn request_context_switch(core: Cpu) {
                let (shift, bit) = match core { Cpu::ProCpu => (24, 1), Cpu::AppCpu => (16, 2) };
                assert_eq!((STALL_CODE.load(SeqCst) >> shift) & 0xff, 0xff, "wake before releasing software stall");
                assert_eq!(STALLED.load(SeqCst), bit, "wrong core woken");
                // Model an idle reentry after the first wake: the waiter must
                // keep waking while CORESTALLED remains asserted.
                if KICKS.fetch_add(1, SeqCst) == 1 { STALLED.store(0, SeqCst); }
            }
        }
    }
}
#[test]
fn resumes_each_idle_core_and_preserves_other_pmu_fields() {
    use esp_hal::system::Cpu;
    for (core, bit, expected) in [(Cpu::ProCpu, 1, 0xff86_1234), (Cpu::AppCpu, 2, 0x86ff_1234)] {
        STALL_CODE.store(0x8686_1234, SeqCst);
        STALLED.store(bit, SeqCst);
        READS.store(0, SeqCst);
        KICKS.store(0, SeqCst);
        resume_core(core);
        assert_eq!(STALL_CODE.load(SeqCst), expected);
        assert_eq!(KICKS.load(SeqCst), 2);
        assert_eq!(STALLED.load(SeqCst), 0);
        // A core already running needs no extra interrupt.
        READS.store(0, SeqCst);
        KICKS.store(0, SeqCst);
        resume_core(core);
        assert_eq!(KICKS.load(SeqCst), 0);
        assert_eq!(STALL_CODE.load(SeqCst), expected);
    }
}
'''


class NorResumeTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler required")
    def test_idle_wake_progress_and_missing_wake_regression(self):
        source = resume_function()
        with tempfile.TemporaryDirectory(prefix="mosaico-nor-resume-") as tmp:
            tmp = pathlib.Path(tmp)
            for missing_wake in (False, True):
                with self.subTest(missing_wake=missing_wake):
                    code = source
                    if missing_wake:
                        code = code.replace("esp_hal::interrupt::__rtos_implementation::request_context_switch(other);", "")
                        self.assertNotEqual(code, source)
                    script = tmp / "resume.rs"
                    script.write_text(HARNESS + "\n" + code)
                    binary = tmp / "resume"
                    build = subprocess.run(["rustc", "--edition=2021", "--test", str(script), "-o", str(binary)], capture_output=True, text=True)
                    self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
                    run = subprocess.run([str(binary)], capture_output=True, text=True, timeout=5)
                    if missing_wake:
                        self.assertNotEqual(run.returncode, 0)
                        self.assertIn("stalled wait made no wake progress", run.stdout + run.stderr)
                    else:
                        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
