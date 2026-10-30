"""Exercise the production radio task dispatch with a recording scheduler."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class RadioAffinityTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler required")
    def test_unspecified_workers_follow_firmware_policy(self):
        source = (ROOT / "vendor/esp-radio-rtos-driver/src/lib.rs").read_text()
        start = source.index("pub unsafe fn task_create(")
        body = source.index("{", start)
        depth, end = 1, body + 1
        while depth:
            depth += (source[end] == "{") - (source[end] == "}")
            end += 1
        harness = r'''
use std::{ffi::c_void, ptr::NonNull, sync::Mutex};
type ThreadPtr = NonNull<()>;
static CALLS: Mutex<Vec<(u32, Option<u32>, usize)>> = Mutex::new(Vec::new());
macro_rules! debug { ($($arg:tt)*) => {{}} }
fn max_task_priority() -> u32 { 23 }
unsafe fn esp_rtos_task_create(
    name: &str, _task: extern "C" fn(*mut c_void), param: *mut c_void,
    priority: u32, core: Option<u32>, stack: usize,
) -> ThreadPtr {
    assert_eq!(name, "timer");
    assert_eq!(param, NonNull::<()>::dangling().as_ptr().cast());
    CALLS.lock().unwrap().push((priority, core, stack));
    NonNull::dangling()
}
extern "C" fn worker(_: *mut c_void) {}
#[test]
fn dispatches_timer_and_preserves_explicit_affinities() {
    for core in [None, Some(0), Some(1)] {
        let task = unsafe {
            task_create("timer", worker, NonNull::<()>::dangling().as_ptr().cast(), 99, core, 8192)
        };
        assert_eq!(task, NonNull::dangling());
    }
    let default = if cfg!(feature = "default-core0") { Some(0) } else { None };
    assert_eq!(*CALLS.lock().unwrap(), vec![(23, default, 8192), (23, Some(0), 8192), (23, Some(1), 8192)]);
}
'''
        with tempfile.TemporaryDirectory(prefix="mosaico-radio-affinity-") as tmp:
            tmp = pathlib.Path(tmp)
            test = tmp / "affinity.rs"
            test.write_text(harness + "\n" + source[start:end])
            for enabled in [False, True]:
                with self.subTest(default_core0=enabled):
                    binary = tmp / ("pinned" if enabled else "upstream")
                    command = ["rustc", "--edition=2024", "--test", str(test), "-o", str(binary)]
                    if enabled:
                        command += ["--cfg", 'feature="default-core0"']
                    build = subprocess.run(command, capture_output=True, text=True)
                    self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
                    result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
