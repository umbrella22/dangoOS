"""Driver CLI contracts against the existing USB simulator; no hardware claim."""
import contextlib
import io
import sys
import types
import unittest
from unittest.mock import patch
from support import FakeSerial, session


class DriverDiagnosticsTest(unittest.TestCase):
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
        for action, value in [("screen-off", "0"), ("screen-off", "61"), ("screen-off", "+3"), ("screen-off", "٣"), ("imu-irq", "both"), ("nand-read", "-1"), ("nand-read", "+1"), ("nand-read", "4294967296")]:
            with self.subTest(value=value), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.cli(action, value)
        self.assertEqual(FakeSerial.instances, [])

    def test_discovery_preserves_raw_reply_and_closes_port(self):
        raw="@ok nand-info id=52a71093 a0=38 b0=10 c0=00 geometry=unknown ecc=unknown readonly=1"
        FakeSerial.replies=[(raw+"\n").encode()]
        self.assertEqual(self.cli("nand-info").strip(),raw)
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_page_read_preserves_ecc_range_and_does_not_wake_panel(self):
        raw = "@ok nand-read page=65535 bytes=2048 crc=12345678 oob_crc=abcdef01 c0=10 f0=00 corrected=1:4 marker=na readonly=1"
        FakeSerial.replies = [(raw + "\n").encode()]
        self.assertEqual(self.cli("nand-read", "65535").strip(), raw)
        self.assertEqual(FakeSerial.instances[0].writes, [b"@mosaico nand-read 65535\n"])
        self.assertTrue(FakeSerial.instances[0].closed)

    def test_power_status_preserves_timing_records_before_the_final_ack(self):
        lines = [
            "@power-wake cycle=2 wake_us=100 restore_us=120",
            "@power-panel cycle=2 panel_us=1200120 frame_us=1250000 brightness_us=1250010",
            "@ok power-status state=active mcu=running mode=manual cycle=2 frame_ready=1 wake_to_ready_us=1249910",
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

    def test_imu_configuration_waits_for_same_revision_readback(self):
        FakeSerial.replies=[b"@ok imu-irq requested=int2 rev=7\n",b"@ok imu-irq-status req=2 applied=0 rev=7/6\n",b"@ok imu-irq-status req=2 applied=2 rev=7/7\n"]
        with patch.object(self.api.time,"sleep"):
            result=self.cli("imu-irq","int2")
        self.assertIn("applied=2 rev=7/7",result)
        self.assertEqual(FakeSerial.instances[0].writes.count(b"@mosaico imu-irq-status\n"),2)

    def test_imu_fault_and_superseded_request_are_not_reported_as_success(self):
        for reply in [b"@ok imu-irq-status req=1 applied=3 rev=1/1\n",b"@ok imu-irq-status req=2 applied=2 rev=2/2\n"]:
            FakeSerial.replies=[b"@ok imu-irq requested=int1 rev=1\n",reply]
            with self.assertRaises(SystemExit):
                self.cli("imu-irq","int1")
            self.assertTrue(FakeSerial.instances[-1].closed)


if __name__ == "__main__":
    unittest.main()
