#!/usr/bin/env python3
"""Pack a GIF as 480x480 grayscale frames with their original durations.

BAN1 stores zlib-compressed XOR changes inside four-pixel-aligned rectangles.
The firmware needs two reusable 4-bit buffers, never all decoded GIF frames.
Pillow is only needed by this offline packer; no image service is called.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import zlib

from PIL import Image, ImageChops

SIZE = (480, 480)
WORK = SIZE[0] * SIZE[1] // 2
HEADER = struct.Struct("<4sHHHHI")
ENTRY = struct.Struct("<III4H")


def pack(source, output, preview=None):
    gif = Image.open(source)
    previous = Image.new("L", SIZE)
    images, frames, payloads, provenance = [], [], [], []
    for index in range(gif.n_frames):
        gif.seek(index)
        duration = gif.info.get("duration", 0)
        if duration <= 0:
            raise ValueError(f"frame {index} has no positive duration")
        # Composited GIF frames include disposal effects. Preserve aspect and
        # the complete square reference; there is no redraw or content crop.
        if gif.width != gif.height:
            raise ValueError("the startup reference must be square")
        frame = gif.convert("RGB").convert("L").resize(SIZE, Image.Resampling.LANCZOS)
        indexed = frame.point(lambda value: (value * 15 + 127) // 255)
        bounds = ImageChops.difference(indexed, previous).getbbox()
        if bounds:
            x0, y0, x1, y1 = bounds
            bounds = (x0 & ~3, y0, (x1 + 3) & ~3, y1)
            current = indexed.crop(bounds).tobytes()
            old = previous.crop(bounds).tobytes()
            delta = bytes(((current[i] ^ old[i]) << 4) | (current[i + 1] ^ old[i + 1])
                          for i in range(0, len(current), 2))
            payload = zlib.compress(delta, 9)
        else:
            bounds, payload = (0, 0, 0, 0), b""
        payloads.append(payload)
        frames.append((duration, bounds))
        previous = indexed
        shown = indexed.point(lambda value: value * 17)
        images.append(shown)
        provenance.append({"index": index, "duration_ms": duration, "bounds": bounds,
                           "gray4_sha256": hashlib.sha256(indexed.tobytes()).hexdigest()})
    offset = HEADER.size + ENTRY.size * len(frames)
    entries = bytearray()
    for (duration, bounds), payload in zip(frames, payloads):
        entries.extend(ENTRY.pack(offset, len(payload), duration, *bounds))
        offset += len(payload)
    bundle = HEADER.pack(b"BAN1", *SIZE, len(frames), 4, WORK) + entries + b"".join(payloads)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(bundle)
    receipt = {"source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
               "frames": len(frames), "duration_ms": sum(d for d, _ in frames),
               "bytes": len(bundle), "sha256": hashlib.sha256(bundle).hexdigest(),
               "size": SIZE, "gray_levels": 16, "frames_detail": provenance}
    if preview:
        preview.mkdir(parents=True, exist_ok=True)
        images[0].save(preview / "boot.gif", save_all=True, append_images=images[1:],
                       duration=[d for d, _ in frames], disposal=2, loop=0, optimize=False)
        images[-1].save(preview / "final.png")
        (preview / "provenance.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return {k: v for k, v in receipt.items() if k != "frames_detail"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--preview", type=Path)
    args = parser.parse_args()
    print(json.dumps(pack(args.source, args.output, args.preview), indent=2))


if __name__ == "__main__":
    main()
