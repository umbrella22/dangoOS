"""S31ONE03 full-image planning and journal integrity without device access."""
import copy
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zlib
from types import SimpleNamespace
import test_release as fixture

release = fixture.API

API = release.layout


class BootLayoutTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fixture.ReleaseTests.setUpClass()
        cls.full = fixture.ReleaseTests.full
        cls.mac = '1c:29:04:d0:a7:4a'

    def test_fixed_geometry_has_one_12m_main_and_independent_recovery(self):
        rows = API.parse_table(release.partition_table())
        API.require_layout(rows)
        apps = [r for r in rows if r['kind'] == 0]
        self.assertEqual([(r['name'], r['size']) for r in apps], [('recovery_app', 0x1c0000), ('main_app', 0xc00000)])
        self.assertEqual(API.check_full(self.full)['sha256'], API.sha(fixture.ReleaseTests.app))
        rows[-2]['size'] = 0x600000
        with self.assertRaises(ValueError): API.require_layout(rows)

    def test_journal_rejects_torn_checksum_and_conflicting_same_generation(self):
        raw = bytearray(self.full[0x9000:0xb000])
        for at in (0,8,24,32,92,96):
            broken = raw.copy(); broken[at] ^= 1
            self.assertEqual(API.inspect_journal(broken), [])
        raw[4096:4196] = raw[:100]
        struct.pack_into('<I', raw, 4096+24, 4)
        struct.pack_into('<I', raw, 4096+92, zlib.crc32(raw[4096:4096+92]))
        with self.assertRaisesRegex(ValueError, 'conflicting'): API.inspect_journal(raw)

    def test_matching_transport_hash_cannot_legitimize_a_bad_main(self):
        broken = bytearray(self.full)
        broken[API.APP_AT+350] ^= 1
        with self.assertRaises(ValueError): API.check_full(broken)

    def prepare(self, root):
        backup=root/'backup';backup.mkdir()
        original=bytes([0x5a])*API.FLASH_BYTES
        (backup/'flash.bin').write_bytes(original)
        manifest=dict(schema=2, mac=self.mac, flash_sha256=API.sha(original))
        API.write_json(backup/'backup.json',manifest)
        full=root/'image.bin';full.write_bytes(self.full)
        plan=root/'plan'
        API.plan(SimpleNamespace(backup=backup,full=full,out=plan))
        return backup,plan,manifest,original

    def test_plan_is_bound_to_complete_backup_device_and_image(self):
        with tempfile.TemporaryDirectory() as d:
            backup,plan,manifest,original=self.prepare(Path(d))
            self.assertEqual(API.checked_plan(plan,manifest)[1],self.full)
            changed=copy.deepcopy(manifest);changed['mac']='00:11:22:33:44:55'
            with self.assertRaises(ValueError): API.checked_plan(plan,changed)
            (plan/'full.bin').write_bytes(self.full[:-1])
            with self.assertRaises(ValueError): API.checked_plan(plan,manifest)

    def test_apply_never_writes_a_changed_medium_or_different_device(self):
        with tempfile.TemporaryDirectory() as d:
            backup,plan,manifest,original=self.prepare(Path(d))
            args=SimpleNamespace(backup=backup,plan=plan,port='fake',baud=115200,confirm_repartition=True,boot=False)
            for wrong_mac in (True,False):
                class Fake:
                    writes=[]
                    def __init__(self,*args):pass
                    def __enter__(self):return self
                    def __exit__(self,*args):pass
                    def mac(self):return '00:11:22:33:44:55' if wrong_mac else manifest['mac']
                    def read(self,*args):return bytes(len(original))
                    def write(self,*args):self.writes.append(args)
                with patch.object(API,'Flash',Fake),self.assertRaises(ValueError):API.apply(args)
                self.assertEqual(Fake.writes,[])


if __name__ == '__main__': unittest.main()
