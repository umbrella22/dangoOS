"""Offline bundle validation and production console transfers; no hardware IO."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

from support import FakeSerial, ROOT, session
sys.path.insert(0, str(ROOT / "scripts/device"))
import resource_bundle as BUNDLES
from test_skin_install import INSTALL, fixture, checksum, Device, Console
from test_release import API as RELEASE


def fonts_fixture(root):
    root.mkdir(parents=True)
    data = b"MUIF\x01\x00\x04\x00" + b"synthetic font payload for transport tests"
    identity = hashlib.sha256(data).hexdigest()
    report = {"schema": 1, "kind": "dango-ui-fonts", "id": identity, "sha256": identity,
              "file": f"fonts-{identity}.bin", "bytes": len(data)}
    (root / report["file"]).write_bytes(data)
    (root / "bundle.json").write_text(json.dumps(report))
    return report


def app_fixture(root):
    (root / "payload/scripts").mkdir(parents=True)
    manifest = {"schema_version": 1, "id": "example", "entry": "scripts/main.lua"}
    (root / "payload/launcher.json").write_text(json.dumps(manifest))
    (root / "payload/scripts/main.lua").write_text("return 42")
    report = {"schema": 1, "kind": "dango-lua-application", "id": "example",
              "files": BUNDLES.inventory(root / "payload"), "documentation": {}}
    (root / "bundle.json").write_text(json.dumps(report))
    return report


class ResourceBundlesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def api(self):
        previous = sys.modules.get("serial")
        sys.modules["serial"] = types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError)
        self.addCleanup(lambda: sys.modules.pop("serial", None) if previous is None else sys.modules.__setitem__("serial", previous))
        api = session.load_api()
        return api, api.Device("fake", False)

    def test_application_inventory_checks_changed_and_extra_payloads(self):
        app_fixture(self.root)
        self.assertEqual(BUNDLES.application(self.root)["id"], "example")
        (self.root / "payload/scripts/main.lua").write_text("return 43")
        with self.assertRaisesRegex(ValueError, "inventory"):
            BUNDLES.application(self.root)

    def test_release_rejects_stale_firmware_pin_even_when_resource_files_match_catalog(self):
        report = fonts_fixture(self.root / "ui-fonts")
        stamp = f"DANGO_UI_FONTS_V1:{report['id']}:{report['bytes']};".encode()
        RELEASE.verify_fonts_pin(b"image segment" + stamp, report)
        for image in (b"old firmware without pin", stamp.replace(report["id"].encode(), b"0" * 64), stamp + stamp):
            with self.assertRaisesRegex(ValueError, "rebuild firmware"):
                RELEASE.verify_fonts_pin(image, report)

    def test_valid_fonts_transfer_checks_running_pin_and_commits_without_reboot(self):
        report = fonts_fixture(self.root / "ui-fonts")
        api, dev = self.api()
        dev.ser.queue = [f"@ok fonts-status expected={report['id']}\n".encode()] + [b"@ok\n"] * 4
        with contextlib.redirect_stdout(io.StringIO()):
            api.push_fonts(dev, self.root / "ui-fonts")
        commands = [b for b in dev.ser.writes if b.startswith(b"@mosaico")]
        self.assertEqual(commands[0], b"@mosaico fonts-status\n")
        self.assertEqual(commands[1], b"@mosaico fonts-begin\n")
        self.assertEqual(commands[-1], b"@mosaico fonts-commit\n")
        self.assertIn((self.root / "ui-fonts" / report["file"]).read_bytes(), dev.ser.writes)
        self.assertFalse(any(b"reboot" in command or b"format" in command for command in commands))

    def test_wrong_firmware_pin_stops_before_starting_transfer(self):
        fonts_fixture(self.root / "ui-fonts")
        api, dev = self.api()
        dev.ser.queue = [b"@ok fonts-status expected=other\n"]
        with self.assertRaisesRegex(ValueError, "running firmware"):
            api.push_fonts(dev, self.root / "ui-fonts")
        self.assertEqual(dev.ser.writes, [b"@mosaico fonts-status\n"])

    def test_bad_font_file_rejected_before_any_device_command(self):
        report = fonts_fixture(self.root / "ui-fonts")
        api, dev = self.api()
        (self.root / "ui-fonts" / report["file"]).write_bytes(b"bad")
        with self.assertRaises(ValueError):
            api.push_fonts(dev, self.root / "ui-fonts")
        self.assertEqual(dev.ser.writes, [])

    def test_failed_body_aborts_without_commit(self):
        report = fonts_fixture(self.root / "ui-fonts")
        api, dev = self.api()
        dev.ser.queue = [f"@ok fonts-status expected={report['id']}\n".encode(),
                         b"@ok begin\n", b"@ok ready\n", b"@err crc\n", b"@ok abort\n"]
        with self.assertRaises(SystemExit):
            api.push_fonts(dev, self.root / "ui-fonts")
        self.assertEqual(dev.ser.writes[-1], b"@mosaico abort\n")
        self.assertFalse(any(b"fonts-commit" in b for b in dev.ser.writes))

    def test_release_preflight_checks_resource_catalogs_before_connection(self):
        manifest = fixture(self.root)
        manifest["ui_fonts"] = fonts_fixture(self.root / "ui-fonts")
        manifest["application_bundles"] = [app_fixture(self.root / "applications/example")]
        (self.root / "tools/resource_bundle.py").write_text("# bundled validator")
        (self.root / "manifest.json").write_text(json.dumps(manifest))
        checksum(self.root)
        self.assertEqual(INSTALL.validate_bundle(self.root), manifest)
        (self.root / "applications/example/payload/scripts/main.lua").write_text("return 100")
        checksum(self.root)
        with patch.object(sys, "argv", ["install.py", "--bundle", str(self.root), "--port", "RUN"]), \
                patch.object(INSTALL, "connect") as connect, patch.object(sys, "stderr"), self.assertRaises(SystemExit):
            INSTALL.main()
        connect.assert_not_called()

    def test_installer_pin_mismatch_precedes_all_writes_and_app_updates_are_opt_in(self):
        manifest = {"ui_fonts": {"id": "expected"}, "nand_skins": [{"id": "eclipse"}],
                    "application_bundles": [{"id": "example"}]}
        device, console = Device(), Console()
        with patch.object(device, "command", return_value="@ok fonts-status expected=wrong"), \
                patch.object(INSTALL, "install_skins") as skins, self.assertRaises(ValueError):
            INSTALL.install_resources(console, device, self.root, manifest)
        skins.assert_not_called()
        for update_apps in (False, True):
            with patch.object(device, "command", return_value="@ok fonts-status expected=expected"), \
                    patch.object(INSTALL, "install_skins"), patch.object(console, "push_fonts", create=True) as fonts, \
                    patch.object(console, "push_bundle", create=True) as apps, contextlib.redirect_stdout(io.StringIO()):
                INSTALL.install_resources(console, device, self.root, manifest, update_apps=update_apps)
            fonts.assert_called_once()
            self.assertEqual(apps.call_count, int(update_apps))
