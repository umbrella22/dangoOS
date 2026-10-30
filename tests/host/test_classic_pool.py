"""Exercise the firmware's real controller pool adapter with its 3 KiB backing."""
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class ClassicPoolTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler required")
    def test_controller_pool(self):
        cargo_home = pathlib.Path(os.environ.get("CARGO_HOME", pathlib.Path.home() / ".cargo"))
        candidates = sorted((cargo_home / "registry/src").glob("*/linked_list_allocator-0.10.6/src/lib.rs"))
        self.assertTrue(candidates, "locked linked_list_allocator source must be cached")
        with tempfile.TemporaryDirectory(prefix="mosaico-pool-") as tmp:
            tmp = pathlib.Path(tmp)
            library = tmp / "liblinked_list_allocator.rlib"
            subprocess.run(["rustc", "--edition=2021", "--crate-type=rlib",
                            "--crate-name=linked_list_allocator", str(candidates[0]),
                            "-o", str(library)], check=True, capture_output=True, text=True)
            # A wrapper preserves pub(super) visibility of the production module.
            source = ROOT / "vendor/esp-radio/src/ble/btdm2/classic/pool.rs"
            harness = tmp / "harness.rs"
            harness.write_text(f'#[path="{source}"] mod pool;\n')
            binary = tmp / "pool-tests"
            subprocess.run(["rustc", "--edition=2021", "--test", str(harness),
                            "--extern", f"linked_list_allocator={library}", "-o", str(binary)],
                           check=True, capture_output=True, text=True)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
