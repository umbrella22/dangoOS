#!/usr/bin/env python3
"""S31ONE03 NOR layout and hash-bound ROM reprovisioning. NAND is untouched."""
import argparse
from datetime import datetime, timezone
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import zlib
from main_image import read_image, validate_image

FLASH_BYTES = 0x1000000
TABLE_AT, TABLE_BYTES = 0x8000, 0x1000
JOURNAL_AT, JOURNAL_BYTES = 0x9000, 0x2000
RECOVERY_AT, RECOVERY_BYTES = 0x20000, 0x1c0000
APP_AT, MAIN_BYTES = 0x210000, 0xc00000
SYSTEM_AT, SYSTEM_BYTES = 0xe10000, 0x1f0000
LAYOUT = "S31ONE03"
EXPECTED = [
    ("boot_state", 1, 0x40, JOURNAL_AT, JOURNAL_BYTES),
    ("phy_init", 1, 1, 0xb000, 0x1000),
    ("sysmeta", 1, 2, 0xc000, 0x14000),
    ("recovery_app", 0, 0, RECOVERY_AT, RECOVERY_BYTES),
    ("coredump", 1, 3, 0x1e0000, 0x20000),
    ("nvs", 1, 2, 0x200000, 0x6000),
    ("main_app", 0, 0x10, APP_AT, MAIN_BYTES),
    ("system", 1, 0x83, SYSTEM_AT, SYSTEM_BYTES),
]
GENERATOR = Path(__file__).resolve().parent / "vendor/gen_esp32part.py"
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


def planned_rows():
    return [dict(name=n, kind=k, subtype=s, offset=a, size=b, flags=0) for n,k,s,a,b in EXPECTED]


def require_layout(rows):
    if rows != planned_rows():
        raise ValueError("partition table differs from S31ONE03")


def boot_records(image):
    info = validate_image(image)
    body = bytearray(96)
    body[:16] = b"DGBOOT3\0S31ONE03"
    struct.pack_into("<QII", body, 16, 0, 2, info["bytes"])  # generation 0, Trial
    body[32:64] = bytes.fromhex(info["sha256"])
    struct.pack_into("<I", body, 92, zlib.crc32(body[:92]))
    return (bytes(body) + b"DONE").ljust(JOURNAL_BYTES, b"\xff")


def inspect_journal(raw):
    if len(raw) != JOURNAL_BYTES:
        raise ValueError("journal requires two sectors")
    records = []
    for index in range(2):
        b = raw[index*4096:index*4096+100]
        if (b[:16] != b"DGBOOT3\0S31ONE03" or b[96:] != b"DONE" or any(b[64:92])
                or zlib.crc32(b[:92]) != struct.unpack_from("<I", b, 92)[0]):
            continue
        generation,state,length = struct.unpack_from("<QII", b, 16)
        if state > 5 or (state != 0 and not 0 < length <= MAIN_BYTES):
            continue
        records.append(dict(index=index,generation=generation,state=state,image_bytes=length,sha256=b[32:64].hex()))
    if len(records)==2 and records[0]["generation"]==records[1]["generation"]:
        if {k:v for k,v in records[0].items() if k!="index"}!={k:v for k,v in records[1].items() if k!="index"}:
            raise ValueError("conflicting boot journal generations")
    return records


def check_full(raw):
    if len(raw) != FLASH_BYTES:
        raise ValueError("full NOR image must be exactly 16 MiB")
    require_layout(parse_table(raw[TABLE_AT:TABLE_AT+TABLE_BYTES]))
    records = inspect_journal(raw[JOURNAL_AT:JOURNAL_AT+JOURNAL_BYTES])
    if len(records)!=1 or records[0]["state"]!=2:
        raise ValueError("full image must start with one Trial record")
    record=records[0]
    image=raw[APP_AT:APP_AT+record["image_bytes"]]
    info=validate_image(image)
    if info["sha256"]!=record["sha256"] or raw[0x2000]!=0xe9 or raw[RECOVERY_AT]!=0xe9:
        raise ValueError("full image boot/recovery/main integrity mismatch")
    return info


def load_backup(directory):
    directory=Path(directory).resolve()
    manifest=json.loads((directory/"backup.json").read_text())
    raw=(directory/"flash.bin").read_bytes()
    if manifest.get("schema")!=2 or len(raw)!=FLASH_BYTES or sha(raw)!=manifest["flash_sha256"]:
        raise ValueError("backup is incomplete or its raw flash hash changed")
    if not re.fullmatch(r"[0-9a-f]{2}(:[0-9a-f]{2}){5}",manifest.get("mac","")):
        raise ValueError("backup device identity missing")
    return directory,manifest,raw


class Flash:
    """Keep one ROM/stub connection alive through backup, write and readback."""
    def __init__(self,port,baud,log):
        self.port,self.baud,self.log=port,baud,log
        self.esp=None
    def __enter__(self):
        import esptool
        with self.log.open("a") as out,contextlib.redirect_stdout(out),contextlib.redirect_stderr(out):
            self.esp=esptool.cmds.detect_chip(self.port,connect_mode="no-reset")
            if self.esp.CHIP_NAME!="ESP32-S31" or self.esp.get_secure_boot_enabled() or self.esp.get_flash_encryption_enabled():
                raise ValueError("ROM target must be an unencrypted ESP32-S31")
            # Software entry uses USB OTG. esptool's USB Serial/JTAG watchdog
            # workaround does not cover this transport. These registers are
            # volatile; the force-download bit remains set until boot().
            self.esp.write_reg(self.esp.RTC_CNTL_WDTWPROTECT_REG,self.esp.RTC_CNTL_WDT_WKEY)
            self.esp.write_reg(self.esp.RTC_CNTL_WDTCONFIG0_REG,0)
            self.esp.write_reg(self.esp.RTC_CNTL_WDTWPROTECT_REG,0)
            self.esp=esptool.run_stub(self.esp)
            esptool.attach_flash(self.esp)
            if 1<<((self.esp.flash_id()>>16)&255)!=FLASH_BYTES:
                raise ValueError("NOR capacity must be 16 MiB")
            self.esp.flash_set_parameters(FLASH_BYTES)
        return self
    def __exit__(self,*_):
        if self.esp is not None:self.esp._port.close()
    def mac(self):
        return ":".join(f"{b:02x}" for b in self.esp.read_mac())
    def read(self,offset,length,path):
        data=self.esp.read_flash(offset,length)
        if len(data)!=length:raise ValueError("incomplete raw NOR read")
        path.write_bytes(data)
        return data
    def write(self,offset,data):
        import esptool
        with self.log.open("a") as out,contextlib.redirect_stdout(out),contextlib.redirect_stderr(out):
            esptool.write_flash(self.esp,[(offset,data)],flash_size="16MB",compress=True,no_progress=True)
    def boot(self):
        # Clear only the volatile software-ROM request immediately before reset.
        self.esp.write_reg(0x20700008,0,4)
        self.esp.hard_reset()


def backup(args):
    directory=Path(args.out).expanduser().resolve()
    directory.mkdir(parents=True,exist_ok=False,mode=0o700)
    with Flash(args.port,args.baud,directory/"backup.log") as flash:
        mac=flash.mac()
        raw=flash.read(0,FLASH_BYTES,directory/"flash.bin")
        if flash.read(0,0x20000,directory/"boot-region.bin")!=raw[:0x20000]:
            raise ValueError("independent boot-region reread differs")
        manifest=dict(schema=2,captured_at=datetime.now(timezone.utc).isoformat(),mac=mac,flash_sha256=sha(raw),nand="not accessed")
        write_json(directory/"backup.json",manifest)
    print(json.dumps(manifest,indent=2))


def plan(args):
    directory,manifest,raw=load_backup(args.backup)
    full=Path(args.full).read_bytes();info=check_full(full)
    output=Path(args.out).resolve();output.mkdir(parents=True,exist_ok=False)
    (output/"full.bin").write_bytes(full)
    receipt=dict(schema=2,backup=str(directory),backup_sha256=sha(raw),mac=manifest["mac"],full_sha256=sha(full),layout=LAYOUT,image=info,nand="not accessed",physical_status="not-run")
    write_json(output/"plan.json",receipt)
    print(json.dumps(receipt,indent=2))


def checked_plan(directory,manifest):
    directory=Path(directory).resolve();receipt=json.loads((directory/"plan.json").read_text())
    full=(directory/"full.bin").read_bytes()
    if (receipt.get("schema")!=2 or receipt["backup_sha256"]!=manifest["flash_sha256"]
            or receipt["mac"]!=manifest["mac"] or receipt["full_sha256"]!=sha(full) or receipt["image"]!=check_full(full)):
        raise ValueError("plan artifacts or bound backup/device changed")
    return directory,full


def apply(args):
    if not args.confirm_repartition:raise ValueError("apply requires --confirm-repartition")
    directory,manifest,original=load_backup(args.backup)
    plan_dir,full=checked_plan(args.plan,manifest)
    with Flash(args.port,args.baud,directory/"apply.log") as flash:
        if flash.mac()!=manifest["mac"]:raise ValueError("attached board is not the backed-up device")
        before=flash.read(0,FLASH_BYTES,directory/"before-apply.bin")
        if before!=original:raise ValueError("NOR changed since backup; no writes performed")
        flash.write(0,full)
        after=flash.read(0,FLASH_BYTES,directory/"after-apply.bin")
        if after!=full:raise ValueError("full NOR readback mismatch; remain in ROM")
        write_json(directory/"apply.json",dict(result="verified-write",mac=manifest["mac"],full_sha256=sha(after),nand="not accessed",device_boot_acceptance="not-run"))
        if args.boot:flash.boot()


def restore(args):
    if not args.confirm_restore:raise ValueError("restore requires --confirm-restore")
    directory,manifest,original=load_backup(args.backup)
    with Flash(args.port,args.baud,directory/"restore.log") as flash:
        if flash.mac()!=manifest["mac"]:raise ValueError("attached board is not the backed-up device")
        flash.write(0,original)
        if flash.read(0,FLASH_BYTES,directory/"after-restore.bin")!=original:raise ValueError("NOR restore readback mismatch")
        write_json(directory/"restore.json",dict(result="verified-restore",mac=manifest["mac"],nand="not accessed"))


def main():
    os.umask(0o077)
    parser=argparse.ArgumentParser(description=__doc__);subs=parser.add_subparsers(dest="command",required=True)
    p=subs.add_parser("backup");p.add_argument("--out",required=True)
    p=subs.add_parser("plan");p.add_argument("--backup",required=True);p.add_argument("--full",required=True);p.add_argument("--out",required=True)
    p=subs.add_parser("inspect");p.add_argument("--backup",required=True)
    p=subs.add_parser("apply");p.add_argument("--backup",required=True);p.add_argument("--plan",required=True);p.add_argument("--confirm-repartition",action="store_true");p.add_argument("--boot",action="store_true")
    p=subs.add_parser("restore");p.add_argument("--backup",required=True);p.add_argument("--confirm-restore",action="store_true")
    for name in ("backup","apply","restore"):
        subs.choices[name].add_argument("--port",required=True);subs.choices[name].add_argument("--baud",type=int,default=115200)
    args=parser.parse_args()
    try:
        if args.command=="inspect":print(json.dumps(load_backup(args.backup)[1],indent=2))
        else:globals()[args.command](args)
    except (ValueError,OSError,RuntimeError,KeyError) as error:parser.exit(2,f"{args.command} failed: {error}\n")


if __name__=="__main__":main()
