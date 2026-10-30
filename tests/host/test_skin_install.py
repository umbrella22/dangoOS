"""Installer preflight and NAND orchestration; no device or network access."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zlib

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("skin_install", ROOT / "scripts/release/install.py")
INSTALL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def fixture(root):
    binary = b"synthetic skin checked by CRC; decoder validation is exercised by app-pack"
    metadata = {"schema_version": 1, "id": "eclipse", "name": "月蚀", "version": "test",
                "canvas": [352, 336], "binary_bytes": len(binary), "binary_crc32": zlib.crc32(binary)}
    files = {"dango-skin/eclipse/fox.bin": binary,
             "dango-skin/eclipse/skin.json": json.dumps(metadata).encode(),
             "install.py": b"# installer", "tools/console.py": b"# console",
             "tools/ota_image.py": b"# image tools", "dangoOS-v0.3.0-esp32s31-ota.bin": b"application"}
    catalog = [{"id": "eclipse", "name": "月蚀", "version": "test", "files": {
        name: {"bytes": len(files[f"dango-skin/eclipse/{name}"]),
               "sha256": digest(files[f"dango-skin/eclipse/{name}"])} for name in ("fox.bin", "skin.json")}}]
    manifest = {"schema": 1, "project": "dangoOS", "version": "0.3.0", "tag": "v0.3.0",
                "chip": "esp32s31", "flash_bytes": 16 * 1024 * 1024,
                "layout": "MOSAICO_S31_AB_LAYOUT_V2_6M_NAND", "nand_skins": catalog,
                "images": {"ota": {"file": "dangoOS-v0.3.0-esp32s31-ota.bin", "bytes": 11,
                                   "sha256": digest(b"application")},
                           "full": {"file": "dangoOS-v0.3.0-esp32s31-full.bin", "bytes": 16 * 1024 * 1024,
                                    "sha256": digest(b"\xff" * (16 * 1024 * 1024)), "flash_offset": 0}}}
    files["manifest.json"] = json.dumps(manifest).encode()
    for name, data in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_bytes(data)
    checksum(root)
    return manifest


def checksum(root):
    files = sorted(p for p in root.rglob("*") if p.is_file() and p.name != "SHA256SUMS")
    (root / "SHA256SUMS").write_text("".join(f"{digest(p.read_bytes())}  {p.relative_to(root).as_posix()}\n" for p in files))


class Device:
    def __init__(self, mounted=True, statuses=()):
        self.mounted = mounted
        self.statuses = iter(statuses)
        self.commands = []
        self.installed = {"default", "custom"}
        self.selected = "custom"

    def command(self, command, **kwargs):
        self.commands.append(command)
        if command == "nand-status":
            return next(self.statuses, f"@ok nand-status mounted={int(self.mounted)} ready=1")
        if command == "nand-format":
            self.mounted = True
            return "@ok nand-format mounted=1"
        if command == "skin-ls":
            return "@ok skin-ls " + " ".join(sorted(self.installed))
        raise AssertionError(f"unexpected command: {command}")


class Console:
    def __init__(self, fail=None):
        self.pushes = []
        self.fail = fail

    @staticmethod
    def response_fields(line):
        return dict(field.split("=", 1) for field in line.split() if "=" in field)

    def push_skin(self, device, root, select):
        self.pushes.append((Path(root).name, select))
        if Path(root).name == self.fail:
            raise SystemExit("skin-commit rejected")
        device.installed.add(Path(root).name)


class SkinInstallTests(unittest.TestCase):
    def test_preflight_accepts_ota_or_full_bundle_and_checks_images(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = fixture(root)
            self.assertEqual(INSTALL.validate_bundle(root), manifest)
            (root / manifest["images"]["ota"]["file"]).unlink()
            (root / manifest["images"]["full"]["file"]).write_bytes(b"\xff" * (16 * 1024 * 1024))
            checksum(root)
            self.assertEqual(INSTALL.validate_bundle(root), manifest)
            with patch.object(INSTALL.subprocess, "run") as run:
                INSTALL.flash_full(root, manifest, "ROM")
                self.assertEqual(run.call_count, 1)
                self.assertEqual(run.call_args.args[0][-2:], ["0x0", str(root / manifest["images"]["full"]["file"])])

    def test_corrupt_skin_is_rejected_before_flash_or_connection(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            (root / "dango-skin/eclipse/fox.bin").write_bytes(b"corrupt")
            with patch.object(sys, "argv", ["install.py", "--bundle", str(root), "--port", "RUN", "--flash-port", "ROM"]), \
                    patch.object(INSTALL, "flash_full") as flash, patch.object(INSTALL, "connect") as connect, \
                    patch.object(sys, "stderr"), self.assertRaises(SystemExit) as raised:
                INSTALL.main()
            self.assertEqual(raised.exception.code, 2)
            flash.assert_not_called()
            connect.assert_not_called()

    def test_missing_unlisted_or_metadata_mismatch_is_rejected(self):
        for mode in ("missing", "extra", "crc", "image-path", "catalog-hash"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                manifest = fixture(root)
                if mode == "missing":
                    (root / "dango-skin/eclipse/fox.bin").unlink()
                elif mode == "extra":
                    (root / "dango-skin/eclipse/source.png").write_bytes(b"private")
                elif mode == "crc":
                    path = root / "dango-skin/eclipse/skin.json"
                    metadata = json.loads(path.read_bytes())
                    metadata["binary_crc32"] ^= 1
                    data = json.dumps(metadata).encode()
                    path.write_bytes(data)
                    manifest["nand_skins"][0]["files"]["skin.json"] = {"bytes": len(data), "sha256": digest(data)}
                elif mode == "image-path":
                    manifest["images"]["full"]["file"] = "../outside.bin"
                else:
                    manifest["nand_skins"][0]["files"]["fox.bin"]["sha256"] = "0" * 64
                (root / "manifest.json").write_text(json.dumps(manifest))
                checksum(root)
                with self.assertRaises((ValueError, OSError)):
                    INSTALL.validate_bundle(root)

    def test_checksum_path_cannot_escape_bundle(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            (root / "SHA256SUMS").write_text("0" * 64 + "  ../secret\n")
            with self.assertRaisesRegex(ValueError, "unsafe"):
                INSTALL.validate_bundle(root)

    def test_installs_without_changing_selection_or_formatting_mounted_nand(self):
        for allow_format in (False, True):
            with self.subTest(allow_format=allow_format):
                device, console = Device(), Console()
                INSTALL.install_skins(console, device, Path("/bundle"), [{"id": "eclipse"}, {"id": "normal"}], allow_format)
                self.assertEqual(console.pushes, [("eclipse", False), ("normal", False)])
                self.assertEqual(device.installed, {"default", "custom", "eclipse", "normal"})
                self.assertEqual(device.selected, "custom")
                self.assertNotIn("nand-format", device.commands)

    def test_unmounted_nand_requires_explicit_flag(self):
        device, console = Device(mounted=False), Console()
        with self.assertRaisesRegex(ValueError, "--format-nand"):
            INSTALL.install_skins(console, device, Path("/bundle"), [{"id": "eclipse"}])
        self.assertEqual(console.pushes, [])
        self.assertNotIn("nand-format", device.commands)

    def test_waits_for_boot_probe_then_formats_only_with_flag(self):
        device = Device(mounted=False, statuses=["@ok nand-status mounted=0 ready=0"])
        with patch.object(INSTALL.time, "sleep"):
            INSTALL.install_skins(Console(), device, Path("/bundle"), [{"id": "eclipse"}], True)
        self.assertEqual(device.commands, ["nand-status", "nand-status", "nand-format", "skin-ls"])

    def test_failed_skin_commit_stops_remaining_installs(self):
        device, console = Device(), Console(fail="normal")
        with self.assertRaisesRegex(SystemExit, "commit rejected"):
            INSTALL.install_skins(console, device, Path("/bundle"), [{"id": i} for i in ("eclipse", "normal", "black-blue")])
        self.assertEqual(console.pushes, [("eclipse", False), ("normal", False)])
        self.assertEqual(device.selected, "custom")
        self.assertNotIn("skin-ls", device.commands)


if __name__ == "__main__":
    unittest.main()
