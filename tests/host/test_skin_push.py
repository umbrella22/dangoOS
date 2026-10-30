"""Exercise production skin upload API against a fake serial transport."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
import zlib
from support import FakeSerial, session


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

    def test_large_skin_and_short_writes_preserve_exact_protocol_stream(self):
        class BackpressuredSerial(self.api.ConsoleSerial):
            def write(self, data):
                if len(data) > 4096:
                    raise OSError('whole-file write exceeded transport timeout')
                accepted = data[:317]
                self.writes.append(accepted)
                return len(accepted)

        self.api.ConsoleSerial = BackpressuredSerial
        FakeSerial.replies = [b'@ok\n'] * 8
        data = bytes(range(256)) * 3089 + b'\0@mosaico reboot\n\xff'
        (self.root / 'fox.bin').write_bytes(data)
        with patch.object(sys, 'argv', ['console.py', 'skin-push', str(self.root), '--select', '--port', 'fake']):
            with contextlib.redirect_stdout(io.StringIO()):
                self.api.main()
        expected = b'@mosaico hello\n@mosaico skin-begin test_skin\n'
        for name in ('fox.bin', 'skin.json'):
            payload = (self.root / name).read_bytes()
            expected += f'@mosaico file {name} {len(payload)} {zlib.crc32(payload):08x}\n'.encode() + payload
        expected += b'@mosaico skin-commit test_skin\n@mosaico skin-select test_skin\n'
        device = FakeSerial.instances[-1]
        self.assertEqual(b''.join(device.writes), expected)
        self.assertTrue(device.closed)

    def test_payload_disconnect_closes_port_without_sending_text_into_binary_body(self):
        class DisconnectingSerial(self.api.ConsoleSerial):
            def write(self, data):
                if data.startswith(b'test binary'):
                    self.writes.append(data[:2])
                    self.in_body = True
                    return 2
                if getattr(self, 'in_body', False):
                    raise OSError('disconnected mid-body')
                return super().write(data)

        self.api.ConsoleSerial = DisconnectingSerial
        FakeSerial.replies = [b'@ok\n'] * 3
        with patch.object(sys, 'argv', ['console.py', 'skin-push', str(self.root), '--select', '--port', 'fake']):
            with self.assertRaises(OSError):
                self.api.main()
        device = FakeSerial.instances[-1]
        self.assertTrue(device.closed)
        self.assertEqual(device.writes[-1], b'te')
        self.assertFalse(any(b'skin-commit' in part or b'skin-select' in part or part == b'@mosaico abort\n' for part in device.writes))


if __name__ == '__main__':
    unittest.main()
