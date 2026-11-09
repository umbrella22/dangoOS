"""Validate an S31ONE03 main image before entering Recovery install mode."""
import hashlib
from pathlib import Path
import struct

LAYOUT = b"S31ONE03"
MARKER = b"DANGO_S31_SINGLE_MAIN_S31ONE03\0"
MAIN_BYTES = 0xC00000
MEMORY = ((0x40000000, 0x54000000), (0x2F000000, 0x2F080000), (0x2E000000, 0x2E008000))


def validate_image(data, *, project_name="mosaico-s31", maximum=MAIN_BYTES, marker=MARKER):
    if not 288 <= len(data) <= maximum:
        raise ValueError("image must fit the S31ONE03 main partition")
    if data[0] != 0xE9 or not 1 <= data[1] <= 16:
        raise ValueError("invalid ESP image header or segment count")
    if struct.unpack_from("<H", data, 12)[0] != 32:
        raise ValueError("image chip must be ESP32-S31")
    if data[2:4] != b"\x02\x40" or data[23] != 1 or any(data[19:23]):
        raise ValueError("image requires 16 MiB/DIO/40 MHz and embedded SHA256")
    entry = struct.unpack_from("<I", data, 4)[0]
    pos, checksum, ranges = 24, 0xEF, []
    for index in range(data[1]):
        if pos + 8 > len(data):
            raise ValueError("truncated segment header")
        address, length = struct.unpack_from("<II", data, pos)
        pos += 8
        if not length or length % 4 or pos + length > len(data):
            raise ValueError("invalid segment length")
        end = address + length
        if not any(lo <= address < end <= hi for lo, hi in MEMORY):
            raise ValueError("segment outside S31 memory")
        if any(address < hi and lo < end for lo, hi in ranges):
            raise ValueError("overlapping load segments")
        ranges.append((address, end))
        if index == 0:
            if length < 256 or struct.unpack_from("<I", data, pos)[0] != 0xABCD5432:
                raise ValueError("missing application descriptor")
            def field(offset):
                raw = data[pos + offset:pos + offset + 32]
                if b"\0" not in raw:
                    raise ValueError("unterminated application metadata")
                value, _, padding = raw.partition(b"\0")
                if not value or any(padding) or not all(33 <= b <= 126 for b in value):
                    raise ValueError("invalid application metadata")
                return value.decode("ascii")
            version, project = field(16), field(48)
            if project != project_name:
                raise ValueError("image is not the S31 main application")
        for byte in data[pos:pos + length]:
            checksum ^= byte
        pos += length
    if not any(lo <= entry < hi for lo, hi in ranges):
        raise ValueError("entry outside loaded segments")
    checksum_at = (pos // 16 + 1) * 16 - 1
    if len(data) != checksum_at + 33 or any(data[pos:checksum_at]):
        raise ValueError("image padding, length or trailing data")
    if data[checksum_at] != checksum or hashlib.sha256(data[:-32]).digest() != data[-32:]:
        raise ValueError("ESP checksum or embedded SHA256 mismatch")
    if marker is not None and marker not in data[:-32]:
        raise ValueError("image lacks the S31ONE03 main contract")
    return {"layout": LAYOUT.decode(), "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "version": version, "project": project}


def read_image(path):
    with Path(path).open("rb") as stream:
        data = stream.read(MAIN_BYTES + 1)
    return data, validate_image(data)
