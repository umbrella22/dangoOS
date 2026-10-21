#!/usr/bin/env python3
"""Convert WebP/GIF/PNG sequence animation into a Rust RGB565 sprite table.

The firmware renders the ball procedurally today, but character art (ears,
tails, whole replacement characters) has to arrive as pixels. This script is
the agreed entry point for that art: give it a WebP/GIF and it produces a Rust
module the firmware can include.

Pipeline:

    WebP/GIF/PNG sequence
      -> ffmpeg decode to RGBA8 (rawvideo)
      -> center-crop to square, scale to --size with area averaging
      -> composite over black (the panel background)
      -> RGB565 little-endian words
      -> Rust `pub static FRAMES: &[&[u16]] = ...` module

Requirements: `ffmpeg` on PATH. No Python packages.

Usage:

    python3 crates/grok-ball/tools/sprite_pack.py \
        --input art/blob-struggle-loop.webp \
        --name struggle \
        --size 240 \
        --out crates/grok-ball/src/sprite/struggle.rs

    # Also write PPM previews so the quantized result can be inspected:
    python3 crates/grok-ball/tools/sprite_pack.py \
        --input art/blob-struggle-loop.webp \
        --name struggle --preview target/sprite-preview
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path


def require_ffmpeg() -> str:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        sys.exit("ffmpeg not found on PATH; install it (brew install ffmpeg)")
    return ffmpeg


def probe(ffmpeg: str, path: Path) -> dict:
    """Return width, height, frame count and average frame duration."""
    ffprobe = shutil.which("ffprobe")
    if ffprobe:
        out = subprocess.run(
            [
                ffprobe,
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,nb_frames,avg_frame_rate,duration",
                "-of",
                "json",
                str(path),
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        stream = json.loads(out)["streams"][0]
        rate_num, rate_den = (stream.get("avg_frame_rate") or "12/1").split("/")
        fps = float(rate_num) / float(rate_den or 1)
        return {
            "width": int(stream["width"]),
            "height": int(stream["height"]),
            "fps": fps if fps > 0 else 12.0,
        }

    # No ffprobe: decode once and rely on the frame count we get.
    return {"width": 0, "height": 0, "fps": 12.0}


def decode(ffmpeg: str, path: Path, size: int) -> list[bytes]:
    """Decode every frame as RGBA8, center-cropped and scaled to size x size."""
    # `crop` needs even dimensions; `scale` keeps aspect and `pad` squares it.
    vf = (
        f"crop='min(iw,ih)':'min(iw,ih)',"
        f"scale={size}:{size}:flags=area,"
        "format=rgba"
    )
    proc = subprocess.run(
        [
            ffmpeg,
            "-v",
            "error",
            "-i",
            str(path),
            "-vf",
            vf,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ],
        check=True,
        capture_output=True,
    )
    frame_bytes = size * size * 4
    data = proc.stdout
    if len(data) % frame_bytes != 0:
        sys.exit(
            f"decoded {len(data)} bytes, not a multiple of one {size}x{size} RGBA frame"
        )
    frames = [data[i : i + frame_bytes] for i in range(0, len(data), frame_bytes)]
    if not frames:
        sys.exit("no frames decoded")
    return frames


def to_rgb565(frame: bytes, size: int, bg: tuple[int, int, int] = (0, 0, 0)) -> list[int]:
    """Composite RGBA over `bg` and pack RGB565 little-endian words."""
    out: list[int] = []
    for index in range(0, len(frame), 4):
        r, g, b, a = frame[index], frame[index + 1], frame[index + 2], frame[index + 3]
        if a != 255:
            # Alpha blend against the panel background.
            inv = 255 - a
            r = (r * a + bg[0] * inv) // 255
            g = (g * a + bg[1] * inv) // 255
            b = (b * a + bg[2] * inv) // 255
        r5 = (r >> 3) & 0x1F
        g6 = (g >> 2) & 0x3F
        b5 = (b >> 3) & 0x1F
        out.append((r5 << 11) | (g6 << 5) | b5)
    assert len(out) == size * size
    return out


def write_preview(directory: Path, name: str, frames: list[list[int]], size: int) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    for index, pixels in enumerate(frames):
        path = directory / f"{name}-{index:02}.ppm"
        with open(path, "wb") as handle:
            handle.write(f"P6\n{size} {size}\n255\n".encode())
            row = bytearray()
            for pixel in pixels:
                r = (pixel >> 11) & 0x1F
                g = (pixel >> 5) & 0x3F
                b = pixel & 0x1F
                row.append((r << 3) | (r >> 2))
                row.append((g << 2) | (g >> 4))
                row.append((b << 3) | (b >> 2))
            handle.write(bytes(row))


def write_rust(
    out: Path,
    name: str,
    frames: list[list[int]],
    size: int,
    frame_ms: int,
    loop_forever: bool,
) -> None:
    ident = "".join(ch if ch.isalnum() else "_" for ch in name).upper()
    lines = [
        "// @generated by crates/grok-ball/tools/sprite_pack.py; do not edit.",
        "//",
        f"// Sprite: {name}",
        f"// Canvas: {size}x{size} RGB565, {len(frames)} frame(s), {frame_ms} ms/frame,",
        f"// loop: {'yes' if loop_forever else 'no (hold last frame)'}.",
        "",
        "#![allow(clippy::unreadable_literal)]",
        "",
        f"/// Canvas edge in pixels.",
        f"pub const SIZE: usize = {size};",
        f"/// Frame duration in milliseconds.",
        f"pub const FRAME_MS: u32 = {frame_ms};",
        f"/// Whether the animation loops.",
        f"pub const LOOPING: bool = {'true' if loop_forever else 'false'};",
        f"/// Number of frames.",
        f"pub const FRAME_COUNT: usize = {len(frames)};",
        "",
        f"pub static FRAMES: [&[u16; SIZE * SIZE]; FRAME_COUNT] = [",
    ]
    for pixels in frames:
        lines.append("    &[")
        for start in range(0, len(pixels), 16):
            chunk = ", ".join(f"0x{value:04X}" for value in pixels[start : start + 16])
            lines.append(f"        {chunk},")
        lines.append("    ],")
    lines.append("];")
    lines.append("")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines))
    _ = ident


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--name", required=True, help="sprite name, e.g. struggle")
    parser.add_argument("--size", type=int, default=240, help="square canvas edge")
    parser.add_argument(
        "--out", type=Path, help="Rust module to write (default: print stats only)"
    )
    parser.add_argument(
        "--fps", type=float, default=None, help="override the frame rate"
    )
    parser.add_argument(
        "--once",
        action="store_true",
        help="one-shot animation: hold the last frame instead of looping",
    )
    parser.add_argument("--preview", type=Path, help="write PPM previews here")
    args = parser.parse_args()

    if not args.input.exists():
        sys.exit(f"input not found: {args.input}")

    ffmpeg = require_ffmpeg()
    info = probe(ffmpeg, args.input)
    frames = decode(ffmpeg, args.input, args.size)
    fps = args.fps or info["fps"] or 12.0
    frame_ms = max(1, int(round(1000.0 / fps)))

    packed = [to_rgb565(frame, args.size) for frame in frames]
    opaque = [
        sum(1 for index in range(0, len(frame), 4) if frame[index + 3] > 8)
        for frame in frames
    ]
    flash = len(packed) * args.size * args.size * 2

    print(f"input:   {args.input}")
    print(f"canvas:  {args.size}x{args.size} RGB565")
    print(f"frames:  {len(packed)} at {frame_ms} ms ({fps:.1f} fps)")
    print(f"opaque:  min={min(opaque)} max={max(opaque)} of {args.size * args.size} px")
    print(f"flash:   {flash} bytes ({flash / 1024 / 1024:.2f} MiB)")

    if args.preview:
        write_preview(args.preview, args.name, packed, args.size)
        print(f"preview: {args.preview}")

    if args.out:
        write_rust(args.out, args.name, packed, args.size, frame_ms, not args.once)
        print(f"rust:    {args.out}")


if __name__ == "__main__":
    main()
