import importlib.util
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("recovery_client", ROOT / "tools/device/recovery.py")
recovery = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(recovery)


class Client:
    def __init__(self, data, fail_at=None):
        self.data, self.fail_at = data, fail_at
        self.identity = "test-device-01"

    def request(self, operation, payload):
        offset, length = struct.unpack("<IH" if operation == "nor" else "<QH", payload)
        if offset == self.fail_at:
            raise OSError("disconnected")
        return self.data[offset:offset + length]


class RecoveryTests(unittest.TestCase):
    def test_framing_crc_noise_and_complete_binary_payload(self):
        payload = bytes(range(256)) * 16
        wire = recovery.encode(73, 3, payload)
        decoder = recovery.Decoder()
        got = []
        for b in b"noiseRCRbroken" + wire:
            got += decoder.feed(bytes([b]))
        self.assertEqual(got, [(73, 3, 0, payload)])
        for pos in (4, 8, 9, 12, 17, len(wire) - 1):
            broken = bytearray(wire); broken[pos] ^= 0x80
            with self.assertRaisesRegex(ValueError, "CRC"):
                recovery.Decoder().feed(broken)

    def test_interrupted_export_resumes_only_verified_prefix(self):
        data = bytes(range(256)) * 100
        with tempfile.TemporaryDirectory() as d:
            output = Path(d) / "nor.bin"
            with self.assertRaises(OSError):
                recovery.export_range(Client(data, 8192), "nor", output, len(data))
            self.assertFalse(output.exists())
            result = recovery.export_range(Client(data), "nor", output, len(data), resume=True)
            self.assertTrue(result["complete"])
            self.assertEqual(output.read_bytes(), data)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_corrupt_partial_export_is_never_silently_resumed(self):
        data = bytes(range(256)) * 100
        with tempfile.TemporaryDirectory() as d:
            output = Path(d) / "nor.bin"
            with self.assertRaises(OSError):
                recovery.export_range(Client(data, 4096), "nor", output, len(data))
            output.with_name("nor.bin.partial").write_bytes(bytes(4096))
            with self.assertRaisesRegex(ValueError, "resume verification"):
                recovery.export_range(Client(data), "nor", output, len(data), resume=True)

    def test_reflashed_medium_cannot_be_mixed_with_an_old_export_prefix(self):
        data = bytes(range(256)) * 100
        with tempfile.TemporaryDirectory() as d:
            output=Path(d)/"nor.bin"
            with self.assertRaises(OSError):
                recovery.export_range(Client(data,4096),"nor",output,len(data))
            modified=bytes([data[0]^1])+data[1:]
            with self.assertRaisesRegex(ValueError,"medium changed"):
                recovery.export_range(Client(modified),"nor",output,len(data),resume=True)
            self.assertFalse(output.exists())

    def test_torn_last_receipt_is_discarded_and_reread(self):
        data=bytes(range(256))*100
        with tempfile.TemporaryDirectory() as d:
            output=Path(d)/"nor.bin"
            with self.assertRaises(OSError):
                recovery.export_range(Client(data,8192),"nor",output,len(data))
            receipt=output.with_name("nor.bin.chunks.jsonl")
            receipt.write_bytes(receipt.read_bytes()+b'{"medium":"nor",')
            result=recovery.export_range(Client(data),"nor",output,len(data),resume=True)
            self.assertTrue(result["complete"])
            self.assertEqual(output.read_bytes(),data)
            self.assertEqual(len([json.loads(s) for s in receipt.read_text().splitlines()]),
                             (len(data)+recovery.PAYLOAD-1)//recovery.PAYLOAD)

    def test_bad_nand_pages_preserve_addresses_oob_and_failure_receipt(self):
        class Nand:
            identity = "test-nand-01"
            def request(self, _, payload):
                page, raw = struct.unpack("<IB", payload)
                if page == 2: raise recovery.DeviceError(2)
                return struct.pack("<IBBBBBBBB", page, raw, 0x20 if page == 1 else 0, 0, 0, 0, 0, int(page == 1), 0) + bytes([page]) * 2176
        with tempfile.TemporaryDirectory() as d:
            output = Path(d) / "nand.bin"
            result = recovery.export_nand(Nand(), output, {"data_bytes": 2048, "spare_bytes": 128, "blocks": 1, "pages_per_block": 4})
            self.assertFalse(result["complete"])
            self.assertEqual(result["read_errors"], [2])
            self.assertEqual(result["uncorrectable_pages"], [1])
            data = output.read_bytes()
            self.assertEqual(len(data), 2176 * 4)
            self.assertEqual(data[3*2176:], bytes([3]) * 2176)
            records = [json.loads(s) for s in output.with_name("nand.bin.pages.jsonl").read_text().splitlines()]
            self.assertEqual(records[2]["page"], 2)
            self.assertEqual(records[2]["placeholder"], "zero")

    def test_install_is_explicit_sequential_and_boot_follows_device_readback(self):
        class Installer:
            def __init__(self): self.calls = []; self.received = bytearray()
            def request(self, op, data=b""):
                self.calls.append(op)
                if op == "install-enter":
                    self.manifest = data
                    return b"installing"
                if op == "install-data":
                    offset, = struct.unpack_from("<I", data)
                    self_offset = len(self.received)
                    if offset != self_offset: raise AssertionError("nonsequential install")
                    self.received.extend(data[4:])
                    return struct.pack("<I", len(self.received))
                if op == "install-finish": return b"trial-ready"
                if op == "boot": return b"reboot"
                raise AssertionError(op)
        data = b"\xe9" + bytes(range(256)) * 50
        info = {"layout": "S31ONE03", "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        client = Installer()
        result = recovery.install_image(client, data, info, boot=True)
        self.assertEqual(bytes(client.received), data)
        self.assertEqual(client.manifest[:16], b"DGINST3\0S31ONE03")
        self.assertEqual(client.manifest[20:], hashlib.sha256(data).digest())
        self.assertEqual(client.calls[-2:], ["install-finish", "boot"])
        self.assertTrue(result["device_readback_verified"])
        client = Installer()
        with self.assertRaisesRegex(ValueError, "changed"):
            recovery.install_image(client, data + b"x", info, boot=True)
        self.assertEqual(client.calls, [])

    def test_failed_install_does_not_retry_write_or_boot(self):
        class Broken:
            def __init__(self): self.calls = []
            def request(self, op, data=b""):
                self.calls.append(op)
                if op == "install-enter": return b"installing"
                if op == "install-leave": return b"read-only"
                raise OSError("USB disconnected with an ambiguous write result")
        data = b"\xe9" + bytes(9999)
        info = {"layout": "S31ONE03", "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        client = Broken()
        with self.assertRaisesRegex(OSError, "disconnected"):
            recovery.install_image(client, data, info, boot=True)
        self.assertEqual(client.calls, ["install-enter", "install-data", "install-leave"])


if __name__ == "__main__":
    unittest.main()
