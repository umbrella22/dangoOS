"""Driver CLI contracts against the existing USB simulator; no hardware claim."""
import contextlib
import io
import sys
import types
import unittest
from unittest.mock import patch
from support import FakeSerial, session


class DeviceStatusTest(unittest.TestCase):
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

    def test_invalid_parameters_never_open_a_port(self):
        for action, value in [("screen-off", "0"), ("screen-off", "61"), ("screen-off", "+3"), ("screen-off", "٣")]:
            with self.subTest(value=value), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.cli(action, value)
        self.assertEqual(FakeSerial.instances, [])

    def test_discovery_preserves_raw_reply_and_closes_port(self):
        raw="@ok nand-info id=52a71093 a0=38 b0=10 c0=00 geometry=unknown ecc=unknown readonly=1"
        FakeSerial.replies=[(raw+"\n").encode()]
        self.assertEqual(self.cli("nand-info").strip(),raw)
        self.assertTrue(FakeSerial.instances[0].closed)


    def test_power_status_reports_readiness_without_timing_records(self):
        lines = [
            "@ok power-status state=active mcu=running mode=manual cycle=2 frame_ready=1",
        ]
        FakeSerial.replies = [("\n".join(lines) + "\n").encode()]
        self.assertEqual(self.cli("power-status").strip().splitlines(), lines)
        self.assertEqual(FakeSerial.instances[0].writes, [b"@mosaico power-status\n"])

    def test_screen_standby_keeps_connection_and_waits_for_restore(self):
        FakeSerial.replies=[b"@ok screen-off state=screen-off\n",b"@ok power-status state=screen-off\n",b"@ok power-status state=active\n",b"@ok screen-on state=active\n"]
        with patch.object(self.api.time,"sleep"):
            self.cli("screen-off","30")
        port=FakeSerial.instances[0]
        self.assertEqual(port.writes,[b"@mosaico screen-off 30\n",b"@mosaico power-status\n",b"@mosaico power-status\n",b"@mosaico screen-on\n"])
        self.assertTrue(port.closed)

    def test_ctrl_c_restores_panel_before_closing(self):
        FakeSerial.replies=[b"@ok screen-off state=screen-off\n",b"@ok power-status state=screen-off\n",b"@ok screen-on state=active\n"]
        with patch.object(self.api.time,"sleep",side_effect=KeyboardInterrupt), self.assertRaises(KeyboardInterrupt):
            self.cli("screen-off","30")
        self.assertEqual(FakeSerial.instances[0].writes[-1],b"@mosaico screen-on\n")
        self.assertTrue(FakeSerial.instances[0].closed)




if __name__ == "__main__":
    unittest.main()
