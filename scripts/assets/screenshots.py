#!/usr/bin/env python3
"""Capture the native UI and repository 2048 app for README, using only stdlib."""
from pathlib import Path
import argparse
import struct
import subprocess
import tempfile
import zlib

ROOT = Path(__file__).resolve().parents[2]


def png(ppm):
    magic, size, maximum, pixels = ppm.split(b"\n", 3)
    width, height = map(int, size.split())
    if magic != b"P6" or maximum != b"255" or len(pixels) != width * height * 3:
        raise ValueError("unsupported screenshot format")

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(b"\0" + pixels[y * width * 3:(y + 1) * width * 3] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=ROOT / "screenshots")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="dango-screenshots-") as temporary:
        subprocess.run(["cargo", "run", "-p", "os", "--locked", "--offline", "--example", "screenshots", "--", temporary],
                       cwd=ROOT, check=True)
        args.out.mkdir(parents=True, exist_ok=True)
        for source in sorted(Path(temporary).glob("*.ppm")):
            target = args.out / source.with_suffix(".png").name
            target.write_bytes(png(source.read_bytes()))
            print(target)


if __name__ == "__main__":
    main()
