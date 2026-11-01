"""Validate standalone application and UI resource inventories before device IO."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re


def require(ok, message):
    if not ok:
        raise ValueError(message)


def inventory(root):
    result = {}
    if not root.exists():
        return result
    for path in sorted(root.rglob("*")):
        require(not path.is_symlink(), "bundle contains a symlink")
        relative = path.relative_to(root).as_posix()
        parts = PurePosixPath(relative).parts
        require(len(parts) <= 6 and all(re.fullmatch(r"[A-Za-z0-9_-][A-Za-z0-9._-]{0,47}", p) for p in parts),
                "invalid bundle resource path")
        if path.is_dir():
            result[relative] = {"kind": "directory"}
        else:
            require(path.is_file(), "unsupported bundle resource")
            data = path.read_bytes()
            require(len(data) <= 1024 * 1024, "bundle file exceeds device transfer limit")
            result[relative] = {"kind": "file", "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    return result


def application(root):
    root = Path(root)
    report = json.loads((root / "bundle.json").read_bytes())
    require(report.get("schema") == 1 and report.get("kind") == "dango-lua-application",
            "unsupported application bundle")
    identity = report.get("id")
    require(isinstance(identity, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,63}", identity), "invalid bundle id")
    require(inventory(root / "payload") == report.get("files")
            and inventory(root / "documentation") == report.get("documentation"), "application bundle inventory mismatch")
    manifest = json.loads((root / "payload/launcher.json").read_bytes())
    require(manifest.get("schema_version") == 1 and manifest.get("id") == identity,
            "application identity mismatch")
    entry = manifest.get("entry")
    require(isinstance(entry, str) and entry.endswith(".lua")
            and report["files"].get(entry, {}).get("kind") == "file", "application entry missing")
    return report


def fonts(root):
    root = Path(root)
    report = json.loads((root / "bundle.json").read_bytes())
    identity = report.get("id")
    require(report.get("schema") == 1 and report.get("kind") == "dango-ui-fonts"
            and isinstance(identity, str) and re.fullmatch(r"[0-9a-f]{64}", identity)
            and report.get("sha256") == identity and report.get("file") == f"fonts-{identity}.bin",
            "unsupported UI font bundle")
    path = root / report["file"]
    require(not path.is_symlink(), "font bundle contains a symlink")
    data = path.read_bytes()
    require(24 <= len(data) == report.get("bytes") <= 1024 * 1024
            and hashlib.sha256(data).hexdigest() == identity
            and data[:8] == b"MUIF\x01\x00\x04\x00", "UI font bundle hash/size/header mismatch")
    require(inventory(root / "documentation") == report.get("documentation", {}), "font documentation inventory mismatch")
    return report
