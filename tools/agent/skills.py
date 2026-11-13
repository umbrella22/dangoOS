"""Validated, atomic, zero-argument skills over explicit device capabilities."""
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile

CAPABILITIES = {"device_status": 1, "set_volume": 2, "set_brightness": 4}
ID = re.compile(r"[a-z0-9][a-z0-9_-]{0,62}\Z")


def action(name, arguments):
    if name not in CAPABILITIES or not isinstance(arguments, dict):
        raise ValueError("unknown device tool or invalid arguments")
    if name == "device_status":
        if arguments:
            raise ValueError("status takes no arguments")
    else:
        if set(arguments) != {"percent"}:
            raise ValueError("percent is required and no other argument is accepted")
        value = arguments["percent"]
        low = 10 if name == "set_brightness" else 0
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= 100 or int(value) != value:
            raise ValueError("invalid device percent")
        arguments = {"percent": int(value)}
    return name, arguments


def validate(spec, grants, check_cancel=lambda: None):
    if not isinstance(spec, dict) or set(spec) != {"id", "description", "steps"}:
        raise ValueError("invalid skill fields")
    if not isinstance(spec["id"], str) or not ID.fullmatch(spec["id"]):
        raise ValueError("invalid skill id")
    if not isinstance(spec["description"], str) or not spec["description"].strip():
        raise ValueError("skill needs a description")
    if not isinstance(spec["steps"], list) or not spec["steps"]:
        raise ValueError("skill needs at least one step")
    capabilities = 0
    steps = []
    simulated = {"volume_percent": 50, "brightness_percent": 50}
    proof = []
    for step in spec["steps"]:
        check_cancel()
        if not isinstance(step, dict) or set(step) != {"tool", "percent"}:
            raise ValueError("invalid skill step")
        if step["tool"] == "device_status" and step["percent"] is None:
            name, args = action(step["tool"], {})
        else:
            name, args = action(step["tool"], {"percent": step["percent"]})
        capabilities |= CAPABILITIES[name]
        if capabilities & ~grants:
            raise ValueError("skill requires an ungranted capability")
        steps.append({"tool": name, "percent": args.get("percent")})
        if name == "set_volume":
            simulated["volume_percent"] = args["percent"]
        if name == "set_brightness":
            simulated["brightness_percent"] = args["percent"]
        proof.append(dict(simulated))
    manifest = {"schema": 1, "id": spec["id"], "description": spec["description"],
                "input_schema": {"type": "object", "properties": {}, "additionalProperties": False},
                "capabilities": capabilities, "steps": steps}
    canonical = json.dumps(manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return {"manifest": manifest, "sha256": hashlib.sha256(canonical).hexdigest(),
            "validation": {"syntax": True, "parameters": True, "capabilities": True,
                           "simulation": proof, "physical_execution": "not-run"}}


class Store:
    def __init__(self, root, grants):
        self.root = Path(root).expanduser().resolve()
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.grants = grants

    def path(self, identity):
        if not isinstance(identity, str) or not ID.fullmatch(identity):
            raise ValueError("invalid skill id")
        return self.root / (identity + ".json")

    def install(self, spec, check_cancel=lambda: None):
        package = validate(spec, self.grants, check_cancel)
        destination = self.path(spec["id"])
        fd, temporary = tempfile.mkstemp(prefix=".candidate-", dir=self.root)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as stream:
                json.dump(package, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            check_cancel()
            os.replace(temporary, destination)
            directory = os.open(self.root, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        finally:
            Path(temporary).unlink(missing_ok=True)
        return {"ok": True, "id": spec["id"], "sha256": package["sha256"], "validated": True, "installed": True}

    def load(self, identity):
        path = self.path(identity)
        if path.is_symlink():
            raise ValueError("skill links are not executable")
        package = json.loads(path.read_text())
        manifest = package["manifest"]
        checked = validate({key: manifest[key] for key in ("id", "description", "steps")}, self.grants)
        if checked != package or manifest["id"] != identity:
            raise ValueError("skill package failed validation")
        return manifest

    def list(self):
        entries = []
        for path in sorted(self.root.glob("*.json")):
            manifest = self.load(path.stem)
            entries.append({key: manifest[key] for key in ("id", "description", "capabilities")})
        return entries

    def run(self, identity, execute, check_cancel):
        manifest = self.load(identity)
        outputs = []
        for step in manifest["steps"]:
            check_cancel()
            name, args = action(step["tool"], {} if step["percent"] is None else {"percent": step["percent"]})
            result = execute(name, args)
            outputs.append({"tool": name, "result": result})
            if not isinstance(result, dict) or result.get("ok") is not True:
                return {"ok": False, "id": identity, "steps": outputs, "error": "device step failed"}
        check_cancel()
        return {"ok": True, "id": identity, "steps": outputs}
