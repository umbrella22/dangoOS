"""USB simulator and CLI loader shared by offline host tests."""
import importlib.util
from pathlib import Path
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/device/session.py"
spec = importlib.util.spec_from_file_location("device_session", SCRIPT)
session = importlib.util.module_from_spec(spec)
spec.loader.exec_module(session)


class FakeSerial:
    replies = []
    instances = []

    def __init__(self, *args, **kwargs):
        self.closed = False
        self.writes = []
        self.queue = list(self.replies)
        self.open_states = []
        self.input_resets = 0
        self.dtr = True
        self.instances.append(self)

    def open(self):
        self.open_states.append(self.dtr)
        self._reset_input_buffer()

    def _reset_input_buffer(self):
        self.input_resets += 1
        self.queue.clear()

    @property
    def in_waiting(self):
        return len(self.queue[0]) if self.queue and isinstance(self.queue[0], bytes) else 0

    def read(self, count):
        item = self.queue.pop(0) if self.queue else b""
        if isinstance(item, Exception):
            raise item
        if len(item) > count:
            self.queue.insert(0, item[count:])
        return item[:count]

    def write(self, data):
        self.writes.append(data)
        return len(data)

    def close(self):
        self.closed = True
