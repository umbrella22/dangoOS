"""Offline transport simulations; never opens a device or imports pyserial."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest

sys.dont_write_bytecode = True
SCRIPT = Path(__file__).resolve().parents[1] / "n1-device-session.py"
spec = importlib.util.spec_from_file_location("n1_session", SCRIPT)
session = importlib.util.module_from_spec(spec)
spec.loader.exec_module(session)


class FakeSerial:
    replies = []
    instances = []

    def __init__(self, *args, **kwargs):
        self.closed = False
        self.writes = []
        self.queue = list(self.replies)
        self.instances.append(self)

    def read(self, count):
        item = self.queue.pop(0) if self.queue else b""
        if isinstance(item, Exception):
            raise item
        return item

    def write(self, data):
        self.writes.append(data)
        return len(data)

    def close(self):
        self.closed = True


class SessionTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.log = Path(self.temp.name) / "receipt.log"
        FakeSerial.instances = []
        FakeSerial.replies = []
        module = types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError)
        self.old_serial = sys.modules.get("serial")
        sys.modules["serial"] = module
        self.addCleanup(self.restore_serial)

    def restore_serial(self):
        if self.old_serial is None:
            sys.modules.pop("serial", None)
        else:
            sys.modules["serial"] = self.old_serial

    def run_session(self, *extra):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return session.main(["--log", str(self.log), "--wait", "0", "--seconds", "0", *extra])

    def test_commands_order_success_and_close(self):
        FakeSerial.replies = [b"boot log\n@ok status\n", b"@ok ls\n"]
        self.assertEqual(self.run_session("--command", "status", "--command", "ls"), 0)
        dev = FakeSerial.instances[0]
        self.assertEqual(dev.writes, [b"@mosaico status\n", b"@mosaico ls\n"])
        self.assertTrue(dev.closed)
        self.assertEqual(dev.write_timeout, 20.0)
        log = self.log.read_text()
        for marker in ("session_started=", "connected=", "session_finished=", "+00:00", "boot log"):
            self.assertIn(marker, log)

    def test_error_reply_is_failure(self):
        FakeSerial.replies = [b"@err rejected\n"]
        self.assertEqual(self.run_session("--command", "status"), 1)
        self.assertTrue(FakeSerial.instances[0].closed)
        self.assertIn("@err rejected", self.log.read_text())

    def test_disconnect_is_failure(self):
        FakeSerial.replies = [OSError("disconnected")]
        self.assertEqual(self.run_session("--command", "status"), 2)
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_write_timeout_is_failure_and_closes(self):
        class SerialTimeoutException(OSError):
            pass

        class WriteTimeoutSerial(FakeSerial):
            def write(self, data):
                self.asserted_timeout = self.write_timeout
                raise SerialTimeoutException("write timed out")

        sys.modules["serial"].Serial = WriteTimeoutSerial
        self.assertEqual(self.run_session("--command", "status"), 2)
        dev = FakeSerial.instances[0]
        self.assertEqual(dev.asserted_timeout, 20.0)
        self.assertTrue(dev.closed)
        self.assertIn("write timed out", self.log.read_text())

    def test_disconnect_during_follow_is_failure(self):
        FakeSerial.replies = [OSError("follow disconnected")]
        self.assertEqual(self.run_session("--seconds", "0.01"), 2)
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_unavailable_port_wait_zero_attempts_once(self):
        attempts = []

        def unavailable(*args, **kwargs):
            attempts.append(args)
            raise OSError("port unavailable")

        sys.modules["serial"].Serial = unavailable
        self.assertEqual(self.run_session(), 2)
        self.assertEqual(len(attempts), 1)
        self.assertIn("session_finished=", self.log.read_text())

    def test_missing_reply_is_failure(self):
        # Use real Device.reply and command, with an empty bounded iterator.
        api = session.load_api()
        api.Device.lines = lambda self, deadline: iter(())
        original = session.load_api
        session.load_api = lambda: api
        self.addCleanup(setattr, session, "load_api", original)
        self.assertEqual(self.run_session("--command", "status"), 1)
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_real_push_success(self):
        FakeSerial.replies = [b"@ok\n"] * 7
        app = SCRIPT.parents[1] / "examples/lua-apps/n1_haptic_probe"
        self.assertEqual(self.run_session("--push", str(app)), 0)
        writes = FakeSerial.instances[0].writes
        self.assertEqual(writes[0], b"@mosaico hello\n")
        self.assertEqual(writes[-1], b"@mosaico commit n1_haptic_probe run\n")
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_real_push_payload_failure_aborts(self):
        FakeSerial.replies = [b"@ok\n"] * 3 + [b"@err crc\n", b"@ok abort\n"]
        app = SCRIPT.parents[1] / "examples/lua-apps/n1_haptic_probe"
        self.assertEqual(self.run_session("--push", str(app)), 1)
        self.assertEqual(FakeSerial.instances[0].writes[-1], b"@mosaico abort\n")
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_existing_log_not_overwritten(self):
        self.log.write_text("existing evidence")
        self.assertEqual(self.run_session(), 2)
        self.assertEqual(self.log.read_text(), "existing evidence")
        self.assertEqual(FakeSerial.instances, [])

    def test_help_without_site_packages_and_invalid_durations(self):
        result = subprocess.run([sys.executable, "-S", str(SCRIPT), "--help"], capture_output=True)
        self.assertEqual(result.returncode, 0)
        for flag in ("--wait", "--seconds"):
            for value in ("nan", "inf", "-inf", "-1", "nope"):
                result = subprocess.run([sys.executable, "-S", str(SCRIPT), "--log", str(self.log), f"{flag}={value}"], capture_output=True)
                self.assertEqual(result.returncode, 2, (flag, value))
                self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()
