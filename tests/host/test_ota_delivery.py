"""Exercise the real OTA CLI with synthetic metadata and a fake USB stream.

Fixtures are not bootable. Nothing in this module connects to hardware.
"""
import contextlib
import hashlib
import io
from pathlib import Path
import struct
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
import zlib

from support import FakeSerial, session


def structural_image():
    header = bytearray(24)
    header[:4] = bytes([0xE9, 1, 2, 0x40])
    header[23] = 1
    struct.pack_into("<I", header, 4, 0x40000020)
    struct.pack_into("<H", header, 12, 32)
    segment = bytearray(9000)
    struct.pack_into("<I", segment, 0, 0xABCD5432)
    segment[16:21] = b"0.3.0"
    segment[48:59] = b"mosaico-s31"
    # Command-looking bytes must remain part of the raw payload.
    segment[256:256 + 18] = b"\0@mosaico reboot\n\xff"
    marker = b"MOSAICO_S31_AB_LAYOUT_V2_6M_NAND\0"
    segment[512:512 + len(marker)] = marker
    raw = header + struct.pack("<II", 0x40000020, len(segment)) + segment
    checksum = 0xEF
    for byte in segment:
        checksum ^= byte
    raw.extend(b"\0" * ((len(raw) // 16 + 1) * 16 - 1 - len(raw)))
    raw.append(checksum)
    return bytes(raw) + hashlib.sha256(raw).digest()


class OtaDeliveryTests(unittest.TestCase):
    def setUp(self):
        self.serial = patch.dict(sys.modules, serial=types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError))
        self.serial.start()
        self.addCleanup(self.serial.stop)
        FakeSerial.instances = []
        FakeSerial.replies = []
        self.api = session.load_api()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "structural-only.bin"
        self.data = structural_image()
        self.path.write_bytes(self.data)

    def run_cli(self, *args):
        with patch.object(sys, "argv", ["console.py", *args, "--port", "fake"]):
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.api.main()
        return out.getvalue()

    def success_replies(self):
        return [b"@ok hello mosaico-push 1\n", b"@ok ota-begin 0 1\n"] + [
            f"@ok ota-chunk {min(offset + 4096, len(self.data))}\n".encode()
            for offset in range(0, len(self.data), 4096)
        ] + [b"@ok ota-end\n"]

    def expected_stream(self):
        stream = b"@mosaico hello\n" + f"@mosaico ota-begin {len(self.data)} {hashlib.sha256(self.data).hexdigest()}\n".encode()
        for offset in range(0, len(self.data), 4096):
            chunk = self.data[offset:offset + 4096]
            stream += f"@mosaico ota-chunk {offset} {len(chunk)} {zlib.crc32(chunk):08x}\n".encode() + chunk
        return stream + b"@mosaico ota-end\n"

    def test_exact_chunk_bytes_hash_offsets_and_activation_receipt(self):
        FakeSerial.replies = self.success_replies()
        result = self.run_cli("ota", str(self.path))
        device = FakeSerial.instances[0]
        self.assertEqual(b"".join(device.writes), self.expected_stream())
        self.assertIn("slot 0 -> 1", result)
        self.assertIn(hashlib.sha256(self.data).hexdigest(), result)
        self.assertIn("must report Valid", result)
        self.assertTrue(device.closed)
        self.assertEqual(device.write_timeout, 20.0)

    def test_short_serial_writes_preserve_command_and_payload_bytes(self):
        class PartialSerial(self.api.ConsoleSerial):
            def write(self, data):
                accepted = data[:37]
                self.writes.append(accepted)
                return len(accepted)
        self.api.ConsoleSerial = PartialSerial
        FakeSerial.replies = self.success_replies()
        self.run_cli("ota", str(self.path))
        self.assertEqual(b"".join(FakeSerial.instances[0].writes), self.expected_stream())

    def test_invalid_image_never_opens_serial(self):
        bad_chip = bytearray(self.data)
        struct.pack_into("<H", bad_chip, 12, 9)
        bad_chip[-32:] = hashlib.sha256(bad_chip[:-32]).digest()
        for data in (b"", self.data[:12], bytes(bad_chip), self.data[:-1] + b"\x80", b"x" * (0x600000 + 1)):
            self.path.write_bytes(data)
            with self.assertRaisesRegex(SystemExit, "before connection"):
                self.run_cli("ota", str(self.path))
        self.assertFalse(FakeSerial.instances)

    def test_structurally_valid_legacy_firmware_is_rejected_before_connection(self):
        old = bytearray(self.data)
        marker = b"MOSAICO_S31_AB_LAYOUT_V2_6M_NAND\0"
        at = old.index(marker)
        old[at:at + len(marker)] = b"\0" * len(marker)
        checksum = 0xEF
        length = struct.unpack_from("<I", old, 28)[0]
        for byte in old[32:32 + length]:
            checksum ^= byte
        old[-33] = checksum
        old[-32:] = hashlib.sha256(old[:-32]).digest()
        self.path.write_bytes(old)
        with self.assertRaisesRegex(SystemExit, "A/B layout compatibility"):
            self.run_cli("ota", str(self.path))
        self.assertFalse(FakeSerial.instances)

    def test_descriptor_string_constraints_match_firmware_before_connect(self):
        for offset, value in [(48, b"\xc3\xa9\0"), (48, b"0. 2\0"), (80, b"mosaico-s31\0junk")]:
            data = bytearray(self.data)
            data[offset:offset + 32] = value.ljust(32, b"\0")
            checksum = 0xEF
            for byte in data[32:32 + struct.unpack_from("<I", data, 28)[0]]:
                checksum ^= byte
            data[-33] = checksum
            data[-32:] = hashlib.sha256(data[:-32]).digest()
            self.path.write_bytes(data)
            with self.assertRaisesRegex(SystemExit, "invalid application metadata"):
                self.run_cli("ota", str(self.path))
        self.assertFalse(FakeSerial.instances)

    def test_duplicate_offset_or_crc_error_aborts_without_activation(self):
        for reply in (b"@ok ota-chunk 0\n", b"@err ota-chunk CRC mismatch\n"):
            FakeSerial.replies = [b"@ok hello\n", b"@ok ota-begin 0 1\n", reply, b"@ok ota-abort\n"]
            with self.assertRaisesRegex(SystemExit, "chunk at 0 failed"):
                self.run_cli("ota", str(self.path))
            writes = FakeSerial.instances[-1].writes
            self.assertEqual(writes[-1], b"@mosaico ota-abort\n")
            self.assertNotIn(b"@mosaico ota-end\n", writes)

    def test_unavailable_or_running_target_is_rejected(self):
        for reply in (b"@err OTA unavailable: legacy layout\n", b"@ok ota-begin 1 1\n"):
            FakeSerial.replies = [b"@ok hello\n", reply, b"@ok ota-abort\n"]
            with self.assertRaises(SystemExit):
                self.run_cli("ota", str(self.path))
            self.assertFalse(any(b"ota-chunk" in w for w in FakeSerial.instances[-1].writes))

    def test_missing_end_ack_is_unconfirmed_and_never_retried(self):
        FakeSerial.replies = self.success_replies()[:-1] + [OSError("USB disconnected"), OSError("USB disconnected")]
        with self.assertRaisesRegex(SystemExit, "completion is unconfirmed"):
            self.run_cli("ota", str(self.path))
        self.assertEqual(FakeSerial.instances[0].writes.count(b"@mosaico ota-end\n"), 1)
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_end_ack_is_returned_before_disconnect_while_filling_a_large_read(self):
        class ResetAfterAckSerial(self.api.ConsoleSerial):
            def read(self, count):
                if self.queue and self.queue[0] == b"@ok ota-end\n" and count > len(self.queue[0]):
                    # POSIX pyserial accumulates the ACK, waits for the rest of
                    # count, then raises on reset without returning those bytes.
                    self.queue = []
                    raise OSError("USB reset while waiting to fill read")
                return super().read(count)
        self.api.ConsoleSerial = ResetAfterAckSerial
        FakeSerial.replies = self.success_replies()
        result = self.run_cli("ota", str(self.path))
        self.assertIn("OTA activated: slot 0 -> 1", result)
        self.assertEqual(b"".join(FakeSerial.instances[0].writes), self.expected_stream())

    def test_complete_buffered_reply_is_delivered_before_reading_closed_port(self):
        FakeSerial.replies = [OSError("USB disconnected")]
        dev = self.api.Device("fake", False)
        self.addCleanup(dev.ser.close)
        dev.buf = b"[info] last telemetry\n@ok ota-end\n"
        self.assertEqual(dev.reply(), "@ok ota-end")
        self.assertEqual(len(dev.ser.queue), 1)

    def test_incomplete_end_ack_at_disconnect_is_not_activation(self):
        FakeSerial.replies = self.success_replies()[:-1] + [b"@ok ota-en", OSError("USB disconnected"), OSError("USB disconnected")]
        with self.assertRaisesRegex(SystemExit, "completion is unconfirmed"):
            self.run_cli("ota", str(self.path))
        self.assertEqual(FakeSerial.instances[0].writes.count(b"@mosaico ota-end\n"), 1)

    def test_no_progress_on_payload_aborts_and_closes(self):
        class StalledSerial(self.api.ConsoleSerial):
            def write(self, data):
                return 0 if len(data) > 200 else super().write(data)
        self.api.ConsoleSerial = StalledSerial
        FakeSerial.replies = [b"@ok hello\n", b"@ok ota-begin 0 1\n", b"@ok ota-abort\n"]
        with self.assertRaisesRegex(OSError, "no progress"):
            self.run_cli("ota", str(self.path))
        self.assertTrue(FakeSerial.instances[0].closed)
        self.assertNotIn(b"@mosaico ota-end\n", FakeSerial.instances[0].writes)

    def test_status_and_abort_do_not_upload(self):
        for action in ("ota-status", "ota-abort"):
            FakeSerial.replies = [f"@ok {action} state=idle\n".encode()]
            result = self.run_cli(action)
            self.assertIn("state=idle", result)
            self.assertEqual(FakeSerial.instances[-1].writes, [f"@mosaico {action}\n".encode()])
            self.assertTrue(FakeSerial.instances[-1].closed)

    def test_slot_status_is_visible_without_verbose_logging(self):
        FakeSerial.replies = [b"[info] ordinary log\n@ota-slot slot=0 state=Valid version=0.2.0\n@ota-slot slot=1 state=New version=0.3.0\n@ok ota-status booted=0\n"]
        result = self.run_cli("ota-status")
        self.assertIn("slot=0 state=Valid version=0.2.0", result)
        self.assertIn("slot=1 state=New version=0.3.0", result)
        self.assertIn("@ok ota-status", result)
        self.assertNotIn("ordinary log", result)


if __name__ == "__main__":
    unittest.main()
