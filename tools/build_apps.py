#!/usr/bin/env python3
"""Build the Lua application tree for app-pack system/bundles."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile


def build(source: Path, output: Path, selected: Path | None = None) -> None:
    source = source.resolve()
    output = output.absolute()
    if output.exists():
        raise ValueError(f"output already exists: {output}")
    manifests = (
        [selected.resolve() / "launcher.json"]
        if selected
        else sorted([*source.glob("*/*/launcher.json"), *source.glob("*/*/lua/launcher.json")])
    )
    if not manifests:
        raise ValueError("no application manifests found")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=f".{output.name}-", dir=output.parent))
    try:
        report = []
        seen = set()
        for manifest in manifests:
            metadata = json.loads(manifest.read_text())
            app = manifest.parent
            identity = metadata["id"]
            matches_directory = app.name == identity or (app.name == "lua" and app.parent.name == identity)
            if (not selected and not matches_directory) or identity in seen:
                raise ValueError(f"duplicate or mismatched application id: {app}")
            seen.add(identity)
            entry = Path(metadata["entry"])
            if entry.is_absolute() or ".." in entry.parts or entry.suffix != ".lua":
                raise ValueError(f"entry must name a relative .lua source: {app}")
            package_root = app.parent if app.name == "lua" and app.parent.name == identity else app
            destination = temporary / identity if selected else temporary / package_root.relative_to(source)
            destination.mkdir(parents=True)
            for item in app.rglob("*"):
                relative = item.relative_to(app)
                if any(part in ("node_modules", "dist") or part.startswith(".") for part in relative.parts):
                    continue
                if item.is_symlink():
                    raise ValueError(f"symbolic application resource: {item}")
                if item.is_dir() or item.name in ("launcher.json", "package.json", "tsconfig.json"):
                    continue
                if item.suffix in (".ts", ".etb"):
                    continue
                target = destination / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(item, target)
            (destination / "launcher.json").write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + "\n")
            entry_bytes = (destination / metadata["entry"]).read_bytes()
            report.append({"id":identity,"source":str(app) if selected else app.relative_to(source).as_posix(),"entry":metadata["entry"],"bytes":len(entry_bytes),"sha256":hashlib.sha256(entry_bytes).hexdigest()})
        (temporary / "build-report.json").write_text(json.dumps({"engine":"lua","applications":report}, indent=2) + "\n")
        os.rename(temporary, output)
    except BaseException:
        shutil.rmtree(temporary)
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1] / "apps/builtin")
    selection.add_argument("--app", type=Path, help="build one source app or language variant")
    options = parser.parse_args()
    build(options.source, options.output, options.app)
