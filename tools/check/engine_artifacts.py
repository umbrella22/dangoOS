#!/usr/bin/env python3
"""Verify precompiled input receipts, engine selection and firmware symbols."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def check_artifacts(root: Path, nm: str) -> dict:
    from compiled_apps import check
    receipt = check()
    for selection in ("both", "neither"):
        assert "os requires exactly one script engine" in (root / f"{selection}.log").read_text(), selection

    evidence = {"engine_id": receipt["engine_id"], "host_abi": receipt["host_abi"], "artifact_source": "precompiled-import", "device_execution": "not_run", "profiles": {}}
    for engine, forbidden in (("ts", r"(?:lua_|luaL_|luaopen_)"), ("lua", r"(?:JS_|embedts_)")):
        elf = root / f"{engine}.elf"
        symbols = subprocess.check_output([nm, "--defined-only", str(elf)], text=True)
        (root / f"{engine}-symbols.txt").write_text(symbols)
        names = {line.split()[-1] for line in symbols.splitlines() if line.split()}
        leaked = sorted(name for name in names if re.match(forbidden, name))
        assert not leaked, (engine, leaked)
        expected = "embedts_load" if engine == "ts" else "lua_newstate"
        assert expected in names, (engine, "missing selected runtime", expected)
        if engine == "ts":
            # JS_EvalInternal is also the runtime's 'eval unsupported' dispatch
            # stub. The parser implementation is __JS_EvalInternal (two '_').
            assert not any(re.match(r"(?:embedts_compile|__JS_EvalInternal|js_parse_program|js_parse_function_decl)", name) for name in names), "source compiler in TS firmware"
        tree = (root / f"{engine}-dependencies.txt").read_text()
        selected_name = "embedts-engine" if engine == "ts" else "lua-engine"
        selected = re.search(rf"^{selected_name} v[^\n]*", tree, re.MULTILINE)
        other = "lua-engine" if engine == "ts" else "embedts-engine"
        assert selected and not re.search(rf"^{other} v", tree, re.MULTILINE), tree
        assert "embedts-builder" not in tree and "tsrs_" not in tree
        if engine == "ts":
            assert "compiler" not in selected.group(), selected.group()
        evidence["profiles"][engine] = {"elf": str(elf), "sha256": hashlib.sha256(elf.read_bytes()).hexdigest(), "elf_bytes": elf.stat().st_size, "selected_vm_present": True, "other_vm_absent": True}
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--nm", required=True)
    args = parser.parse_args()
    evidence = check_artifacts(args.out, args.nm)
    (args.out / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
