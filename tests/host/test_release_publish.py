"""Exercise draft recovery and publication ordering without writing to GitHub."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/release"))
SPEC = importlib.util.spec_from_file_location("firmware_publish", ROOT / "scripts/release/publish.py")
API = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(API)


class PublishTests(unittest.TestCase):
    def scenario(self, *, exists, draft, corrupt_download=False):
        state = {"exists": exists, "draft": draft}
        calls = []
        manifest = {"tag": "v0.2.1", "source_commit": "1" * 40}

        def command(*args, check=True):
            calls.append(args)
            result = subprocess.CompletedProcess(args, 0, "", "")
            if args[0] == "api":
                self.assertEqual(args[1], "repos/example/firmware/git/ref/tags/v0.2.1")
                result.stdout = json.dumps({"object": {"type": "commit", "sha": "1" * 40}})
            elif args[:2] == ("release", "view"):
                if not state["exists"]:
                    self.assertFalse(check)
                    result.returncode = 1
                    result.stderr = "release not found\n"
                else:
                    result.stdout = json.dumps({"isDraft": state["draft"], "assets": [], "url": "https://example.invalid/release"})
            elif args[:2] == ("release", "create"):
                self.assertFalse(state["exists"])
                self.assertIn("--draft", args)
                state.update(exists=True, draft=True)
            elif args[:2] == ("release", "upload"):
                self.assertTrue(state["draft"])
            elif args[:2] == ("release", "download"):
                destination = Path(args[args.index("--dir") + 1])
                (destination / "SHA256SUMS").write_bytes(b"corrupt\n" if corrupt_download else b"checksums\n")
            elif args[:2] == ("release", "edit"):
                self.assertIn("--draft=false", args)
                state["draft"] = False
            else:
                self.fail(f"unexpected GitHub operation: {args}")
            return result

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            notes = root / "scripts/release/notes/v0.2.1.md"
            notes.parent.mkdir(parents=True)
            notes.write_text("Release notes\n")
            bundle = root / "bundle"
            bundle.mkdir()
            (bundle / "SHA256SUMS").write_bytes(b"checksums\n")
            with patch.object(API, "ROOT", root), patch.object(API, "verify", return_value=manifest), \
                    patch.object(API, "gh", side_effect=command), contextlib.redirect_stdout(io.StringIO()):
                if corrupt_download:
                    with self.assertRaisesRegex(ValueError, "downloaded release differs"):
                        API.publish(bundle, "example/firmware")
                else:
                    API.publish(bundle, "example/firmware")
        return state, [args[:2] for args in calls]

    def test_first_publication_and_existing_draft_recovery(self):
        for exists in (False, True):
            with self.subTest(existing_draft=exists):
                state, calls = self.scenario(exists=exists, draft=True)
                self.assertFalse(state["draft"])
                self.assertEqual(calls.count(("release", "create")), 0 if exists else 1)
                self.assertLess(calls.index(("release", "upload")), calls.index(("release", "download")))
                self.assertLess(calls.index(("release", "download")), calls.index(("release", "edit")))

    def test_download_mismatch_leaves_release_draft(self):
        state, calls = self.scenario(exists=True, draft=True, corrupt_download=True)
        self.assertTrue(state["draft"])
        self.assertNotIn(("release", "edit"), calls)

    def test_public_release_is_verified_without_writes(self):
        _, calls = self.scenario(exists=True, draft=False)
        self.assertIn(("release", "download"), calls)
        for action in ("create", "upload", "edit"):
            self.assertNotIn(("release", action), calls)


if __name__ == "__main__":
    unittest.main()
