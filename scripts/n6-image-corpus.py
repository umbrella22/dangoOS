#!/usr/bin/env python3
"""Create independent ESP image mutations for host tests. Never flash this corpus.

Usage: python3 scripts/n6-image-corpus.py <valid-s31.bin> <new-output-directory>
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
from ota_image import AB_LAYOUT_MARKER, validate_image


def create(image, destination):
    baseline = Path(image).read_bytes()
    info = validate_image(baseline)
    root = Path(destination)
    root.mkdir(parents=True, exist_ok=False)
    segments, pos = [], 24
    for _ in range(baseline[1]):
        address, length = struct.unpack_from("<II", baseline, pos)
        segments.append(dict(header_offset=pos, load_address=address, length=length))
        pos += 8 + length
    checksum_at = (pos // 16 + 1) * 16 - 1
    entries = []

    def save(name, data, expect, reason):
        (root / name).write_bytes(data)
        entries.append(dict(file=name, bytes=len(data), sha256=hashlib.sha256(data).hexdigest(), expect=expect, reason=reason))

    def mutate(name, offset, payload, reason, *, payload_changed=False, expect="reject"):
        data = bytearray(baseline)
        data[offset:offset + len(payload)] = payload
        if payload_changed:
            checksum = 0xEF
            for segment in segments:
                lo = segment["header_offset"] + 8
                for byte in data[lo:lo + segment["length"]]:
                    checksum ^= byte
            data[checksum_at] = checksum
        data[-32:] = hashlib.sha256(data[:-32]).digest()
        save(name, data, expect, reason)

    save("valid-s31-16mb.bin", baseline, "accept", "Original image; verify independently with esptool image-info")
    mutate("bad-magic.bin", 0, b"\0", "Invalid header magic with repaired embedded SHA")
    mutate("wrong-chip.bin", 12, struct.pack("<H", 9), "ESP32-S3 instead of S31")
    mutate("declared-4mb.bin", 3, b"\x20", "4 MiB cannot cover slot 1")
    mutate("zero-segments.bin", 1, b"\0", "No load segments")
    mutate("overflow-segment.bin", 28, struct.pack("<I", 0xFFFFFFF0), "Segment exceeds image")
    mutate("invalid-load-address.bin", 24, struct.pack("<I", 0xFFFF0000), "Unsupported load address")
    mutate("invalid-entry-point.bin", 4, b"\0" * 4, "Entry outside loaded memory")
    mutate("missing-appdesc.bin", 32, b"\0" * 4, "Missing appdesc; checksum and embedded hash repaired", payload_changed=True)
    mutate("bad-checksum.bin", checksum_at, bytes([baseline[checksum_at] ^ 0x80]), "Bad XOR checksum with repaired SHA")
    bad = bytearray(baseline)
    bad[-1] ^= 1
    save("bad-embedded-hash.bin", bad, "reject", "Correct transport SHA, wrong embedded SHA")
    save("truncated-header.bin", baseline[:12], "reject", "Incomplete header")
    save("truncated-segment.bin", baseline[:segments[-1]["header_offset"] + 12], "reject", "Incomplete load data")
    save("truncated-hash.bin", baseline[:-1], "reject", "Missing final digest byte")
    save("trailing-payload.bin", baseline + b"extra-data", "reject", "Unexpected trailing bytes")
    mutate("wrong-project.bin", 32 + 48, b"another-board\0".ljust(32, b"\0"), "Foreign app descriptor", payload_changed=True)
    mutate("unterminated-version.bin", 32 + 16, b"x" * 32, "App version lacks NUL", payload_changed=True)
    mutate("nonascii-version.bin", 32 + 16, b"\xc3\xa9\0".ljust(32, b"\0"), "App version must be ASCII", payload_changed=True)
    mutate("space-version.bin", 32 + 16, b"0. 2\0".ljust(32, b"\0"), "App version must be graphic ASCII", payload_changed=True)
    mutate("noncanonical-project.bin", 32 + 48, b"mosaico-s31\0garbage".ljust(32, b"\0"), "Nonzero app project suffix after NUL", payload_changed=True)
    mutate("reserved-header.bin", 19, b"\x80", "Reserved header field set")
    mutate("missing-hash-flag.bin", 23, b"\0", "No embedded hash flag")
    if len(segments) > 1:
        mutate("overlapping-segment.bin", segments[-1]["header_offset"], struct.pack("<I", segments[0]["load_address"]), "Overlapping load ranges")
    if info["ab_layout"]:
        mutate("legacy-layout-marker.bin", baseline.index(AB_LAYOUT_MARKER), b"\0" * len(AB_LAYOUT_MARKER), "Structurally valid but must be rejected by the OTA compatibility gate", payload_changed=True, expect="accept")
    receipt = dict(purpose="Independent host mutation corpus, not bootable acceptance artifacts; never flash mutations", source=info, segments=segments, checksum_offset=checksum_at, cases=entries)
    (root / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"Created {len(entries)} cases in {root}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument("output")
    args = parser.parse_args()
    create(args.image, args.output)
