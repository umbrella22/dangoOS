"""Run production BTU teardown before its worker could initialize the stack."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


def function(source, signature):
    start = source.index(signature)
    opening = source.index("{", start)
    depth, end = 1, opening + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class ClassicPartialInitTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("cc"), "native C compiler required")
    def test_missing_worker_repeated_cleanup_and_retry(self):
        source = (ROOT / "vendor/esp-bluedroid/upstream/host/bluedroid/stack/btu/btu_task.c").read_text()
        fixture = (ROOT / "tests/host/fixtures/classic_partial_init.c").read_text()
        implementation = "\nstatic bool btu_task_initialized;\n" + "\n".join(
            function(source, signature) for signature in
            ("void btu_task_start_up(void *param)", "void btu_task_shut_down(void)"))
        with tempfile.TemporaryDirectory(prefix="mosaico-partial-init-") as tmp:
            harness = pathlib.Path(tmp) / "partial-init.c"
            harness.write_text(fixture.replace("/* PRODUCTION_IMPLEMENTATION */", implementation))
            binary = pathlib.Path(tmp) / "partial-init"
            build = subprocess.run(["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                                    str(harness), "-o", str(binary)], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
