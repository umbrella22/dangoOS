#!/usr/bin/env python3
"""Generate the hub's CJK bitmap fonts (RFC 0006 section 2.2).

Scans the hub sources for the characters they actually display, rasterizes
them from Noto Sans SC at each size and writes one `hub-<name>.bin` per font
into crates/mosaico-s31/src/assets/. The firmware reads these with
`ui::hub_font` (an `lv_font_t` whose callbacks are written in Rust).

Usage:
    python3 assets/fonts/gen_hub_font.py --regular NotoSansSC-Regular.otf \
        --medium NotoSansSC-Medium.otf

The source fonts are not committed (8 MB each). Download them from
https://github.com/notofonts/noto-cjk/tree/main/Sans/SubsetOTF/SC
(SIL Open Font License 1.1).

File format (little endian), version 1:
    0   b"MFNT"
    4   u8  version (1)
    5   u8  bits per pixel (4)
    6   u16 line height, px
    8   u16 base line (descent below the baseline), px
    10  u16 glyph count
    12  u32 reserved (0)
    16  glyph table, count x 16 bytes, sorted by code point:
            u32 code point, u32 bitmap offset (from the bitmap area),
            u16 advance, u8 box w, u8 box h, i8 ofs x, i8 ofs y, u16 reserved
    ..  bitmap area: per glyph, rows of ceil(w / 2) bytes, high nibble first
"""

import argparse
import pathlib
import re
import struct

from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parents[2]
CRATE = ROOT / "crates" / "mosaico-s31"
OUT = CRATE / "src" / "assets"
# Every file whose string literals reach the screen through a hub font.
SOURCES = [CRATE / "src" / "hub", CRATE / "src" / "ui" / "hub_view.rs"]

ASCII = "".join(chr(c) for c in range(0x20, 0x7F))
EXTRA = "°·–…×"
DIGITS = "0123456789:"

# name, size px, weight, character set ("text" = scanned + ASCII)
FONTS = [
    ("20", 20, "regular", "text"),
    ("24", 24, "regular", "text"),
    ("30", 30, "medium", "text"),
    ("digits-64", 64, "regular", "digits"),
]


def is_cjk(ch: str) -> bool:
    cp = ord(ch)
    return (
        0x3000 <= cp <= 0x303F  # CJK punctuation
        or 0x4E00 <= cp <= 0x9FFF  # unified ideographs
        or 0xFF00 <= cp <= 0xFFEF  # full-width forms
    )


def scanned_characters() -> str:
    """CJK characters inside string literals of the hub sources."""
    literal = re.compile(r'"((?:[^"\\]|\\.)*)"')
    found = set()
    for source in SOURCES:
        files = sorted(source.rglob("*.rs")) if source.is_dir() else [source]
        for path in files:
            text = path.read_text(encoding="utf-8")
            for match in literal.finditer(text):
                found.update(ch for ch in match.group(1) if is_cjk(ch))
    return "".join(sorted(found))


def quantize(value: int) -> int:
    return (value * 15 + 127) // 255


def build(font: ImageFont.FreeTypeFont, chars: str) -> bytes:
    # Noto's own metrics give a 1.45 em line, too loose for 480 px cards.
    # 1.3 em with the baseline 0.24 em up still clears every CJK glyph box.
    size = font.size
    line_height = round(size * 1.3)
    descent = round(size * 0.24)
    glyphs = []
    bitmaps = bytearray()
    for ch in sorted(set(chars)):
        x0, y0, x1, y1 = font.getbbox(ch, anchor="ls")
        advance = round(font.getlength(ch))
        w, h = max(0, x1 - x0), max(0, y1 - y0)
        offset = len(bitmaps)
        if w and h:
            image = Image.new("L", (w, h), 0)
            ImageDraw.Draw(image).text((-x0, -y0), ch, font=font, fill=255, anchor="ls")
            pixels = image.load()
            for y in range(h):
                for x in range(0, w, 2):
                    hi = quantize(pixels[x, y])
                    lo = quantize(pixels[x + 1, y]) if x + 1 < w else 0
                    bitmaps.append((hi << 4) | lo)
        assert w < 256 and h < 256 and -128 <= x0 < 128 and -128 <= -y1 < 128, ch
        # ofs_y: bottom of the box above the baseline (LVGL convention).
        glyphs.append((ord(ch), offset, advance, w, h, x0, -y1))
    header = b"MFNT" + struct.pack("<BBHHHI", 1, 4, line_height, descent, len(glyphs), 0)
    table = b"".join(struct.pack("<IIHBBbbH", cp, off, adv, w, h, ox, oy, 0) for cp, off, adv, w, h, ox, oy in glyphs)
    return header + table + bytes(bitmaps)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--regular", required=True, type=pathlib.Path)
    parser.add_argument("--medium", required=True, type=pathlib.Path)
    args = parser.parse_args()

    text = ASCII + EXTRA + scanned_characters()
    print(f"{len(scanned_characters())} CJK characters: {scanned_characters()}")
    for name, size, weight, charset in FONTS:
        path = args.regular if weight == "regular" else args.medium
        font = ImageFont.truetype(str(path), size)
        data = build(font, text if charset == "text" else DIGITS)
        out = OUT / f"hub-{name}.bin"
        out.write_bytes(data)
        print(f"{out.relative_to(ROOT)}: {len(data)} bytes")


if __name__ == "__main__":
    main()
