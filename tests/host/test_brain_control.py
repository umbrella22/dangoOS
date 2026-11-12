"""Rust behavior diagnostics and the removed script delivery boundary."""
import contextlib
import io
import sys
import types
import unittest
from unittest.mock import patch
from support import FakeSerial, session


class BrainControlTest(unittest.TestCase):
    def setUp(self):
        serial = patch.dict(sys.modules, serial=types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError))
        serial.start()
        self.addCleanup(serial.stop)
        FakeSerial.instances = []
        FakeSerial.replies = []
        self.api = session.load_api()

    def run_cli(self, *args):
        with patch.object(sys, "argv", ["console.py", *args, "--port", "fake"]):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.api.main()
        return out.getvalue()

    def test_status_reports_firmware_behavior(self):
        FakeSerial.replies = [b"@ok brain-status active=rust source=firmware\n"]
        self.assertIn("active=rust source=firmware", self.run_cli("brain-status"))
        self.assertEqual(FakeSerial.instances[0].writes, [b"@mosaico brain-status\n"])

    def test_removed_mutations_fail_before_opening_usb(self):
        for command in ["brain-push", "brain-reload", "brain-default"]:
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    self.run_cli(command)
        self.assertFalse(FakeSerial.instances)


if __name__ == "__main__":
    unittest.main()
