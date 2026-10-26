"""Brain delivery tests use the real CLI and a simulated USB reply stream."""
import contextlib
import io
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
import zlib
from support import FakeSerial, session


class BrainControlTest(unittest.TestCase):
    def setUp(self):
        self.serial = patch.dict(sys.modules, serial=types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError))
        self.serial.start()
        self.addCleanup(self.serial.stop)
        FakeSerial.instances = []
        FakeSerial.replies = []
        self.api = session.load_api()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "brain.lua"
        self.data = b"return {on_event=function()end,tick=function()fox.set_expression('Happy')end}"
        self.path.write_bytes(self.data)

    def run_cli(self, *args):
        with patch.object(sys, "argv", ["console.py", *args, "--port", "fake"]):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.api.main()
        return out.getvalue()

    def test_push_waits_for_real_commit_and_transfers_exact_source(self):
        FakeSerial.replies = [b"@ok hello mosaico-push 1\n", b"@ok brain-begin\n", b"@ok ready source.lua\n", b"@ok file source.lua\n", b"[INFO] validated\n@ok brain-commit\n"]
        out = self.run_cli("brain-push", str(self.path))
        self.assertIn("@ok brain-commit", out)
        self.assertEqual(FakeSerial.instances[0].writes, [
            b"@mosaico hello\n", b"@mosaico brain-begin\n",
            f"@mosaico file source.lua {len(self.data)} {zlib.crc32(self.data):08x}\n".encode(),
            self.data, b"@mosaico brain-commit\n",
        ])

    def test_failed_candidate_reports_failure_and_aborts(self):
        FakeSerial.replies = [b"@ok hello mosaico-push 1\n", b"@ok brain-begin\n", b"@ok ready source.lua\n", b"@ok file source.lua\n", b"@err brain-commit: callback budget exhausted\n", b"@ok abort\n"]
        with self.assertRaisesRegex(SystemExit, "callback budget exhausted"):
            self.run_cli("brain-push", str(self.path))
        self.assertEqual(FakeSerial.instances[0].writes[-1], b"@mosaico abort\n")

    def test_bad_local_source_rejected_before_serial_connection(self):
        for data in [b"", b"x" * 65537, b"\xff"]:
            self.path.write_bytes(data)
            with self.assertRaises(SystemExit):
                self.run_cli("brain-push", str(self.path))
        self.assertFalse(FakeSerial.instances)

    def test_control_commands_print_result(self):
        for command in ["brain-status", "brain-default", "brain-reload"]:
            FakeSerial.replies = [f"@ok {command} active=lua\n".encode()]
            self.assertIn(f"@ok {command}", self.run_cli(command))
            self.assertEqual(FakeSerial.instances[-1].writes, [f"@mosaico {command}\n".encode()])


if __name__ == "__main__":
    unittest.main()
