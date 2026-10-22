"""Offline ESP32-S31 application-image validation, independent of firmware code."""
import hashlib
import struct
from pathlib import Path

SLOT_BYTES = 0x300000
AB_LAYOUT_MARKER = b"MOSAICO_S31_AB_LAYOUT_V1\0"
MEMORY = ((0x40000000, 0x54000000), (0x2F000000, 0x2F080000), (0x2E000000, 0x2E008000))


def validate_image(data):
    if not 24 <= len(data) <= SLOT_BYTES:
        raise ValueError("image must fit the 3 MiB application slot")
    if data[0] != 0xE9 or not 1 <= data[1] <= 16:
        raise ValueError("invalid ESP image header or segment count")
    if struct.unpack_from("<H", data, 12)[0] != 32:
        raise ValueError("image chip must be ESP32-S31 (32)")
    if data[2] != 2 or data[3] != 0x40:
        raise ValueError("image must declare 16 MiB flash, DIO, 40 MHz")
    if data[23] != 1 or any(data[19:23]):
        raise ValueError("embedded SHA256 is required and reserved header fields must be zero")
    entry = struct.unpack_from("<I", data, 4)[0]
    pos, checksum, segments = 24, 0xEF, []
    for index in range(data[1]):
        if pos + 8 > len(data):
            raise ValueError("truncated segment header")
        address, length = struct.unpack_from("<II", data, pos)
        pos += 8
        if not length or length % 4 or pos + length > len(data):
            raise ValueError("invalid or truncated segment length")
        if not any(lo <= address < address + length <= hi for lo, hi in MEMORY):
            raise ValueError("segment outside ESP32-S31 memory")
        if any(address < hi and lo < address + length for lo, hi in segments):
            raise ValueError("overlapping load segments")
        segments.append((address, address + length))
        if index == 0:
            if length < 256 or struct.unpack_from("<I", data, pos)[0] != 0xABCD5432:
                raise ValueError("missing esp_app_desc in first segment")
            def cstring(offset):
                raw = data[pos + offset:pos + offset + 32]
                if b"\0" not in raw:
                    raise ValueError("unterminated application metadata")
                end = raw.index(0)
                if end == 0 or any(not 33 <= byte <= 126 for byte in raw[:end]) or any(raw[end:]):
                    raise ValueError("invalid application metadata")
                return raw[:end].decode("ascii")
            version, project = cstring(16), cstring(48)
            if project != "mosaico-s31":
                raise ValueError("image project must be mosaico-s31")
        for byte in data[pos:pos + length]:
            checksum ^= byte
        pos += length
    if not any(lo <= entry < hi for lo, hi in segments):
        raise ValueError("entry point is outside loaded segments")
    checksum_at = (pos // 16 + 1) * 16 - 1
    if len(data) != checksum_at + 33:
        raise ValueError("truncated hash or unexpected trailing image bytes")
    if any(data[pos:checksum_at]):
        raise ValueError("nonzero image padding")
    if data[checksum_at] != checksum:
        raise ValueError("ESP segment XOR checksum mismatch")
    if hashlib.sha256(data[:-32]).digest() != data[-32:]:
        raise ValueError("embedded image SHA256 mismatch")
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "embedded_sha256": data[-32:].hex(), "project": project,
            "version": version, "entry": entry, "segments": len(segments),
            "flash_bytes": 0x1000000, "ab_layout": AB_LAYOUT_MARKER in data[:-32]}


def read_image(path, *, require_ab=True):
    with Path(path).open("rb") as stream:
        data = stream.read(SLOT_BYTES + 1)
    info = validate_image(data)
    if require_ab and not info["ab_layout"]:
        raise ValueError("image lacks A/B layout compatibility marker; older firmware may format an application slot as data")
    return data, info


if __name__ == "__main__":
    import argparse
    import json
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    parser.add_argument("--structural-only", action="store_true", help="inspect an older image without accepting it for migration or OTA")
    args = parser.parse_args()
    try:
        print(json.dumps(read_image(args.image, require_ab=not args.structural_only)[1], indent=2))
    except (ValueError, OSError) as error:
        parser.exit(2, f"image rejected: {error}\n")
