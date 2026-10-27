#!/usr/bin/env python3
"""Install checksum-pinned Linux CI tools and export Cargo's target environment."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import tarfile
import urllib.request
import zipfile


def download(url, path, expected):
    if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        temporary = path.with_suffix(path.suffix + ".download")
        with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as out:
            while block := response.read(1024 * 1024):
                out.write(block)
        if hashlib.sha256(temporary.read_bytes()).hexdigest() != expected:
            temporary.unlink()
            raise ValueError(f"download checksum mismatch: {url}")
        temporary.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ("Linux", "x86_64"):
        parser.error("this bootstrap is for the Linux x86_64 CI runner")
    root = args.out.resolve()
    root.mkdir(parents=True, exist_ok=True)
    tools = json.loads(Path(__file__).with_name("inputs.json").read_text())["tools"]
    archive = root / "gcc.tar.xz"
    download(tools["gcc"]["url"], archive, tools["gcc"]["sha256"])
    stamp = root / "gcc.sha256"
    if not stamp.exists() or stamp.read_text() != tools["gcc"]["sha256"]:
        with tarfile.open(archive) as package:
            package.extractall(root, filter="data")
        stamp.write_text(tools["gcc"]["sha256"])
    archive = root / "espflash.zip"
    download(tools["espflash"]["url"], archive, tools["espflash"]["sha256"])
    binary = root / "bin" / "espflash"
    binary.parent.mkdir(exist_ok=True)
    with zipfile.ZipFile(archive) as package:
        binary.write_bytes(package.read("espflash"))
    binary.chmod(0o755)
    compiler = root / "riscv32-esp-elf" / "bin"
    environment = {
        "CC_riscv32imafc_unknown_none_elf": str(compiler / "riscv32-esp-elf-gcc"),
        "AR_riscv32imafc_unknown_none_elf": str(compiler / "riscv32-esp-elf-ar"),
        "NM": str(compiler / "riscv32-esp-elf-nm"),
        "LVGL_SYSROOT": str(root / "riscv32-esp-elf" / "riscv32-esp-elf"),
        "LIBCLANG_PATH": "/usr/lib/llvm-18/lib",
    }
    with Path(os.environ["GITHUB_ENV"]).open("a") as output:
        for key, value in environment.items():
            output.write(f"{key}={value}\n")
    with Path(os.environ["GITHUB_PATH"]).open("a") as output:
        output.write(f"{compiler}\n{binary.parent}\n")
    print(json.dumps({"gcc": tools["gcc"]["version"], "espflash": tools["espflash"]["version"]}))


if __name__ == "__main__":
    main()
