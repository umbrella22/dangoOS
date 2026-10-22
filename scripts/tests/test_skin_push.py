"""Exercise production skin upload API against a fake serial transport."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from test_n1_device_session import FakeSerial, session


class SkinPushTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'skin.json').write_text(json.dumps({'id': 'test_skin'}))
        (self.root / 'fox.bin').write_bytes(b'test binary; firmware validates format')
        FakeSerial.instances = []
        FakeSerial.replies = []
        previous = sys.modules.get('serial')
        sys.modules['serial'] = types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError)
        def restore():
            if previous is None:
                sys.modules.pop('serial', None)
            else:
                sys.modules['serial'] = previous
        self.addCleanup(restore)
        self.api = session.load_api()
        self.dev = self.api.Device('fake', False)

    def test_upload_commit_then_select_and_no_app_commands(self):
        self.dev.ser.queue = [b'@ok\n'] * 8
        with contextlib.redirect_stdout(io.StringIO()):
            self.api.push_skin(self.dev, str(self.root), True)
        commands = [b for b in self.dev.ser.writes if b.startswith(b'@mosaico')]
        self.assertEqual(commands[1], b'@mosaico skin-begin test_skin\n')
        self.assertEqual(commands[-2:], [b'@mosaico skin-commit test_skin\n', b'@mosaico skin-select test_skin\n'])
        self.assertFalse(any(b'@mosaico begin ' in b for b in commands))

    def test_bad_payload_aborts_and_does_not_commit_or_select(self):
        self.dev.ser.queue = [b'@ok\n'] * 3 + [b'@err crc\n', b'@ok abort\n']
        with self.assertRaises(SystemExit):
            self.api.push_skin(self.dev, str(self.root), True)
        self.assertEqual(self.dev.ser.writes[-1], b'@mosaico abort\n')
        self.assertFalse(any(b'skin-commit' in b or b'skin-select' in b for b in self.dev.ser.writes))

    def test_invalid_identity_or_unexpected_file_rejected_before_write(self):
        for skin_id in ('../x', 'test\n', 'white space', 'default', 42):
            (self.root / 'skin.json').write_text(json.dumps({'id': skin_id}))
            with self.assertRaises(SystemExit):
                self.api.push_skin(self.dev, str(self.root), False)
            self.assertFalse(self.dev.ser.writes)
        (self.root / 'skin.json').write_text(json.dumps({'id': 'valid'}))
        (self.root / 'extra').write_bytes(b'no')
        with self.assertRaises(SystemExit):
            self.api.push_skin(self.dev, str(self.root), False)
        self.assertFalse(self.dev.ser.writes)


if __name__ == '__main__':
    unittest.main()
