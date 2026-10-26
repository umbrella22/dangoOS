#!/usr/bin/env python3
"""Prepare and verify the attended ESP32-S31 A/B layout migration.

backup reads raw flash into a new directory outside the repository.
plan is offline. apply and restore require an explicit confirmation switch,
an intact backup and the same hardware MAC; they leave the board in ROM mode.
"""
import argparse
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
SLOT_BYTES = 0x300000
DATA_AT = 0x810000
DATA_BYTES = 0x1D0000
PROTECTED = ((0, 0x8000), (0xB000, 0x210000), (0x9E0000, FLASH_BYTES))
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


def planned_rows(original):
    require_legacy(original)
    result = []
    for row in original:
        if row["name"] != "main_app":
            result.append(dict(row))
            continue
        result.append(dict(row, size=SLOT_BYTES))
        result.append(dict(name="ota_1", kind=0, subtype=0x11, offset=0x510000, size=SLOT_BYTES, flags=0))
        result.append(dict(name="data", kind=1, subtype=0x83, offset=DATA_AT, size=DATA_BYTES, flags=0))
    return result


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
    require_legacy(rows)
    if raw[0x2000] != 0xE9 or (raw[0x2003] >> 4) != 4:
        raise ValueError("backup bootloader does not identify unencrypted 16 MiB flash")
    return directory, manifest, raw, rows


class Flash:
    def __init__(self, port, baud, log):
        self.base = ["esptool", "--chip", "esp32s31", "--port", port, "--baud", str(baud), "--before", "no-reset", "--after", "no-reset"]
        self.log = log

    def command(self, *args):
        command = self.base + [str(arg) for arg in args]
        run = subprocess.run(command, capture_output=True, text=True)
        with self.log.open("a", encoding="utf-8") as output:
            output.write(json.dumps(command) + "\n" + run.stdout + run.stderr + "\n")
        if run.returncode:
            raise RuntimeError(f"esptool failed ({run.returncode}); see {self.log}")
        return run.stdout

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


def backup(args):
    directory = Path(args.out).expanduser().resolve()
    # The device tools live two directories below the repository root; a raw backup must survive removal of this repo.
    repo = Path(__file__).resolve().parents[2]
    if directory == repo or repo in directory.parents:
        raise ValueError("choose a backup directory outside the repository")
    directory.mkdir(parents=True, exist_ok=False)
    flash = Flash(args.port, args.baud, directory / "backup.log")
    mac = flash.mac()
    raw = flash.read(0, FLASH_BYTES, directory / "flash.bin")
    rows = parse_table(raw[TABLE_AT:TABLE_AT + TABLE_BYTES])
    require_legacy(rows)
    # A second independent read verifies the boot/table/otadata recovery range.
    boot = flash.read(0, 0x20000, directory / "boot-region.bin")
    if boot != raw[:0x20000]:
        raise ValueError("boot-region reread differs; backup cannot be used")
    (directory / "legacy-data.bin").write_bytes(raw[OLD_DATA_AT:OLD_DATA_AT + OLD_DATA_BYTES])
    manifest = dict(schema=1, captured_at=datetime.now(timezone.utc).isoformat(), mac=mac,
                    flash_sha256=sha(raw), boot_region_sha256=sha(boot),
                    legacy_data_sha256=sha(raw[OLD_DATA_AT:OLD_DATA_AT + OLD_DATA_BYTES]),
                    partitions=rows, otadata=inspect_otadata(raw[OTA_AT:OTA_AT + OTA_BYTES]),
                    bootloader_evidence=bootloader_evidence(raw))
    write_json(directory / "backup.json", manifest)
    load_backup(directory)
    print(json.dumps(manifest, indent=2))


def plan(args):
    directory, manifest, raw, rows = load_backup(args.backup)
    image, image_info = read_image(args.image)
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
        if row["name"] in old_records and row["name"] != "main_app" and generated[i * 32:(i + 1) * 32] != old_records[row["name"]]:
            raise ValueError("generator changed a protected partition record")
    (output / "application.bin").write_bytes(image)
    receipt = dict(schema=1, backup=str(directory), backup_sha256=manifest["flash_sha256"], mac=manifest["mac"],
                   table_sha256=sha(generated), image=image_info, partitions=result,
                   destructive_ranges=["main_app image at 0x210000", "new data 0x810000..0x9E0000 (old data is not migrated)", "partition table 0x8000..0x9000", "otadata 0x9000..0xB000"],
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
    return directory, table, image


def guard_protected(before, after):
    for lo, hi in PROTECTED:
        if before[lo:hi] != after[lo:hi]:
            raise ValueError(f"protected flash bytes changed at 0x{lo:x}..0x{hi:x}")


def apply(args):
    if not args.confirm_repartition:
        raise ValueError("apply requires --confirm-repartition after attended review of plan.json")
    directory, manifest, original, rows = load_backup(args.backup)
    plan_dir, table, image = checked_plan(args.plan, manifest, rows)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    flash = Flash(args.port, args.baud, directory / f"apply-{stamp}.log")
    if flash.mac() != manifest["mac"]:
        raise ValueError("attached board is not the backed-up device")
    with tempfile.TemporaryDirectory(prefix="mosaico-layout-") as temporary:
        work = Path(temporary)
        before = flash.read(0, FLASH_BYTES, work / "before.bin")
        if before != original:
            raise ValueError("flash changed since backup; create and review a new backup/plan")
        # The compatible application is present before changing its partition map.
        flash.command("write-flash", hex(APP_AT), plan_dir / "application.bin")
        flash.command("erase-region", hex(DATA_AT), hex(DATA_BYTES))
        flash.command("write-flash", hex(TABLE_AT), plan_dir / "partitions.bin")
        flash.command("erase-region", hex(OTA_AT), hex(OTA_BYTES))
        after = flash.read(0, FLASH_BYTES, work / "after.bin")
        guard_protected(original, after)
        if after[TABLE_AT:TABLE_AT + TABLE_BYTES] != table or after[APP_AT:APP_AT + len(image)] != image:
            raise ValueError("table/application readback mismatch; remain in download mode and restore")
        if after[OTA_AT:OTA_AT + OTA_BYTES] != b"\xff" * OTA_BYTES or after[DATA_AT:DATA_AT + DATA_BYTES] != b"\xff" * DATA_BYTES:
            raise ValueError("otadata/data erase readback mismatch")
        receipt = dict(result="verified-write", mac=manifest["mac"], table_sha256=sha(table), image_sha256=sha(image),
                       protected_bytes="identical", device_boot_acceptance="not-run")
        write_json(directory / f"apply-{stamp}.json", receipt)
    print("Migration writes verified; board remains in download mode. Reset for attended boot acceptance.")


def restore(args):
    if not args.confirm_restore:
        raise ValueError("restore requires --confirm-restore; it replaces firmware and all app data with the backup")
    directory, manifest, original, _ = load_backup(args.backup)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    flash = Flash(args.port, args.baud, directory / f"restore-{stamp}.log")
    if flash.mac() != manifest["mac"]:
        raise ValueError("attached board is not the backed-up device")
    with tempfile.TemporaryDirectory(prefix="mosaico-restore-") as temporary:
        work = Path(temporary)
        before = flash.read(0, FLASH_BYTES, work / "before.bin")
        guard_protected(original, before)
        for name, start, size in [("legacy-main-data", APP_AT, OLD_APP_BYTES), ("otadata", OTA_AT, OTA_BYTES), ("partition-table", TABLE_AT, TABLE_BYTES)]:
            path = work / f"{name}.bin"
            path.write_bytes(original[start:start + size])
            flash.command("write-flash", hex(start), path)
        after = flash.read(0, FLASH_BYTES, work / "after.bin")
        if after != original:
            raise ValueError("restored flash does not match original raw backup")
        write_json(directory / f"restore-{stamp}.json", dict(result="verified-restore", flash_sha256=sha(after), mac=manifest["mac"], device_boot_acceptance="not-run"))
    print("Original firmware, layout and data restored byte for byte. Reset for attended boot acceptance.")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    subs = parser.add_subparsers(dest="command", required=True)
    p = subs.add_parser("backup"); p.add_argument("--out", required=True)
    p = subs.add_parser("plan"); p.add_argument("--backup", required=True); p.add_argument("--image", required=True); p.add_argument("--out", required=True)
    p = subs.add_parser("inspect"); p.add_argument("--backup", required=True)
    p = subs.add_parser("apply"); p.add_argument("--backup", required=True); p.add_argument("--plan", required=True); p.add_argument("--confirm-repartition", action="store_true")
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
