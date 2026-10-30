"""Credential framing, multiline scan delivery and bounded wireless CLI input."""
import contextlib
import io
import sys
import types
import unittest
from unittest.mock import patch
from support import FakeSerial, session


class RadioConsoleTest(unittest.TestCase):
    def setUp(self):
        mock = patch.dict(sys.modules, serial=types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError))
        mock.start()
        self.addCleanup(mock.stop)
        FakeSerial.instances = []
        FakeSerial.replies = []
        self.api = session.load_api()

    def cli(self, *args):
        with patch.object(sys, "argv", ["console.py", *args, "--port", "fake"]), contextlib.redirect_stdout(io.StringIO()) as out:
            self.api.main()
        return out.getvalue()

    def test_password_is_prompted_locally_and_command_preserves_ssid_spaces(self):
        FakeSerial.replies = [b"@ok wifi-connect ip=192.0.2.1/24\n"]
        with patch.object(self.api.getpass, "getpass", return_value="my password") as prompt:
            text = self.cli("wifi-connect", "test network")
        prompt.assert_called_once()
        self.assertNotIn("my password", text)
        self.assertEqual(FakeSerial.instances[0].writes, [b"@mosaico wifi-connect 74657374206e6574776f726b 6d792070617373776f7264\n"])
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_short_non_ascii_or_oversized_password_fails_before_opening_serial(self):
        for value in ("short", "x" * 64, "\n" * 8, "密" * 8):
            with self.subTest(length=len(value)), patch.object(self.api.getpass, "getpass", return_value=value), contextlib.redirect_stderr(io.StringIO()) as errors, self.assertRaises(SystemExit):
                self.cli("wifi-connect", "network")
            self.assertNotIn(repr(value), errors.getvalue())
        self.assertEqual(FakeSerial.instances, [])

    def test_full_length_wire_command_fits_firmware_line_without_truncation(self):
        line = "@mosaico " + self.api.wifi_connect_command("界" * 10 + "ab", "x" * 63)
        self.assertLessEqual(len(line.encode()), 256)
        self.assertEqual(bytes.fromhex(line.split()[2]), ("界" * 10 + "ab").encode())

    def test_scan_keeps_network_records_before_ack(self):
        lines = ["@wifi-ap ssid=74657374206e6574 ch=6 rssi=-45 auth=Some(Wpa2Personal)", "@ok wifi-scan count=1 band=2.4GHz"]
        FakeSerial.replies = [("\n".join(lines) + "\n").encode()]
        self.assertEqual(self.cli("wifi-scan").strip().splitlines(), lines)

    def test_saved_network_commands_keep_identity_and_never_request_a_key(self):
        for action in ("wifi-saved-connect", "wifi-forget"):
            FakeSerial.replies = [(f"@ok {action}\n").encode()]
            with patch.object(self.api.getpass, "getpass") as prompt:
                self.cli(action, " a b ")
            prompt.assert_not_called()
            self.assertEqual(FakeSerial.instances[-1].writes, [(f"@mosaico {action} 2061206220\n").encode()])
        FakeSerial.replies = [b"@ok wifi-auto automatic=0\n"]
        self.cli("wifi-auto", "off")
        self.assertEqual(FakeSerial.instances[-1].writes, [b"@mosaico wifi-auto off\n"])
        FakeSerial.instances = []
        for args in (("wifi-auto", "yes"), ("wifi-forget", "x" * 33)):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.cli(*args)
        self.assertFalse(FakeSerial.instances)

    def test_enable_command_reaches_the_radio_service(self):
        FakeSerial.replies = [b"@ok wifi-on state=on\n"]
        self.assertEqual(self.cli("wifi-on").strip(), "@ok wifi-on state=on")
        self.assertEqual(FakeSerial.instances[0].writes, [b"@mosaico wifi-on\n"])
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_update_url_retains_query_and_rejects_framing_before_port_open(self):
        url = "https://updates.example:8443/manifest.json?build=a%2Bb"
        FakeSerial.replies = [b"@ok update queued\n"]
        self.cli("update-url", url)
        self.assertEqual(FakeSerial.instances[0].writes, [("@mosaico update-url " + url + "\n").encode()])
        FakeSerial.instances = []
        for bad in ("http://updates.example/x", "https://example/x\n@mosaico reboot", "https://user@example/x", "https://example:0/x"):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.cli("update-url", bad)
        self.assertEqual(FakeSerial.instances, [])

    def test_invalid_probe_cannot_inject_headers_and_never_opens_port(self):
        for args in (("wifi-probe", "host\r\nHeader:value"), ("wifi-probe", "host", "--tcp-port", "0"), ("wifi-probe", "host", "--tcp-port", "65536"), ("bt-on", "--tcp-port", "80")):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.cli(*args)
        self.assertEqual(FakeSerial.instances, [])

    def test_update_proxy_address_and_disable_reach_device_without_injection(self):
        for value in ("10.26.104.1:7777", "off"):
            FakeSerial.replies = [b"@ok update queued\n"]
            self.cli("update-proxy", value)
            self.assertEqual(FakeSerial.instances[-1].writes, [("@mosaico update-proxy " + value + "\n").encode()])
        FakeSerial.instances = []
        for value in ("10.26.104.1:0", "10.26.104.1:65536", "0.0.0.0:7777", "http://10.26.104.1:7777", "10.26.104.1:7777\n@mosaico reboot"):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.cli("update-proxy", value)
        self.assertEqual(FakeSerial.instances, [])

    def test_device_errors_close_port_without_printing_credentials(self):
        FakeSerial.replies = [b"@err wifi-connect association timeout\n"]
        with patch.object(self.api.getpass, "getpass", return_value="password"), self.assertRaises(SystemExit) as error:
            self.cli("wifi-connect", "network")
        self.assertNotIn("70617373776f7264", str(error.exception))
        self.assertNotIn("password", str(error.exception))
        self.assertTrue(FakeSerial.instances[0].closed)


if __name__ == "__main__":
    unittest.main()
