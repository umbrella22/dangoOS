"""Photo delivery uses the actual CLI against a bounded fake USB reply stream."""
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


class PhotoDeliveryTest(unittest.TestCase):
    def setUp(self):
        self.serial = patch.dict(sys.modules, serial=types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError))
        self.serial.start()
        self.addCleanup(self.serial.stop)
        FakeSerial.instances = []
        FakeSerial.replies = []
        self.api = session.load_api()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "photo.jpg"
        self.data = b"\xff\xd8binary\0pixels\xff\xd9"
        self.path.write_bytes(self.data)

    def run_cli(self, *args):
        with patch.object(sys, "argv", ["console.py", *args, "--port", "fake"]):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.api.main()
        return out.getvalue()

    def test_bytes_nested_name_commit_and_optional_album_run(self):
        name = "2026/photo.JPEG"
        FakeSerial.replies = [b"@ok hello mosaico-push 1\n", b"@ok photo-begin\n", f"@ok ready {name}\n".encode(), f"@ok file {name}\n".encode(), f"@ok photo-commit {name}\n".encode(), b"@ok run photo\n"]
        result = self.run_cli("photo-push", str(self.path), "--name", name, "--run")
        self.assertIn("/data/appdata/photo/photos/2026/photo.JPEG", result)
        self.assertEqual(FakeSerial.instances[0].writes, [b"@mosaico hello\n", b"@mosaico photo-begin\n", f"@mosaico file {name} {len(self.data)} {zlib.crc32(self.data):08x}\n".encode(), self.data, f"@mosaico photo-commit {name}\n".encode(), b"@mosaico run photo\n"])
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_open_preserves_pending_console_input_and_raises_dtr_after_open(self):
        FakeSerial.replies = [b"initial telemetry\r\n@ok hello mosaico-push 1\r\n"]
        dev = self.api.Device("fake", True)
        self.addCleanup(dev.ser.close)
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(dev.command("hello"), "@ok hello mosaico-push 1")
        self.assertIn("initial telemetry", output.getvalue())
        self.assertEqual(dev.ser.open_states, [False])
        self.assertTrue(dev.ser.dtr)
        self.assertEqual(dev.ser.input_resets, 0)
        # Only the implicit open discard is suppressed; an explicit reset works.
        dev.ser._reset_input_buffer()
        self.assertEqual(dev.ser.input_resets, 1)

    def test_partial_writes_preserve_entire_binary_body(self):
        class PartialSerial(self.api.ConsoleSerial):
            def write(self, data):
                accepted = data[:3]
                self.writes.append(accepted)
                return len(accepted)

        self.api.ConsoleSerial = PartialSerial
        data = self.data + b"\0@mosaico abort\n\xff"
        self.path.write_bytes(data)
        FakeSerial.replies = [b"@ok hello\n", b"@ok photo-begin\n", b"@ok ready photo.jpg\n", b"@ok file photo.jpg\n", b"@ok photo-commit photo.jpg\n", b"@ok run photo\n"]
        self.run_cli("photo-push", str(self.path), "--run")
        self.assertEqual(b"".join(FakeSerial.instances[0].writes),
                         b"@mosaico hello\n@mosaico photo-begin\n" +
                         f"@mosaico file photo.jpg {len(data)} {zlib.crc32(data):08x}\n".encode() +
                         data + b"@mosaico photo-commit photo.jpg\n@mosaico run photo\n")

    def test_payload_disconnect_or_zero_write_does_not_send_text_cleanup(self):
        for failure in (OSError("disconnected mid-body"), 0):
            with self.subTest(failure=failure):
                class FailingSerial(self.api.ConsoleSerial):
                    def write(inner, data):
                        if data == self.data:
                            inner.writes.append(data[:2])
                            return 2
                        if data == self.data[2:]:
                            if isinstance(failure, Exception):
                                raise failure
                            return failure
                        return super().write(data)

                self.api.ConsoleSerial = FailingSerial
                FakeSerial.replies = [b"@ok hello\n", b"@ok photo-begin\n", b"@ok ready photo.jpg\n"]
                with self.assertRaises(OSError):
                    self.run_cli("photo-push", str(self.path), "--run")
                dev = FakeSerial.instances[-1]
                self.assertEqual(dev.writes[-1], self.data[:2])
                self.assertTrue(dev.closed)

    def test_uncertain_ready_or_file_receipt_does_not_abort_or_commit(self):
        for replies in ([OSError("ready lost")],
                        [b"@ok ready another.jpg\n"],
                        [b"@ok ready photo.jpg\n", OSError("file receipt lost")],
                        [b"@ok ready photo.jpg\n", b"@ok file another.jpg\n"]):
            with self.subTest(replies=replies):
                FakeSerial.replies = [b"@ok hello\n", b"@ok photo-begin\n", *replies]
                with self.assertRaises((OSError, SystemExit)):
                    self.run_cli("photo-push", str(self.path), "--run")
                dev = FakeSerial.instances[-1]
                self.assertTrue(dev.closed)
                self.assertFalse(any(b"@mosaico abort\n" == w or b"photo-commit" in w or b"run photo" in w for w in dev.writes))

    def test_explicit_header_rejection_allows_abort_without_payload(self):
        FakeSerial.replies = [b"@ok hello\n", b"@ok photo-begin\n", b"@err file: no space\n", b"@ok abort\n"]
        with self.assertRaisesRegex(SystemExit, "no space"):
            self.run_cli("photo-push", str(self.path), "--run")
        self.assertNotIn(self.data, FakeSerial.instances[-1].writes)
        self.assertEqual(FakeSerial.instances[-1].writes[-1], b"@mosaico abort\n")

    def test_bad_local_file_or_path_rejected_before_connection(self):
        for name in ["../outside.jpg", "/other.jpg", ".hidden.jpg", "x.lua", "a b.jpg", "a\n.jpg", "a/" * 6 + "x.jpg"]:
            with self.assertRaises(SystemExit):
                self.run_cli("photo-push", str(self.path), "--name", name)
        for data in [b"", b"x" * (1024 * 1024 + 1)]:
            self.path.write_bytes(data)
            with self.assertRaises(SystemExit):
                self.run_cli("photo-push", str(self.path))
        self.assertFalse(FakeSerial.instances)

    def test_failed_crc_aborts_without_commit_or_launch(self):
        FakeSerial.replies = [b"@ok hello\n", b"@ok photo-begin\n", b"@ok ready photo.jpg\n", b"@err file photo.jpg: crc mismatch\n", b"@ok abort\n"]
        with self.assertRaisesRegex(SystemExit, "crc mismatch"):
            self.run_cli("photo-push", str(self.path), "--run")
        writes = FakeSerial.instances[0].writes
        self.assertEqual(writes[-1], b"@mosaico abort\n")
        self.assertFalse(any(b"photo-commit" in w or b"run photo" in w for w in writes))

    def test_failed_rename_is_not_reported_as_completed_or_launched(self):
        FakeSerial.replies = [b"@ok hello\n", b"@ok photo-begin\n", b"@ok ready photo.jpg\n", b"@ok file photo.jpg\n", b"@err photo commit: no space\n", b"@ok abort\n"]
        with self.assertRaisesRegex(SystemExit, "no space"):
            self.run_cli("photo-push", str(self.path), "--run")
        self.assertNotIn(b"@mosaico run photo\n", FakeSerial.instances[0].writes)


if __name__ == "__main__":
    unittest.main()
