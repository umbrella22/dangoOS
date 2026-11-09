"""Voice credential provisioning through binary upload, without hardware."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
from support import FakeSerial, session


class VoiceConsoleTest(unittest.TestCase):
    def setUp(self):
        mock = patch.dict(sys.modules, serial=types.SimpleNamespace(Serial=FakeSerial, SerialException=OSError))
        mock.start()
        self.addCleanup(mock.stop)
        FakeSerial.instances = []
        FakeSerial.replies = []
        self.api = session.load_api()
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / "voice.json"

    def cli(self, *args):
        with patch.object(sys, "argv", ["console.py", *args, "--port", "fake"]), contextlib.redirect_stdout(io.StringIO()) as out:
            self.api.main()
        return out.getvalue()

    def test_keys_are_binary_payload_and_never_printed_or_placed_in_commands(self):
        config = {"schema": 1, "asr": {"url": "wss://asr.example/v1/realtime?intent=transcription", "api_key": "private-asr", "model": "transcribe"},
                  "chat": {"url": "https://chat.example/v1/responses", "api_key": "private-chat", "model": "chat"}}
        data = json.dumps(config).encode()
        self.path.write_bytes(data)
        FakeSerial.replies = [b"@ok voice-config-begin\n", b"@ok ready config.json\n", b"@ok file config.json\n",
                              b"@ok voice-config-commit next-turn=1\n", b"@ok voice-reload configured=1\n"]
        output = self.cli("voice-config-push", str(self.path))
        device = FakeSerial.instances[0]
        self.assertTrue(device.closed)
        self.assertIn(data, device.writes)
        for value in ("private-asr", "private-chat"):
            self.assertNotIn(value, output)
            self.assertFalse(any(value.encode() in line for line in device.writes if line.startswith(b"@mosaico ")))

    def test_invalid_secret_json_is_rejected_without_echoing_it_or_opening_serial(self):
        self.path.write_bytes(b'{"api_key":"private-invalid",broken}')
        with contextlib.redirect_stderr(io.StringIO()) as errors, self.assertRaises(SystemExit):
            self.cli("voice-config-push", str(self.path))
        self.assertNotIn("private-invalid", errors.getvalue())
        self.assertEqual(FakeSerial.instances, [])

    def test_cancel_and_status_do_not_require_a_config_argument(self):
        for command in ("voice-status", "voice-cancel"):
            FakeSerial.replies = [f"@ok {command}\n".encode()]
            self.cli(command)
            self.assertEqual(FakeSerial.instances[-1].writes, [f"@mosaico {command}\n".encode()])
            self.assertTrue(FakeSerial.instances[-1].closed)

    def test_text_preserves_whitespace_quotes_unicode_and_newlines(self):
        text = '  你好，团子🦊 "hello"\\\n第二行  '
        FakeSerial.replies = [b"@ok voice-ask accepted\n"]
        self.cli("voice-ask", text)
        wire = FakeSerial.instances[-1].writes[0]
        self.assertEqual(wire.count(b"\n"), 1)
        self.assertEqual(bytes.fromhex(wire.removeprefix(b"@mosaico voice-ask ").decode()), text.encode())

    def test_text_domain_and_console_limits_are_distinct_and_checked_before_open(self):
        for text in (" \n\t", "a\0b", "a" * 1025, "中" * 100):
            with self.subTest(text_bytes=len(text.encode())), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.cli("voice-ask", text)
        self.assertEqual(FakeSerial.instances, [])
        self.assertEqual(len((self.api.PREFIX + self.api.voice_ask_command("a" * 150)).encode()), 319)
        with self.assertRaisesRegex(ValueError, "console command capacity"):
            self.api.voice_ask_command("a" * 151)

    @staticmethod
    def chunk(session, offset, total, data):
        return f"@ok voice-read session={session} offset={offset} total={total} hex={data.hex()}\n".encode()

    def test_reply_reconstructs_bytes_before_decoding_split_chinese_and_emoji(self):
        text = "a" * 47 + "团子🦊\n" + "回复" * 2000
        raw = text.encode()
        FakeSerial.replies = [self.chunk(7, offset, len(raw), raw[offset:offset + 48]) for offset in range(0, len(raw), 48)]
        self.assertEqual(self.cli("voice-read"), text + "\n")
        self.assertEqual(FakeSerial.instances[-1].writes, [f"@mosaico voice-read {offset}\n".encode() for offset in range(0, len(raw), 48)])

    def test_reply_rejects_session_changes_invalid_offsets_no_progress_and_malformed_bytes(self):
        first = self.chunk(7, 0, 49, b"a" * 48)
        cases = [
            [first, self.chunk(8, 48, 49, b"b")],
            [first, self.chunk(7, 47, 49, b"b")],
            [first, self.chunk(7, 48, 50, b"b")],
            [self.chunk(7, 0, 1, b"")],
            [self.chunk(7, 0, 1, b"ab")],
            [b"@ok voice-read session=7 offset=0 total=1 hex=0\n"],
            [self.chunk(7, 0, 1, b"\xff")],
        ]
        for replies in cases:
            with self.subTest(replies=replies), self.assertRaises(SystemExit):
                FakeSerial.replies = replies
                self.cli("voice-read")
            self.assertTrue(FakeSerial.instances[-1].closed)
        FakeSerial.replies = [self.chunk(7, 0, 0, b"")]
        self.assertEqual(self.cli("voice-read"), "\n")

    def test_page_offset_is_sent_without_a_model_request_and_invalid_offsets_are_rejected(self):
        FakeSerial.replies=[b"@ok voice-page requested\n"]
        self.cli("voice-page","1024")
        self.assertEqual(FakeSerial.instances[-1].writes,[b"@mosaico voice-page 1024\n"])
        for bad in ("-1","x","4294967296"):
            with contextlib.redirect_stderr(io.StringIO()),self.assertRaises(SystemExit):
                self.cli("voice-page",bad)

    def test_status_prints_heap_and_error_lines_before_ack(self):
        FakeSerial.replies = [b"@voice-heap capability=internal free=123 largest=100 largest_valid=1\n",
                              b"@voice-error none\n", b"@ok voice-status busy=0\n"]
        output = self.cli("voice-status")
        self.assertIn("largest=100", output)
        self.assertIn("@voice-error none", output)


if __name__ == "__main__":
    unittest.main()
