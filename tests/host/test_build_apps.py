"""Exercise Lua application export with the maintained source tree."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from support import ROOT


class BuildAppsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def build(self, output, *options):
        subprocess.run(
            [sys.executable, str(ROOT / "scripts/build_apps.py"), str(output), *map(str, options)],
            check=True, capture_output=True, text=True,
        )
        return json.loads((output / "build-report.json").read_text())

    def test_factory_export_preserves_categories_ids_and_source_bytes(self):
        manifests = sorted((ROOT / "apps").glob("*/*/launcher.json"))
        self.assertEqual(len(manifests), 5)
        variants = self.root / "variants"
        for manifest in manifests:
            relative = manifest.parent.relative_to(ROOT / "apps")
            shutil.copytree(manifest.parent, variants / relative / "lua")
        for label, source in (("flat", ROOT / "apps"), ("variants", variants)):
            with self.subTest(layout=label):
                output = self.root / f"export-{label}"
                report = self.build(output, "--source", source)
                self.assertEqual(report["engine"], "lua")
                self.assertEqual(len(report["applications"]), 5)
                for manifest in manifests:
                    metadata = json.loads(manifest.read_text())
                    destination = output / manifest.parent.relative_to(ROOT / "apps")
                    self.assertEqual(json.loads((destination / "launcher.json").read_text()), metadata)
                    self.assertEqual(
                        (destination / metadata["entry"]).read_bytes(),
                        (manifest.parent / metadata["entry"]).read_bytes(),
                    )
                    self.assertTrue((destination / "LICENSE").is_file())

    def test_single_application_uses_its_launcher_id(self):
        source = ROOT / "apps/games/snake"
        output = self.root / "single"
        report = self.build(output, "--app", source)
        self.assertEqual([app["id"] for app in report["applications"]], ["snake"])
        self.assertEqual(
            (output / "snake/scripts/main.lua").read_bytes(),
            (source / "scripts/main.lua").read_bytes(),
        )


if __name__ == "__main__":
    unittest.main()
