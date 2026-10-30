"""Offline NOR simulation. No serial import or device access is used."""
import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import tempfile
import sys
import unittest
from unittest.mock import MagicMock, patch

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts/device"
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location("ota_layout", SCRIPTS / "layout.py")
API = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(API)


def metadata_only_image():
    """Small structural fixture, never a bootable or flashable application."""
    header=bytearray(24);header[0:4]=bytes([0xE9,1,2,0x40]);header[23]=1
    struct.pack_into("<I",header,4,0x40000020);struct.pack_into("<H",header,12,32)
    segment=bytearray(512);struct.pack_into("<I",segment,0,0xABCD5432)
    segment[16:21]=b"0.3.0";segment[48:59]=b"mosaico-s31"
    marker=b"MOSAICO_S31_AB_LAYOUT_V2_6M_NAND\0";segment[256:256+len(marker)]=marker
    raw=header+struct.pack("<II",0x40000020,len(segment))+segment
    checksum=0xEF
    for byte in segment:checksum^=byte
    raw.extend(b"\0"*((len(raw)//16+1)*16-1-len(raw)));raw.append(checksum)
    return bytes(raw)+hashlib.sha256(raw).digest()


def raw_fixture():
    rows = [("otadata",1,0,0x9000,0x2000), ("phy_init",1,1,0xB000,0x1000),
            ("sysmeta",1,2,0xC000,0x14000), ("vibe_mode",0,0x20,0x20000,0x1C0000),
            ("coredump",1,3,0x1E0000,0x20000), ("nvs",1,2,0x200000,0x6000),
            ("main_app",0,0x10,0x210000,0x7D0000), ("ui_apps",1,0x41,0x9E0000,0x320000),
            ("system",1,0x83,0xD00000,0x300000)]
    table=b"".join(struct.pack("<HBBII16sI",0x50AA,k,s,o,n,name.encode(),0) for name,k,s,o,n in rows)
    table+=b"\xeb\xeb"+b"\xff"*14+hashlib.md5(table).digest()
    table+=b"\xff"*(4096-len(table))
    raw=bytearray(b"\xff"*API.FLASH_BYTES)
    raw[0x2000]=0xE9;raw[0x2003]=0x40
    raw[0x8000:0x9000]=table
    raw[0x7E0000:0x7E0008]=b"old data"
    raw[0x20000:0x20008]=b"factory!"
    return bytes(raw)


class MemoryFlash:
    def __init__(self, raw):
        self.data=bytearray(raw);self.writes=[];self.reads=0;self.identity="aa:bb:cc:dd:ee:ff"
        self.fail_at=None;self.boots=0
    def __enter__(self): return self
    def __exit__(self, *_): pass
    def mac(self): return self.identity
    def boot(self): self.boots+=1
    def read(self, offset, length, path):
        self.reads+=1;value=bytes(self.data[offset:offset+length]);path.write_bytes(value);return value
    def command(self, command, offset, payload):
        self.writes.append((command,offset,str(payload)))
        if self.fail_at==len(self.writes): raise RuntimeError("simulated power interruption")
        offset=int(offset,0)
        if command=="write-flash":
            data=Path(payload).read_bytes();self.data[offset:offset+len(data)]=data
        elif command=="erase-region":
            length=int(payload,0);self.data[offset:offset+length]=b"\xff"*length
        else: raise AssertionError(command)


class LayoutTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name);self.backup=self.root/"backup";self.backup.mkdir()
        self.raw=raw_fixture();(self.backup/"flash.bin").write_bytes(self.raw)
        API.write_json(self.backup/"backup.json",dict(schema=1,flash_sha256=API.sha(self.raw),mac="aa:bb:cc:dd:ee:ff"))
        self.image=self.root/"structural-fixture.bin";self.image.write_bytes(metadata_only_image())
        self.system=self.root/"system.bin";self.system.write_bytes(b"synthetic-system".ljust(API.SYSTEM_BYTES,b"\xff"))
        self.plan=self.root/"plan"
        with contextlib.redirect_stdout(io.StringIO()),contextlib.redirect_stderr(io.StringIO()):
            API.plan(argparse.Namespace(backup=self.backup,image=self.image,system=self.system,out=self.plan))
        self.flash=MemoryFlash(self.raw)

    def apply(self, confirmed=True, boot=False):
        with patch.object(API,"Flash",return_value=self.flash),contextlib.redirect_stdout(io.StringIO()):
            API.apply(argparse.Namespace(backup=self.backup,plan=self.plan,port="fake",baud=115200,confirm_repartition=confirmed,boot=boot))

    def restore(self):
        with patch.object(API,"Flash",return_value=self.flash),contextlib.redirect_stdout(io.StringIO()):
            API.restore(argparse.Namespace(backup=self.backup,port="fake",baud=115200,confirm_restore=True))

    def test_generated_md5_layout_and_protected_records_round_trip(self):
        original=API.parse_table(self.raw[0x8000:0x9000])
        actual=API.parse_table((self.plan/"partitions.bin").read_bytes())
        self.assertEqual(actual,API.planned_rows(original))
        self.assertNotIn("data",[r["name"] for r in actual])
        self.assertEqual(API.named(actual,"system")["offset"],0xE10000)
        self.assertEqual(API.named(actual,"main_app")["size"],0x600000)
        self.assertEqual(API.named(actual,"ota_1")["offset"],0x810000)

    def test_unknown_prefix_partition_cannot_be_carried_into_the_new_layout(self):
        rows=API.parse_table(self.raw[0x8000:0x9000])
        rows.append(dict(name="extra",kind=1,subtype=0x40,offset=0x206000,size=0x1000,flags=0))
        with self.assertRaisesRegex(ValueError,"retained NOR prefix"):
            API.planned_rows(rows)

    def test_apply_then_restore_is_exact_and_never_writes_protected_bytes(self):
        self.apply()
        API.guard_protected(self.raw,self.flash.data)
        self.assertEqual(self.flash.data[0x810000:0xE10000],b"\xff"*0x600000)
        self.assertEqual([int(offset,0) for _,offset,_ in self.flash.writes],[0x210000,0x210000,0xE10000,0x8000,0x9000])
        self.restore()
        self.assertEqual(bytes(self.flash.data),self.raw)

    def test_unconfirmed_apply_and_wrong_device_do_not_write(self):
        with self.assertRaisesRegex(ValueError,"confirm-repartition"): self.apply(False)
        self.flash.identity="00:11:22:33:44:55"
        with self.assertRaisesRegex(ValueError,"backed-up device"): self.apply()
        self.assertFalse(self.flash.writes)

    def test_changed_flash_or_backup_refuses_before_writing(self):
        self.flash.data[0x7E0000]^=1
        with self.assertRaisesRegex(ValueError,"changed since backup"): self.apply()
        self.assertFalse(self.flash.writes)
        (self.backup/"flash.bin").write_bytes(self.raw[:-1]+b"\0")
        with self.assertRaisesRegex(ValueError,"hash changed"): self.apply()
        self.assertFalse(self.flash.writes)

    def test_mutated_plan_or_image_is_rejected(self):
        data=bytearray((self.plan/"partitions.bin").read_bytes());data[20]^=1
        (self.plan/"partitions.bin").write_bytes(data)
        with self.assertRaises(ValueError):self.apply()
        self.assertFalse(self.flash.writes)

    def test_interrupted_migration_can_be_restored(self):
        for failure in range(1,6):
            self.flash=MemoryFlash(self.raw);self.flash.fail_at=failure
            with self.assertRaisesRegex(RuntimeError,"interruption"):self.apply()
            self.assertFalse(list(self.backup.glob("apply-*.json")))
            self.flash.fail_at=None;self.restore()
            self.assertEqual(bytes(self.flash.data),self.raw)

    def test_optional_boot_requires_complete_verified_write(self):
        self.flash.fail_at=3
        with self.assertRaisesRegex(RuntimeError,"interruption"):self.apply(boot=True)
        self.assertEqual(self.flash.boots,0)
        self.flash=MemoryFlash(self.raw)
        self.apply(boot=True)
        self.assertEqual(self.flash.boots,1)

    def test_esptool_connection_survives_all_operations_and_closes_on_failure(self):
        tool=MagicMock()
        esp=tool.connect_esp.return_value
        tool.run_stub.return_value=esp
        esp.read_mac.return_value=(0xaa,0xbb,0xcc,0xdd,0xee,0xff)
        def read(_esp, at, size, path, **_):
            self.assertEqual(esp.__exit__.call_count,0)
            Path(path).write_bytes(self.raw[at:at+size])
        tool.read_flash.side_effect=read
        with patch.dict(sys.modules,esptool=tool):
            with self.assertRaisesRegex(RuntimeError,"esptool failed"):
                with API.Flash("fake",921600,self.root/"transport.log") as flash:
                    self.assertEqual(flash.mac(),"aa:bb:cc:dd:ee:ff")
                    self.assertEqual(flash.read(0,4096,self.root/"read.bin"),self.raw[:4096])
                    flash.command("write-flash","0x210000",self.image)
                    tool.read_flash.side_effect=OSError("device lost")
                    flash.read(0,4096,self.root/"lost.bin")
        tool.connect_esp.assert_called_once()
        tool.run_stub.assert_called_once()
        esp.__exit__.assert_called_once()

    def test_restore_confirmation_and_protected_bytes_are_required(self):
        with patch.object(API,"Flash",return_value=self.flash):
            with self.assertRaisesRegex(ValueError,"confirm-restore"):
                API.restore(argparse.Namespace(backup=self.backup,port="fake",baud=115200,confirm_restore=False))
        self.flash.data[0x20000]^=1
        with self.assertRaisesRegex(ValueError,"protected flash bytes"):
            self.restore()
        self.assertFalse(self.flash.writes)

    def test_old_layout_image_cannot_be_used_to_prepare_migration(self):
        image=bytearray(metadata_only_image())
        marker=b"MOSAICO_S31_AB_LAYOUT_V2_6M_NAND\0";at=image.index(marker)
        image[at:at+len(marker)]=b"\0"*len(marker)
        checksum=0xEF
        for byte in image[32:544]:checksum^=byte
        image[-33]=checksum;image[-32:]=hashlib.sha256(image[:-32]).digest()
        self.image.write_bytes(image)
        with self.assertRaisesRegex(ValueError,"A/B layout compatibility"):
            API.plan(argparse.Namespace(backup=self.backup,image=self.image,system=self.system,out=self.root/"rejected-plan"))
        self.assertFalse((self.root/"rejected-plan").exists())

    def test_unknown_corrupt_or_overlapping_tables_are_rejected(self):
        original=self.raw[0x8000:0x9000]
        for position,value in [(0,0),(20,64),(28,1)]:
            table=bytearray(original);table[position]=value
            with self.assertRaises(ValueError):API.parse_table(table)
        rows=API.parse_table(original);rows[6]["size"]=0x300000
        with self.assertRaisesRegex(ValueError,"legacy"):API.require_legacy(rows)

    def test_otadata_crc_matches_pinned_idf_fixture(self):
        raw=bytearray(b"\xff"*8192);struct.pack_into("<I",raw,0,1);struct.pack_into("<I",raw,24,2)
        raw[28:32]=bytes.fromhex("9a984347")
        self.assertTrue(API.inspect_otadata(raw)[0]["crc_ok"])
        raw[0]=2;self.assertFalse(API.inspect_otadata(raw)[0]["crc_ok"])


if __name__=="__main__":unittest.main()
