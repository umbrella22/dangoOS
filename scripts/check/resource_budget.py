#!/usr/bin/env python3
"""Check a generated main image and linked SRAM against the firmware budget."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/device"))
from main_image import MAIN_BYTES, validate_image

POLICY = Path(__file__).with_name("resource-budget.json")


def elf_usage(data):
    """Count allocated sections, including NOBITS and static heap pools."""
    if data[:7] != b"\x7fELF\x01\x01\x01" or len(data) < 52:
        raise ValueError("expected a little-endian ELF32 firmware")
    header = struct.unpack_from("<16sHHIIIIIHHHHHH", data)
    if header[2] != 243:
        raise ValueError("expected a RISC-V ELF")
    offset, stride, count, strings_index = header[6], header[11], header[12], header[13]
    if stride != 40 or not count or strings_index >= count or offset + stride * count > len(data):
        raise ValueError("invalid ELF section table")
    sections = [struct.unpack_from("<10I", data, offset + stride * i) for i in range(count)]
    names = sections[strings_index]
    strings = data[names[4]:names[4] + names[5]]
    totals = {"internal_allocated_sections_bytes": 0, "rtc_allocated_sections_bytes": 0,
              "main_stack_bytes": 0}
    details = []
    for name_at, kind, flags, address, file_at, size, *_ in sections:
        if not flags & 2 or not size:
            continue
        if kind != 8 and file_at + size > len(data):
            raise ValueError("truncated allocated ELF section")
        if 0x2F000000 <= address < 0x2F080000:
            region, limit = "internal", 0x2F080000
        elif 0x2E000000 <= address < 0x2E008000:
            region, limit = "rtc", 0x2E008000
        else:
            continue
        if address + size > limit:
            raise ValueError("allocated ELF section crosses its memory region")
        name = strings[name_at:].split(b"\0", 1)[0].decode("ascii")
        if name == ".stack":
            totals["main_stack_bytes"] += size
        else:
            totals[f"{region}_allocated_sections_bytes"] += size
        details.append({"name": name, "region": region, "address": address, "bytes": size})
    return {**totals, "sections": details}


def assess(elf, image, policy=None):
    if policy is None:
        policy = json.loads(POLICY.read_text())
    if policy["application_slot_bytes"] != MAIN_BYTES:
        raise ValueError("resource budget slot differs from the S31ONE03 layout")
    info = validate_image(image)
    usage = elf_usage(elf)
    baseline = policy["baseline"]
    current = {"image_bytes": len(image), **{k: v for k, v in usage.items() if k != "sections"}}
    growth = {k: v - baseline[k] for k, v in current.items()}
    remaining = policy["application_slot_bytes"] - len(image)
    failures = []
    if remaining < policy["minimum_slot_headroom_bytes"]:
        failures.append("application slot headroom is below the resource budget")
    if current["main_stack_bytes"] < policy["minimum_main_stack_bytes"]:
        failures.append("linked main stack is below the resource budget")
    for metric, allowed in policy["maximum_growth_bytes"].items():
        if growth[metric] > allowed:
            failures.append(f"{metric} growth {growth[metric]} exceeds budget {allowed}")
    return {
        "schema": 1, "status": "pass" if not failures else "fail", "failures": failures,
        "elf_sha256": hashlib.sha256(elf).hexdigest(), "image_sha256": info["sha256"],
        "current": current, "growth": growth, "slot_remaining_bytes": remaining,
        "baseline_commit": policy["baseline_commit"], "sections": usage["sections"],
        "runtime_measurements": None,
        "internal_accounting": "allocated ELF sections exclude main stack and include static heap pools; this is not live heap usage",
    }


def save_image(elf, destination):
    subprocess.run(["espflash", "save-image", "--chip", "esp32s31", "--flash-size", "16mb",
                    "--flash-mode", "dio", "--flash-freq", "40mhz", str(elf), str(destination)],
                   check=True, stdout=subprocess.PIPE)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--elf", type=Path,
                        default=ROOT / "target/riscv32imafc-unknown-none-elf/release/os-fw")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        with tempfile.TemporaryDirectory(prefix="dango-resource-budget-") as temporary:
            image = Path(temporary) / "firmware.bin"
            save_image(args.elf, image)
            report = assess(args.elf.read_bytes(), image.read_bytes())
        encoded = json.dumps(report, indent=2) + "\n"
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded)
        print(encoded, end="")
        return 0 if report["status"] == "pass" else 1
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"resource budget rejected: {error}\n")


if __name__ == "__main__":
    sys.exit(main())
