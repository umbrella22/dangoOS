#!/usr/bin/env python3
"""Verify imported EmbedTS artifact receipts and the OS source/build boundary."""
import hashlib
import json
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[2]

def check():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    engine = workspace["workspace"]["dependencies"]["ts-engine"]
    assert engine["package"] == "embedts-engine" and "path" not in engine
    assert engine["git"] == "https://github.com/umbrella22/EmbedTypescript.git"
    revision = engine["rev"]
    assert re.fullmatch(r"[0-9a-f]{40}", revision), "engine must use a full Git commit"
    assert engine["default-features"] is False
    for consumer in ("crates/os", "tests/benchmarks/script-engines/runner"):
        dependency = tomllib.loads((ROOT / consumer / "Cargo.toml").read_text())["dependencies"]["ts-engine"]
        assert dependency == {"workspace": True, "optional": True}, consumer
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    packages = [p for p in lock["package"] if p["name"] == "embedts-engine"]
    assert len(packages) == 1
    assert packages[0]["source"] == f"git+{engine['git']}?rev={revision}#{revision}"
    directory = ROOT / "artifacts/embedts"
    manifest = json.loads((directory / "manifest.json").read_text())
    assert manifest["format"] == "embedts-artifacts-v1"
    listed = {"manifest.json"}
    for item in manifest["artifacts"]:
        path = directory / item["path"]
        assert path.resolve().is_relative_to(directory.resolve()), item["path"]
        data = path.read_bytes()
        assert len(data) == item["bytes"] and hashlib.sha256(data).hexdigest() == item["sha256"], item["path"]
        package = bytes.fromhex(data.decode()[5:]) if item["path"].endswith(".hex") and data.startswith(b"etb1:") else data
        assert len(package) > 84 and package[:8] == b"EMBEDTS\0" and package[8:12] == bytes([1,0,0,0]), item["path"]
        assert int.from_bytes(package[12:16], "little") == len(package)-84
        assert package[16:48].hex() == manifest["engine_id"]
        assert int.from_bytes(package[48:52], "little") == manifest["host_abi"]
        assert hashlib.sha256(package[84:]).digest() == package[52:84]
        assert item["path"] not in listed
        listed.add(item["path"])
    actual = {p.relative_to(directory).as_posix() for p in directory.rglob("*") if p.is_file()}
    assert actual == listed, (actual - listed, listed - actual)
    sources = [ROOT / p for p in ("crates", "apps", "tools", "tests", "scripts", "examples")]
    assert not [p for folder in sources for pattern in ("*.ts", "*.tsx", "*.mts", "*.cts") for p in folder.rglob(pattern)], "TypeScript source in OS tree"
    cargo = (ROOT / "crates/os/Cargo.toml").read_text()
    assert "ts-compiler" not in cargo and "embedts-builder" not in cargo and 'features = ["compiler"]' not in cargo
    build = (ROOT / "crates/os/build.rs").read_text()
    assert "compile_module" not in build and "ts_compiler" not in build
    print(f"EmbedTS inputs: {len(listed)-1} verified, engine={manifest['engine_id']}, ABI={manifest['host_abi']}")
    return manifest

if __name__ == "__main__":
    check()
