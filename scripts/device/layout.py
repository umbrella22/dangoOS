#!/usr/bin/env python3
"""Prepare and verify ESP32-S31 development NOR reprovisioning.

backup reads raw flash into a new directory outside the repository.
plan is offline. apply and restore require an explicit confirmation switch,
an intact backup and the same hardware MAC; they leave the download stub active.
The independent SPI NAND volume is not changed by this tool.
"""
import argparse
import contextlib
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import zlib

from ota_image import read_image, validate_image

FLASH_BYTES = 0x1000000
TABLE_AT = 0x8000
TABLE_BYTES = 0x1000
OTA_AT = 0x9000
OTA_BYTES = 0x2000
APP_AT = 0x210000
OLD_APP_BYTES = 0x7D0000
OLD_DATA_AT = 0x7E0000
OLD_DATA_BYTES = 0x200000
SLOT_BYTES = 0x600000
SLOT1_AT = 0x810000
SYSTEM_AT = 0xE10000
SYSTEM_BYTES = 0x1F0000
PROTECTED = ((0, 0x8000), (0xB000, 0x210000))
GENERATOR = Path(__file__).resolve().parent / "vendor" / "gen_esp32part.py"
GENERATOR_SHA = "02f6c1c5f012393d47987ee64bc50ba319a2eda48cbcec754d8cfaf780cce529"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def parse_table(data):
    if len(data) != TABLE_BYTES:
        raise ValueError("partition table must be one 4 KiB sector")
    rows, names = [], set()
    for offset in range(0, 0xC00, 32):
        item = data[offset:offset + 32]
        if item[:2] == b"\xeb\xeb":
            if item[:16] != b"\xeb\xeb" + b"\xff" * 14:
                raise ValueError("invalid partition MD5 header")
            if hashlib.md5(data[:offset]).digest() != item[16:]:
                raise ValueError("partition MD5 mismatch")
            if any(byte != 255 for byte in data[offset + 32:]):
                raise ValueError("unexpected data after partition checksum")
            break
        magic, kind, sub, start, size, label, flags = struct.unpack("<HBBII16sI", item)
        if magic != 0x50AA:
            raise ValueError("partition checksum missing or invalid entry")
        name = label.split(b"\0", 1)[0].decode("ascii")
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,16}", name) or name in names:
            raise ValueError("invalid or duplicate partition name")
        if flags != 0:
            raise ValueError("nonzero partition flags are unsupported by this firmware")
        if not size or start % 4096 or size % 4096 or start < 0x9000 or start + size > FLASH_BYTES:
            raise ValueError("invalid partition bounds")
        if kind == 0 and start % 65536:
            raise ValueError("application partition is not 64 KiB aligned")
        if any(start < p["offset"] + p["size"] and p["offset"] < start + size for p in rows):
            raise ValueError("overlapping partitions")
        names.add(name)
        rows.append(dict(name=name, kind=kind, subtype=sub, offset=start, size=size, flags=flags))
    else:
        raise ValueError("partition table has no MD5 terminator")
    if not rows:
        raise ValueError("empty partition table")
    return rows


def named(rows, name):
    matches = [r for r in rows if r["name"] == name]
    if len(matches) != 1:
        raise ValueError(f"missing or ambiguous {name} partition")
    return matches[0]


def require_legacy(rows):
    main, ota = named(rows, "main_app"), named(rows, "otadata")
    if (main["kind"], main["subtype"], main["offset"], main["size"], main["flags"]) != (0, 0x10, APP_AT, OLD_APP_BYTES, 0):
        raise ValueError("backup is not the positively identified legacy main_app layout")
    if (ota["kind"], ota["subtype"], ota["offset"], ota["size"], ota["flags"]) != (1, 0, OTA_AT, OTA_BYTES, 0):
        raise ValueError("unexpected otadata layout")
    for name, kind, subtype, start in [("vibe_mode", 0, 0x20, 0x20000), ("ui_apps", 1, 0x41, 0x9E0000), ("system", 1, 0x83, 0xD00000)]:
        row = named(rows, name)
        if (row["kind"], row["subtype"], row["offset"]) != (kind, subtype, start):
            raise ValueError(f"unexpected protected partition {name}")
    if any(r["name"] in ("ota_1", "data") or (r["kind"] == 0 and (r["subtype"] == 0 or (0x10 <= r["subtype"] <= 0x1F and r["name"] != "main_app"))) for r in rows):
        raise ValueError("backup already has another OTA/data layout")


def require_source(rows):
    main, ota = named(rows, "main_app"), named(rows, "otadata")
    if (main["kind"], main["subtype"], main["offset"]) != (0, 0x10, APP_AT):
        raise ValueError("source main_app identity/offset is unknown")
    if (ota["kind"], ota["subtype"], ota["offset"], ota["size"]) != (1, 0, OTA_AT, OTA_BYTES):
        raise ValueError("unexpected source otadata")
    retained = [("phy_init",1,1,0xb000,0x1000), ("sysmeta",1,2,0xc000,0x14000),
                ("vibe_mode",0,0x20,0x20000,0x1c0000), ("coredump",1,3,0x1e0000,0x20000),
                ("nvs",1,2,0x200000,0x6000)]
    for name, kind, subtype, at, size in retained:
        row = named(rows, name)
        if (row["kind"], row["subtype"], row["offset"], row["size"]) != (kind,subtype,at,size):
            raise ValueError(f"unexpected retained partition {name}")
    if {r["name"] for r in rows if r["offset"] < APP_AT} != {"otadata", *(r[0] for r in retained)}:
        raise ValueError("unknown partition in the retained NOR prefix")


def planned_rows(original):
    require_source(original)
    result = [dict(row) for row in original if row["offset"] < APP_AT]
    result.extend([
        dict(name="main_app", kind=0, subtype=0x10, offset=APP_AT, size=SLOT_BYTES, flags=0),
        dict(name="ota_1", kind=0, subtype=0x11, offset=SLOT1_AT, size=SLOT_BYTES, flags=0),
        dict(name="system", kind=1, subtype=0x83, offset=SYSTEM_AT, size=SYSTEM_BYTES, flags=0),
    ])
    return sorted(result, key=lambda row: row["offset"])


def inspect_otadata(raw):
    # ESP-IDF crc32_le(UINT32_MAX, sequence) includes entry/exit inversions.
    # Pinned esp-hal fixture: sequence 1 -> on-flash bytes 9a 98 43 47.
    def crc(sequence):
        return zlib.crc32(struct.pack("<I", sequence), 0xFFFFFFFF)
    return [dict(entry=i, sequence=(sequence := struct.unpack_from("<I", raw, i * 4096)[0]),
                 state=struct.unpack_from("<I", raw, i * 4096 + 24)[0],
                 crc_ok=(sequence not in (0, 0xFFFFFFFF) and crc(sequence) == struct.unpack_from("<I", raw, i * 4096 + 28)[0])) for i in range(2)]



def bootloader_evidence(raw):
    """Strings are observations; they cannot prove an enabled build option."""
    boot = raw[0x2000:0x8000]
    strings = [s.decode("ascii") for s in re.findall(rb"[ -~]{6,}", boot)]
    return {"idf_strings": [s for s in strings if "ESP-IDF" in s or "IDF_VER" in s][:12],
            "rollback_strings": [s for s in strings if "rollback" in s.lower() or "pending_verify" in s.lower()][:12],
            "rollback_enabled": "unknown: strings do not establish CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE"}


def load_backup(directory):
    directory = Path(directory).resolve()
    manifest = json.loads((directory / "backup.json").read_text())
    raw = (directory / "flash.bin").read_bytes()
    if manifest.get("schema") != 1 or len(raw) != FLASH_BYTES or sha(raw) != manifest["flash_sha256"]:
        raise ValueError("backup is incomplete or its raw flash hash changed")
    if not re.fullmatch(r"[0-9a-f]{2}(:[0-9a-f]{2}){5}", manifest.get("mac", "")):
        raise ValueError("backup device identity missing")
    rows = parse_table(raw[TABLE_AT:TABLE_AT + TABLE_BYTES])
    require_source(rows)
    if raw[0x2000] != 0xE9 or (raw[0x2003] >> 4) != 4:
        raise ValueError("backup bootloader does not identify unencrypted 16 MiB flash")
    return directory, manifest, raw, rows


class Flash:
    def __init__(self, port, baud, log):
        self.port, self.baud = port, baud
        self.log = log
        self.esp = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        if self.esp is not None:
            # Keep the stub running. Exiting it to ROM drops S31 USB-OTG.
            self.esp.__exit__(None, None, None)
            self.esp = None

    def command(self, *args):
        # One serial connection spans MAC, all reads, writes and readback.
        # esptool is imported only for device operations; offline planning and
        # tests do not require it. Use the Python environment that has esptool.
        import esptool
        with self.log.open("a", encoding="utf-8") as output, contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            output.write(json.dumps([str(arg) for arg in args]) + "\n")
            try:
                if self.esp is None:
                    self.esp = esptool.connect_esp(port=self.port, chip="esp32s31", before="no-reset")
                    self.esp = esptool.run_stub(self.esp)
                    self.esp.change_baud(self.baud)
                    esptool.attach_flash(self.esp)
                command, *values = args
                if command == "read-mac":
                    return "MAC: " + ":".join(f"{b:02x}" for b in self.esp.read_mac("BASE_MAC"))
                if command == "read-flash":
                    at, size, path = values
                    esptool.read_flash(self.esp, int(at, 0), int(size, 0), str(path), flash_size="16MB", no_progress=True)
                elif command == "erase-region":
                    at, size = values
                    esptool.erase_region(self.esp, int(at, 0), int(size, 0))
                elif command == "write-flash":
                    at, path = values
                    esptool.write_flash(self.esp, [(int(at, 0), str(path))], flash_size="16MB", no_progress=True)
                else:
                    raise ValueError(f"unsupported layout operation: {command}")
                return ""
            except Exception as error:
                output.write(f"{type(error).__name__}: {error}\n")
                raise RuntimeError(f"esptool failed; see {self.log}: {error}") from error

    def mac(self):
        match = re.search(r"\bMAC:\s*([0-9a-fA-F:]{17})", self.command("read-mac"))
        if not match:
            raise ValueError("cannot identify attached board MAC")
        return match[1].lower()

    def read(self, offset, length, path):
        self.command("read-flash", hex(offset), hex(length), path)
        data = path.read_bytes()
        if len(data) != length:
            raise ValueError("incomplete raw flash read")
        return data

    def boot(self):
        import esptool
        with self.log.open("a", encoding="utf-8") as output, contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            output.write('["watchdog-reset-after-verified-write"]\n')
            esptool.reset_chip(self.esp, "watchdog-reset")


def backup(args):
    directory = Path(args.out).expanduser().resolve()
    # The device tools live two directories below the repository root; a raw backup must survive removal of this repo.
    repo = Path(__file__).resolve().parents[2]
    if directory == repo or repo in directory.parents:
        raise ValueError("choose a backup directory outside the repository")
    directory.mkdir(parents=True, exist_ok=False)
    with Flash(args.port, args.baud, directory / "backup.log") as flash:
        mac = flash.mac()
        raw = flash.read(0, FLASH_BYTES, directory / "flash.bin")
        rows = parse_table(raw[TABLE_AT:TABLE_AT + TABLE_BYTES])
        require_source(rows)
        # A second independent read verifies the boot/table/otadata recovery range.
        boot = flash.read(0, 0x20000, directory / "boot-region.bin")
        if boot != raw[:0x20000]:
            raise ValueError("boot-region reread differs; backup cannot be used")
        manifest = dict(schema=1, captured_at=datetime.now(timezone.utc).isoformat(), mac=mac,
                        flash_sha256=sha(raw), boot_region_sha256=sha(boot),
                        partitions=rows, otadata=inspect_otadata(raw[OTA_AT:OTA_AT + OTA_BYTES]),
                        bootloader_evidence=bootloader_evidence(raw))
        write_json(directory / "backup.json", manifest)
        load_backup(directory)
    print(json.dumps(manifest, indent=2))


def plan(args):
    directory, manifest, raw, rows = load_backup(args.backup)
    image, image_info = read_image(args.image)
    system = Path(args.system).read_bytes()
    if len(system) != SYSTEM_BYTES:
        raise ValueError("system image has the wrong partition size")
    result = planned_rows(rows)
    if sha(GENERATOR.read_bytes()) != GENERATOR_SHA:
        raise ValueError("pinned partition generator hash changed")
    output = Path(args.out).resolve()
    output.mkdir(parents=True, exist_ok=False)
    csv = output / "partitions.csv"
    csv.write_text("# Name,Type,SubType,Offset,Size,Flags\n" + "".join(
        f"{r['name']},0x{r['kind']:x},0x{r['subtype']:x},0x{r['offset']:x},0x{r['size']:x},\n" for r in result))
    subprocess.run([sys.executable, str(GENERATOR), "--flash-size", "16MB", "--offset", "0x8000", "--primary-bootloader-offset", "0x2000", str(csv), str(output / "partitions.bin")], check=True)
    generated = (output / "partitions.bin").read_bytes()
    if len(generated) > TABLE_BYTES:
        raise ValueError("generated table exceeds one sector")
    generated += b"\xff" * (TABLE_BYTES - len(generated))
    (output / "partitions.bin").write_bytes(generated)
    if parse_table(generated) != result:
        raise ValueError("generator round trip changed partition fields")
    old_records = {r["name"]: raw[TABLE_AT + i * 32:TABLE_AT + (i + 1) * 32] for i, r in enumerate(rows)}
    for i, row in enumerate(result):
        if row["name"] in old_records and row["offset"] < APP_AT and generated[i * 32:(i + 1) * 32] != old_records[row["name"]]:
            raise ValueError("generator changed a protected partition record")
    (output / "application.bin").write_bytes(image)
    (output / "system.bin").write_bytes(system)
    receipt = dict(schema=1, backup=str(directory), backup_sha256=manifest["flash_sha256"], mac=manifest["mac"],
                   table_sha256=sha(generated), image=image_info, system_sha256=sha(system), partitions=result,
                   destructive_ranges=["application slots and system 0x210000..0x1000000", "partition table 0x8000..0x9000", "otadata 0x9000..0xB000"],
                   physical_status="not-run")
    write_json(output / "plan.json", receipt)
    print(json.dumps(receipt, indent=2))


def checked_plan(directory, manifest, rows):
    directory = Path(directory).resolve()
    receipt = json.loads((directory / "plan.json").read_text())
    table = (directory / "partitions.bin").read_bytes()
    image, image_info = read_image(directory / "application.bin")
    if receipt.get("schema") != 1 or receipt["backup_sha256"] != manifest["flash_sha256"] or receipt["mac"] != manifest["mac"]:
        raise ValueError("plan is bound to a different backup/device")
    if sha(table) != receipt["table_sha256"] or parse_table(table) != planned_rows(rows) or image_info != receipt["image"]:
        raise ValueError("planned artifacts changed")
    system = (directory / "system.bin").read_bytes()
    if len(system) != SYSTEM_BYTES or sha(system) != receipt.get("system_sha256"):
        raise ValueError("planned system image changed")
    return directory, table, image, system


def guard_protected(before, after):
    for lo, hi in PROTECTED:
        if before[lo:hi] != after[lo:hi]:
            raise ValueError(f"protected flash bytes changed at 0x{lo:x}..0x{hi:x}")


def apply(args):
    if not args.confirm_repartition:
        raise ValueError("apply requires --confirm-repartition after attended review of plan.json")
    directory, manifest, original, rows = load_backup(args.backup)
    plan_dir, table, image, system = checked_plan(args.plan, manifest, rows)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    with Flash(args.port, args.baud, directory / f"apply-{stamp}.log") as flash:
        if flash.mac() != manifest["mac"]:
            raise ValueError("attached board is not the backed-up device")
        with tempfile.TemporaryDirectory(prefix="mosaico-layout-") as temporary:
            work = Path(temporary)
            before = flash.read(0, FLASH_BYTES, work / "before.bin")
            if before != original:
                raise ValueError("flash changed since backup; create and review a new backup/plan")
            # This is explicit development reprovisioning. No volume migration.
            flash.command("erase-region", hex(APP_AT), hex(FLASH_BYTES - APP_AT))
            flash.command("write-flash", hex(APP_AT), plan_dir / "application.bin")
            flash.command("write-flash", hex(SYSTEM_AT), plan_dir / "system.bin")
            flash.command("write-flash", hex(TABLE_AT), plan_dir / "partitions.bin")
            flash.command("erase-region", hex(OTA_AT), hex(OTA_BYTES))
            after = flash.read(0, FLASH_BYTES, work / "after.bin")
            guard_protected(original, after)
            if after[TABLE_AT:TABLE_AT + TABLE_BYTES] != table or after[APP_AT:APP_AT + len(image)] != image:
                raise ValueError("table/application readback mismatch; remain in download mode and restore")
            expected_tail = bytearray(b"\xff" * (FLASH_BYTES - APP_AT))
            expected_tail[:len(image)] = image
            expected_tail[SYSTEM_AT-APP_AT:] = system
            if after[OTA_AT:OTA_AT + OTA_BYTES] != b"\xff" * OTA_BYTES or after[APP_AT:] != expected_tail:
                raise ValueError("slot/system/otadata readback mismatch")
            receipt = dict(result="verified-write", mac=manifest["mac"], table_sha256=sha(table), image_sha256=sha(image),
                           protected_bytes="identical", device_boot_acceptance="not-run")
            write_json(directory / f"apply-{stamp}.json", receipt)
        if getattr(args, "boot", False):
            flash.boot()
    print("NOR writes verified. Check runtime boot/health if --boot was used; otherwise reset for boot acceptance.")


def restore(args):
    if not args.confirm_restore:
        raise ValueError("restore requires --confirm-restore; it replaces NOR contents with the backup")
    directory, manifest, original, _ = load_backup(args.backup)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    with Flash(args.port, args.baud, directory / f"restore-{stamp}.log") as flash:
        if flash.mac() != manifest["mac"]:
            raise ValueError("attached board is not the backed-up device")
        with tempfile.TemporaryDirectory(prefix="mosaico-restore-") as temporary:
            work = Path(temporary)
            before = flash.read(0, FLASH_BYTES, work / "before.bin")
            guard_protected(original, before)
            for name, start, size in [("application-system", APP_AT, FLASH_BYTES - APP_AT), ("otadata", OTA_AT, OTA_BYTES), ("partition-table", TABLE_AT, TABLE_BYTES)]:
                path = work / f"{name}.bin"
                path.write_bytes(original[start:start + size])
                flash.command("write-flash", hex(start), path)
            after = flash.read(0, FLASH_BYTES, work / "after.bin")
            if after != original:
                raise ValueError("restored flash does not match original raw backup")
            write_json(directory / f"restore-{stamp}.json", dict(result="verified-restore", flash_sha256=sha(after), mac=manifest["mac"], device_boot_acceptance="not-run"))
    print("NOR backup restored byte for byte; independent NAND is unchanged. Reset for boot acceptance.")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    subs = parser.add_subparsers(dest="command", required=True)
    p = subs.add_parser("backup"); p.add_argument("--out", required=True)
    p = subs.add_parser("plan"); p.add_argument("--backup", required=True); p.add_argument("--image", required=True); p.add_argument("--system", required=True); p.add_argument("--out", required=True)
    p = subs.add_parser("inspect"); p.add_argument("--backup", required=True)
    p = subs.add_parser("apply"); p.add_argument("--backup", required=True); p.add_argument("--plan", required=True); p.add_argument("--confirm-repartition", action="store_true")
    p.add_argument("--boot", action="store_true", help="watchdog-reset to the application only after complete readback passes")
    p = subs.add_parser("restore"); p.add_argument("--backup", required=True); p.add_argument("--confirm-restore", action="store_true")
    for name in ("backup", "apply", "restore"):
        subs.choices[name].add_argument("--port", required=True)
        subs.choices[name].add_argument("--baud", type=int, default=115200)
    args = parser.parse_args()
    try:
        if args.command == "inspect":
            print(json.dumps(load_backup(args.backup)[1], indent=2))
        else:
            globals()[args.command](args)
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"{args.command} failed: {error}\n")


if __name__ == "__main__":
    main()
