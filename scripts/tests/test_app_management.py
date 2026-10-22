"""Management CLI protocol receipts, without opening a physical serial port."""
import contextlib
import io
import sys
import types
import unittest
from unittest.mock import patch
from test_n1_device_session import FakeSerial, session

class AppManagementTest(unittest.TestCase):
    def setUp(self):
        self.serial = patch.dict(sys.modules, serial=types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError))
        self.serial.start()
        self.addCleanup(self.serial.stop)
        FakeSerial.instances = []
        FakeSerial.replies = []
        self.api = session.load_api()

    def test_rmdata_exact_identity_command_and_completion(self):
        FakeSerial.replies = [b'boot log\n@ok rmdata snake\n']
        with patch.object(sys, 'argv', ['push-app.py', 'rmdata', 'snake', '--port', 'fake']):
            self.api.main()
        self.assertEqual(FakeSerial.instances[0].writes, [b'@mosaico rmdata snake\n'])

    def test_real_removal_error_propagates(self):
        FakeSerial.replies = [b'@err rm factory: factory apps cannot be removed; use rmdata\n']
        with patch.object(sys, 'argv', ['push-app.py', 'rm', 'factory', '--port', 'fake']):
            with self.assertRaises(SystemExit):
                self.api.main()

    def test_rmdata_requires_identity_before_connecting(self):
        with patch.object(sys, 'argv', ['push-app.py', 'rmdata', '--port', 'fake']):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.api.main()
        self.assertFalse(FakeSerial.instances)

if __name__ == '__main__':
    unittest.main()
