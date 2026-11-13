#!/usr/bin/env python3
"""RCR1 rescue client. Export is read only; install is a separate command."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
import secrets
from pathlib import Path
import struct
import time
import zlib

MAGIC = b"RCR1"
PAYLOAD = 4096
OPS = {"hello": 1, "nor": 2, "nand": 3, "mount": 4, "list": 5,
       "file": 6, "logical": 7, "status": 8, "install-enter": 0x10,
       "install-data": 0x11, "install-finish": 0x12, "install-leave": 0x13,
       "boot": 0x14, "rom-download": 0x15}
ERRORS = {1: "invalid request", 2: "media I/O error", 3: "unavailable",
          4: "read only", 5: "not found", 6: "cancelled on device"}


class DeviceError(OSError):
    def __init__(self, status):
        self.status = status
        super().__init__(ERRORS.get(status, f"RCR1 error {status}"))


def encode(ident: int, operation: int, data: bytes = b"", status: int = 0) -> bytes:
    if len(data) > PAYLOAD:
        raise ValueError("one USB payload exceeds 4096 bytes")
    fields = struct.pack("<IBBH", ident, operation, status, len(data))
    return MAGIC + fields + struct.pack("<I", zlib.crc32(fields + data)) + data


class Decoder:
    def __init__(self):
        self.buffer = bytearray()

    def feed(self, data):
        packets = []
        for b in data:
            self.buffer.append(b)
            if len(self.buffer) <= 4:
                while not MAGIC.startswith(self.buffer):
                    del self.buffer[0]
                continue
            if len(self.buffer) < 16:
                continue
            ident, op, status, size, crc = struct.unpack("<IBBHI", self.buffer[4:16])
            if size > PAYLOAD:
                self.buffer.clear()
                raise ValueError("invalid RCR1 payload length")
            if len(self.buffer) < 16 + size:
                continue
            payload = bytes(self.buffer[16:])
            actual = zlib.crc32(self.buffer[4:12] + payload)
            self.buffer.clear()
            if actual != crc:
                raise ValueError("RCR1 CRC mismatch")
            packets.append((ident, op, status, payload))
        return packets


class Client:
    def __init__(self, serial_port, timeout=30):
        self.serial = serial_port
        self.timeout = timeout
        self.ident = secrets.randbits(32)
        self.decoder = Decoder()

    def request(self, operation, data=b""):
        op = OPS[operation]
        self.ident = (self.ident + 1) & 0xFFFFFFFF
        wire = encode(self.ident, op, data)
        while wire:
            n = self.serial.write(wire)
            if not n:
                raise OSError("USB write made no progress")
            wire = wire[n:]
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            # A short final USB packet must not wait for a full 4 KiB serial
            # read timeout on every chunk of a whole-disk export.
            available = min(PAYLOAD, self.serial.in_waiting or 1)
            for ident, reply_op, status, payload in self.decoder.feed(self.serial.read(available)):
                if ident != self.ident:
                    continue
                if reply_op != op:
                    raise ValueError("RCR1 operation mismatch")
                if status:
                    raise DeviceError(status)
                return payload
        raise TimeoutError("USB request timed out; resume the export after reconnecting")

    def hello(self):
        value = json.loads(self.request("hello"))
        if ((value.get("protocol"), value.get("board")) != ("RCR1", "mosaico-s31")
                or value.get("mode") not in ("read-only", "install")):
            raise ValueError("connected device is not the independent S31 Recovery")
        self.identity = value["identity"]
        return value


def private_file(path, mode):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = os.O_RDWR | os.O_CREAT
    if "w" in mode:
        flags |= os.O_TRUNC
    if "a" in mode:
        flags |= os.O_APPEND
    fd = os.open(path, flags, 0o600)
    os.fchmod(fd, 0o600)
    return os.fdopen(fd, mode)


def export_range(client, medium, output, total, chunk=PAYLOAD, resume=False):
    """Resume only a prefix matching its receipts and the connected medium."""
    if medium not in ("nor", "logical") or not 1 <= chunk <= PAYLOAD:
        raise ValueError("invalid export medium or USB chunk size")
    output = Path(output)
    partial = output.with_name(output.name + ".partial")
    receipt = output.with_name(output.name + ".chunks.jsonl")
    offset = 0
    receipt_bytes = 0
    digest = hashlib.sha256()
    with private_file(partial, "r+b") as disk:
        if resume and receipt.exists():
            with receipt.open("rb") as records:
                for line in records:
                    # A torn last receipt has not committed that chunk. The
                    # extra disk tail is truncated only after prefix validation.
                    if not line.endswith(b"\n"):
                        break
                    record = json.loads(line)
                    if (record["medium"] != medium or record["total"] != total
                            or record["offset"] != offset or record["identity"] != client.identity
                            or not 1 <= record["length"] <= PAYLOAD or offset + record["length"] > total):
                        raise ValueError("export receipt identity or offset mismatch")
                    data = disk.read(record["length"])
                    if hashlib.sha256(data).hexdigest() != record["sha256"]:
                        raise ValueError("existing export bytes fail resume verification")
                    fmt = "<IH" if medium == "nor" else "<QH"
                    current = client.request(medium, struct.pack(fmt, offset, len(data)))
                    if current != data:
                        raise ValueError("connected medium changed since the interrupted export")
                    digest.update(data)
                    offset += len(data)
                    receipt_bytes += len(line)
            if offset > total:
                raise ValueError("export receipt exceeds medium capacity")
        elif not resume:
            disk.truncate(0)
        else:
            raise ValueError("resume needs the matching partial file and receipt")
        disk.seek(offset)
        disk.truncate(offset)
        with private_file(receipt, "r+" if resume else "w") as records:
            if resume:
                records.seek(receipt_bytes)
                records.truncate()
            while offset < total:
                length = min(chunk, total - offset)
                fmt = "<IH" if medium == "nor" else "<QH"
                data = client.request(medium, struct.pack(fmt, offset, length))
                if len(data) != length:
                    raise ValueError(f"short {medium} read at {offset:#x}")
                disk.write(data)
                disk.flush()
                os.fsync(disk.fileno())
                record = {"medium": medium, "identity": client.identity, "total": total, "offset": offset,
                          "length": length, "sha256": hashlib.sha256(data).hexdigest()}
                records.write(json.dumps(record) + "\n")
                records.flush()
                os.fsync(records.fileno())
                digest.update(data)
                offset += length
                if offset % (256 * 1024) == 0 or offset == total:
                    print(f"{medium}: {offset}/{total}", flush=True)
    os.replace(partial, output)
    result = {"medium": medium, "identity": client.identity, "bytes": total, "sha256": digest.hexdigest(), "complete": True}
    output.with_name(output.name + ".json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def export_nand(client, output, geometry, raw=True, start=0, pages=None):
    """Fixed-position data+OOB file with one status record for every page.

    Transport failures leave zero placeholders at the original page position,
    and complete remains false. ECC failures preserve returned physical bytes.
    """
    output = Path(output)
    size = geometry["data_bytes"] + geometry["spare_bytes"]
    total_pages = geometry["blocks"] * geometry["pages_per_block"]
    if pages is None:
        pages = total_pages - start
    if start < 0 or pages < 0 or start + pages > total_pages:
        raise ValueError("NAND page range outside physical geometry")
    errors, uncorrectable = [], []
    manifest = output.with_name(output.name + ".json")
    manifest.write_text(json.dumps({"medium": "nand", "identity": client.identity, "complete": False,
                                    "state": "reading", "start_page": start, "pages": pages}) + "\n")
    partial = output.with_name(output.name + ".partial")
    with private_file(partial, "w+b") as disk, private_file(output.with_name(output.name + ".pages.jsonl"), "w") as records:
        for page in range(start, start + pages):
            try:
                data = client.request("nand", struct.pack("<IB", page, int(raw)))
                if len(data) != 12 + size or struct.unpack("<I", data[:4])[0] != page or data[4] != int(raw):
                    raise ValueError("NAND page address, mode or length mismatch")
                payload = data[12:]
                record = {"page": page, "file_offset": (page - start) * size, "raw": raw,
                          "status": data[5], "extended": data[6] if data[7] else None,
                          "corrected_min": data[8], "corrected_max": data[9],
                          "uncorrectable": bool(data[10]), "sha256": hashlib.sha256(payload).hexdigest()}
                if record["uncorrectable"]:
                    uncorrectable.append(page)
            except DeviceError as error:
                if error.status != 2:
                    raise
                errors.append(page)
                payload = bytes(size)
                record = {"page": page, "file_offset": (page - start) * size,
                          "error": str(error), "placeholder": "zero", "raw": raw}
            disk.write(payload)
            disk.flush()
            records.write(json.dumps(record) + "\n")
            records.flush()
            if (page - start + 1) % geometry["pages_per_block"] == 0:
                os.fsync(disk.fileno()); os.fsync(records.fileno())
                print(f"nand: {page + 1 - start}/{pages}, read_errors={len(errors)}", flush=True)
    os.replace(partial, output)
    result = {"medium": "nand", "identity": client.identity, "start_page": start, "pages": pages, "page_bytes": size,
              "raw": raw, "complete": not errors and not uncorrectable, "read_errors": errors,
              "uncorrectable_pages": uncorrectable, "geometry": geometry}
    output.with_name(output.name + ".json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def get_file(client, volume, path, output):
    prefix = bytes([volume])
    wire_path = path.encode("utf-8")
    output = Path(output)
    partial = output.with_name(output.name + ".partial")
    offset, size = 0, None
    digest = hashlib.sha256()
    with private_file(partial, "w+b") as disk:
        while size is None or offset < size:
            data = client.request("file", prefix + struct.pack("<IH", offset, PAYLOAD - 4) + wire_path)
            if len(data) < 4:
                raise ValueError("file reply is missing size")
            current_size = struct.unpack("<I", data[:4])[0]
            if size is not None and size != current_size:
                raise ValueError("file size changed during rescue")
            size = current_size
            payload = data[4:]
            if offset + len(payload) > size or (not payload and offset < size):
                raise ValueError("file extraction ended before its declared size")
            disk.write(payload); digest.update(payload); offset += len(payload)
        disk.flush(); os.fsync(disk.fileno())
    os.replace(partial, output)
    return {"path": path, "bytes": size, "sha256": digest.hexdigest(), "complete": True}


def install_image(client, data, info, boot=False):
    """The caller validates the complete image before opening writable mode.

    Transfers are sequential and limited by the RCR1 packet representation.
    An interruption requires a fresh install; it never marks a partial image
    bootable. FINISH is acknowledged only after device-side complete readback.
    """
    if len(data) != info["bytes"] or hashlib.sha256(data).hexdigest() != info["sha256"]:
        raise ValueError("image changed after validation")
    if info["layout"] != "S31ONE03":
        raise ValueError("unsupported main layout")
    enter = b"DGINST3\0" + b"S31ONE03" + struct.pack("<I", len(data)) + bytes.fromhex(info["sha256"])
    if client.request("install-enter", enter) != b"installing":
        raise ValueError("Recovery did not enter install mode")
    try:
        for offset in range(0, len(data), PAYLOAD - 4):
            chunk = data[offset:offset + PAYLOAD - 4]
            reply = client.request("install-data", struct.pack("<I", offset) + chunk)
            if reply != struct.pack("<I", offset + len(chunk)):
                raise ValueError("Recovery install offset mismatch")
            if offset // (1024 * 1024) != (offset + len(chunk)) // (1024 * 1024):
                print(f"install: {offset + len(chunk)}/{len(data)}", flush=True)
        if client.request("install-finish") != b"trial-ready":
            raise ValueError("Recovery did not verify the complete main image")
    except BaseException:
        # Never retry a mutating command after an ambiguous transport result.
        # A reachable device can leave write mode; Installing remains durable.
        try:
            client.request("install-leave")
        except (OSError, ValueError):
            pass
        raise
    result = {**info, "device_readback_verified": True, "state": "trial", "boot_requested": boot}
    if boot and client.request("boot", b"BOOT") != b"reboot":
        raise ValueError("Recovery rejected trial boot")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", required=True)
    parser.add_argument("--timeout", type=float, default=30, help="USB request timeout, seconds")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("info"); sub.add_parser("status")
    sub.add_parser("boot", help="boot a verified Trial or Ready main image")
    sub.add_parser("rom-download", help="explicitly leave Recovery and enter the factory ROM downloader")
    p = sub.add_parser("install", help="replace main only; interrupted installs remain in Recovery")
    p.add_argument("image", type=Path); p.add_argument("--boot", action="store_true")
    for name in ("nor", "logical"):
        p = sub.add_parser(name); p.add_argument("output", type=Path); p.add_argument("--resume", action="store_true")
    p = sub.add_parser("nand"); p.add_argument("output", type=Path)
    p.add_argument("--ecc", action="store_true"); p.add_argument("--start-page", type=int, default=0); p.add_argument("--pages", type=int)
    for name in ("ls", "get"):
        p = sub.add_parser(name); p.add_argument("volume", choices=("data", "system")); p.add_argument("path")
        if name == "get": p.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.command == "install":
        from main_image import read_image
        image, image_info = read_image(args.image)
    import serial
    with serial.Serial(args.port, 115200, timeout=.1, write_timeout=args.timeout, exclusive=True) as port:
        port.dtr = True
        client = Client(port, args.timeout); info = client.hello()
        if args.command == "info": result = info
        elif args.command == "status": result = json.loads(client.request("status"))
        elif args.command == "boot": result = {"reply": client.request("boot", b"BOOT").decode("ascii")}
        elif args.command == "rom-download": result = {"reply": client.request("rom-download", b"ROM!").decode("ascii")}
        elif args.command == "install": result = install_image(client, image, image_info, args.boot)
        elif args.command == "nor": result = export_range(client, "nor", args.output, info["nor_bytes"], resume=args.resume)
        elif args.command == "logical":
            g = json.loads(client.request("mount", b"\0"))
            result = export_range(client, "logical", args.output, g["bytes"], resume=args.resume)
        elif args.command == "nand": result = export_nand(client, args.output, info["nand"], not args.ecc, args.start_page, args.pages)
        else:
            volume = 0 if args.volume == "data" else 1
            client.request("mount", bytes([volume]))
            if args.command == "get": result = get_file(client, volume, args.path, args.output)
            else:
                result = []; index = 0
                while True:
                    e = json.loads(client.request("list", bytes([volume]) + struct.pack("<I", index) + args.path.encode()))
                    if e.get("end"): break
                    result.append(e); index += 1
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
