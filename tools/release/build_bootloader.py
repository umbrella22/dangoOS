#!/usr/bin/env python3
"""Build the pinned single-main bootloader without changing an IDF checkout."""
import argparse
import hashlib
import json
import os
import re
import struct
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def checked_image(data):
    if not (24 <= len(data) <= 0x6000 and data[0] == 0xe9
            and struct.unpack_from("<H", data, 12)[0] == 32):
        raise ValueError("bootloader does not fit the S31 reserved region")
    # This clock also serves runtime instruction fetch; 40 MHz slows the UI.
    if data[2:4] != bytes([2, 0x4f]):
        raise ValueError("S31 bootloader requires DIO, 80 MHz Flash and 16 MiB capacity")


def source_sha256():
    digest = hashlib.sha256()
    for path in sorted((ROOT / "firmware/boot/loader").rglob("*")):
        if path.is_file():
            digest.update(path.relative_to(ROOT / "firmware/boot/loader").as_posix().encode() + b"\0")
            digest.update(path.read_bytes())
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--idf", required=True, type=Path)
    parser.add_argument("--tools", required=True, type=Path)
    parser.add_argument("--build", type=Path, default=ROOT / "target/bootloader")
    args = parser.parse_args()
    idf, tools, build = args.idf.resolve(), args.tools.resolve(), args.build.resolve()
    actual = subprocess.check_output(["git", "-C", str(idf), "rev-parse", "HEAD"], text=True).strip()
    expected = (ROOT / "firmware/boot/loader/idf-revision.txt").read_text().strip()
    if actual != expected:
        raise SystemExit(f"IDF revision mismatch: expected {expected}, got {actual}")
    python = list(tools.glob("python_env/idf*_env/bin/python"))
    if len(python) != 1:
        raise SystemExit("one IDF Python environment is required under --tools")
    tool_bins = sorted({p.parent for pattern in ("tools/**/bin/riscv32-esp-elf-gcc", "tools/**/bin/cmake", "tools/**/ninja") for p in tools.glob(pattern) if p.is_file()})
    env = os.environ.copy()
    version_source = (idf / "tools/cmake/version.cmake").read_text()
    version = ".".join(re.search(rf"set\(IDF_VERSION_{part}\s+(\d+)\)", version_source)[1]
                       for part in ("MAJOR", "MINOR", "PATCH"))
    env.update(IDF_PATH=str(idf), IDF_TOOLS_PATH=str(tools), IDF_PYTHON_ENV_PATH=str(python[0].parent.parent),
               ESP_IDF_VERSION=version,
               PATH=os.pathsep.join(map(str, [python[0].parent, *tool_bins])) + os.pathsep + env.get("PATH", ""))
    build.mkdir(parents=True, exist_ok=True)
    # sdkconfig.defaults does not override a previous generated sdkconfig.
    # Reused build directories must use the current repository configuration.
    (build / "sdkconfig").unlink(missing_ok=True)
    subprocess.run([str(python[0]), str(idf / "tools/idf.py"), "-C", str(ROOT / "firmware/boot/loader"), "-B", str(build),
                    "-D", "IDF_TARGET=esp32s31", "-D", f"SDKCONFIG={build / 'sdkconfig'}",
                    "-D", f"SDKCONFIG_DEFAULTS={ROOT / 'firmware/boot/loader/sdkconfig.defaults'}", "bootloader"], env=env, check=True)
    binary = build / "bootloader/bootloader.bin"
    data = binary.read_bytes()
    checked_image(data)
    receipt = {"idf_revision": actual, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "offset": 0x2000,
               "layout": "S31ONE03", "source_sha256": source_sha256(), "source": "firmware/boot/loader/bootloader_components/main"}
    (build / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
