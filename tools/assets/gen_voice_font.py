#!/usr/bin/env python3
"""Build the separate 20px voice CJK resource; it is never embedded in firmware.

Uses the SIL OFL Noto Sans SC Regular source described in gen_hub_font.py.
Install the resulting file with console.py voice-font-push <file>.
"""
import argparse
import hashlib
from pathlib import Path
from PIL import ImageFont
from gen_hub_font import ASCII, EXTRA, build


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--regular", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    # Include basic CJK, CJK punctuation and full-width forms. This is an
    # explicit coverage contract, not a promise to render every Unicode script.
    characters = "".join(sorted(set(ASCII + EXTRA + "".join(
        chr(cp) for lo, hi in [(0x4E00, 0xA000), (0x3000, 0x3040), (0xFF00, 0xFFF0)]
        for cp in range(lo, hi)))))
    data = build(ImageFont.truetype(str(args.regular), 20), characters)
    if len(data) > 5 * 1024 * 1024:
        parser.error("font exceeds the firmware's 5 MiB resource limit")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(data)
    print(f"{args.output}: {len(data)} bytes; sha256={hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main()
