"""Run the controller's production OSAL event functions against a bounded queue."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


def function(source, name):
    start = source.index("fn " + name + "(")
    body = source.index("{", start)
    depth = 1
    end = body + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class ClassicEventQueueTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler required")
    def test_pending_event_coalescing_and_repost(self):
        source = (ROOT / "vendor/esp-radio/src/ble/btdm2/mod.rs").read_text()
        types = source[source.index("#[repr(C)]\n#[derive(Default, Clone, Copy)]\nstruct BtdmOsalPtr"):
                       source.index("struct Callout {")]
        names = ["event_inner", "event_try_queue", "wr_btdm_osal_eventq_get",
                 "wr_btdm_osal_eventq_put", "wr_btdm_osal_eventq_put_to_front",
                 "wr_btdm_osal_eventq_remove", "wr_btdm_osal_event_is_queued",
                 "wr_btdm_osal_event_get_arg", "wr_btdm_osal_event_set_arg"]
        fixture = (ROOT / "tests/host/fixtures/classic_event_queue.rs").read_text()
        with tempfile.TemporaryDirectory(prefix="mosaico-eventq-") as tmp:
            tmp = pathlib.Path(tmp)
            harness = tmp / "eventq.rs"
            harness.write_text(fixture + "\n" + types + "\n" +
                               "\n".join(function(source, name) for name in names))
            binary = tmp / "eventq"
            build = subprocess.run(["rustc", "--edition=2024", "--test", str(harness),
                                    "-o", str(binary)], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
