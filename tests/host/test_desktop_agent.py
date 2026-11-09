"""Desktop skills, cancellation and Responses framing without device or cloud."""
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from agent import model
from agent.bridge import Cancelled, run, strict_json, utf8_chunks
from agent.skills import Store, action


class AgentTests(unittest.TestCase):
    def spec(self, identity="quiet"):
        return {"id": identity, "description": "安静模式", "steps": [
            {"tool": "set_volume", "percent": 35}, {"tool": "set_brightness", "percent": 45},
            {"tool": "device_status", "percent": None}]}

    def test_install_is_atomic_and_rejects_path_escape_unknown_api_and_ungranted_steps(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(root, 7)
            store.install(self.spec())
            before = store.path("quiet").read_bytes()
            for spec in (self.spec("../escape"), {**self.spec(), "steps": [{"tool": "shell", "percent": None}]},
                         {**self.spec(), "steps": [{"tool": "set_brightness", "percent": 9}]}):
                with self.assertRaises(ValueError):
                    store.install(spec)
                self.assertEqual(store.path("quiet").read_bytes(), before)
            with self.assertRaises(ValueError):
                Store(root, 1).load("quiet")
            self.assertEqual([x["id"] for x in store.list()], ["quiet"])
            package = json.loads(before)
            package["manifest"]["steps"][0]["percent"] = 99
            store.path("quiet").write_text(json.dumps(package))
            with self.assertRaises(ValueError):
                store.load("quiet")

    def test_cancellation_between_steps_stops_actual_executor_and_installed_skill_is_reused(self):
        with tempfile.TemporaryDirectory() as root:
            store = Store(root, 7)
            store.install(self.spec())
            calls = []
            def check():
                if len(calls) == 1:
                    raise Cancelled()
            def execute(name, args):
                calls.append((name, args))
                return {"ok": True}
            with self.assertRaises(Cancelled):
                store.run("quiet", execute, check)
            self.assertEqual(len(calls), 1)
            calls.clear()
            result = store.run("quiet", execute, lambda: None)
            self.assertTrue(result["ok"])
            self.assertEqual([c[0] for c in calls], ["set_volume", "set_brightness", "device_status"])
            self.assertEqual(store.load("quiet")["id"], "quiet")

    def test_percent_accepts_json_integer_values_and_rejects_coercion_and_duplicate_fields(self):
        for value in (10, 10.0, 1e2):
            self.assertEqual(action("set_volume", {"percent": value})[1], {"percent": int(value)})
        for value in (True, "10", 10.5, -1, 101, float("nan")):
            with self.assertRaises(ValueError):
                action("set_volume", {"percent": value})
        with self.assertRaises(ValueError):
            strict_json('{"percent":30,"percent":40}')
        with self.assertRaises(ValueError):
            strict_json('{"percent":NaN}')

    def test_unicode_chunks_and_large_sse_events_have_no_reply_length_cap(self):
        text = "🦊你好\n" * 50000
        chunks = list(utf8_chunks(text))
        self.assertEqual(b"".join(chunks).decode(), text)
        self.assertTrue(all(len(c) <= 100 and c.decode() for c in chunks))
        payload = {"type": "response.completed", "response": {"status": "completed", "output": [{"type": "reasoning", "encrypted_content": "x" * 100000}]}}
        wire = ("data: " + json.dumps(payload) + "\r\n\r\ndata: [DONE]\n\n").encode()
        self.assertEqual(list(model.events(io.BytesIO(wire))), [payload])
        with self.assertRaises(ValueError):
            list(model.events(io.BytesIO(b'data: {}\n')))

    def test_repeated_provider_call_id_cannot_repeat_a_device_action(self):
        class Wire:
            grants = 7
        class Task:
            wire = Wire()
            receipt = {"rounds": 0}
            def __init__(self):
                self.calls = []
            def check(self):
                pass
            def emit(self, text):
                pass
            def execute(self, name, args):
                self.calls.append((name,args))
                return {"ok": True}
        class Backend:
            index = 0
            def round(self, context, tools, emit, check):
                self.index += 1
                if self.index <= 2:
                    return [{"type":"function_call","status":"completed","call_id":"same","name":"set_volume","arguments":'{"percent":35}'}]
                return [{"type":"message","content":[{"type":"output_text","text":"done"}]}]
        task = Task()
        with tempfile.TemporaryDirectory() as root:
            run(task, Backend(), Store(root,7), "quiet")
        self.assertEqual(task.calls, [("set_volume", {"percent":35})])
        self.assertEqual(task.receipt["rounds"], 3)


if __name__ == "__main__":
    unittest.main()
