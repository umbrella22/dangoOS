import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "display_probe", Path(__file__).resolve().parents[2] / "scripts/device/display_probe.py"
)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class DisplayProbeTests(unittest.TestCase):
    def test_busy_share_excludes_current_upload_and_reports_missing_records(self):
        text = "\n".join([
            "[INFO] display.busy begin=100 end=200",
            "[INFO] display.busy begin=260 end=340",
            "[INFO] display.frame id=1 kind=screen rgb888=true ok=true start=260 end=340 te=10",
            "[INFO] display.cost id=1 bytes=100 direct=0 windows=1 pack=5 transfer=70 pixel=300 offset=0",
            "[INFO] display.input id=1 first=150 last=220 read=250 count=3",
            "[INFO] display.frame id=2 kind=screen rgb888=true ok=true start=400 end=500 te=0",
            "[INFO] display.cost id=3 bytes=100 pack=5 trans",
        ])
        with tempfile.TemporaryDirectory() as directory:
            log, output = Path(directory)/"run.log", Path(directory)/"run.json"
            log.write_text(text)
            probe.analyze(log,output)
            result = json.loads(output.read_text())
        screen = result["scenarios"]["screen"]
        self.assertEqual(screen["busy_wait_us"]["p50"],50)
        self.assertEqual(screen["input_oldest_us"]["p50"],150)
        self.assertEqual(screen["input_latest_us"]["p50"],80)
        self.assertEqual(screen["busy_latency_share"]["p50"],.333)
        self.assertEqual(result["unpaired_frames"],[2])
        self.assertEqual(len(result["incomplete_lines"]),1)


if __name__ == "__main__":
    unittest.main()
