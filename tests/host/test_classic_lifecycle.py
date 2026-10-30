"""Exercise the production Classic adapter against asynchronous profile callbacks."""
import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
VENDOR = ROOT / "vendor/esp-bluedroid"


class ClassicLifecycleTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("cc"), "native C compiler required")
    def test_profile_acknowledgements_restart_and_volume(self):
        with tempfile.TemporaryDirectory(prefix="mosaico-classic-") as tmp:
            binary = pathlib.Path(tmp) / "lifecycle"
            command = ["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror",
                       "-I", str(VENDOR / "src"), "-I", str(VENDOR / "shim"),
                       "-I", str(VENDOR / "upstream/host/bluedroid/api/include/api"),
                       str(ROOT / "tests/host/fixtures/classic_lifecycle.c"),
                       "-o", str(binary)]
            build = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("lifecycle and volume passed", result.stdout)

    def test_imported_sources_have_recorded_provenance(self):
        upstream = json.loads((VENDOR / "UPSTREAM.json").read_text())
        patches = json.loads((VENDOR / "PATCHES.json").read_text())
        self.assertEqual(upstream["commit"], patches["upstream_commit"])
        original = {item["path"]: item["sha256"] for item in upstream["files"]}
        changed = {item["path"]: item for item in patches["files"]}
        self.assertEqual(len(original), len(upstream["files"]))
        self.assertEqual(len(changed), len(patches["files"]))
        self.assertTrue(changed.keys() <= original.keys())
        self.assertTrue((VENDOR / patches["patch"]).is_file())
        actual = {str(p.relative_to(VENDOR / "upstream"))
                  for p in (VENDOR / "upstream").rglob("*") if p.is_file()}
        self.assertEqual(actual, original.keys())
        for name, digest in original.items():
            with self.subTest(source=name):
                if name in changed:
                    self.assertEqual(digest, changed[name]["upstream_sha256"])
                    self.assertTrue(changed[name]["reason"])
                    digest = changed[name]["local_sha256"]
                self.assertEqual(hashlib.sha256((VENDOR / "upstream" / name).read_bytes()).hexdigest(), digest)
