"""Only synthetic image bytes; no device or serial port is opened."""
import hashlib
import importlib.util
from pathlib import Path
import struct
import unittest

SPEC = importlib.util.spec_from_file_location("main_image", Path(__file__).resolve().parents[2] / "tools/device/main_image.py")
image = importlib.util.module_from_spec(SPEC); SPEC.loader.exec_module(image)


def fixture(marker=image.MARKER, project=b"mosaico-s31", chip=32):
    header=bytearray(24);header[:4]=b"\xe9\x01\x02\x40";header[23]=1
    struct.pack_into("<IH",header,4,0x40000020,0);struct.pack_into("<H",header,12,chip)
    segment=bytearray(9000);struct.pack_into("<I",segment,0,0xabcd5432)
    segment[16:21]=b"0.4.4";segment[48:48+len(project)]=project
    segment[512:512+len(marker)]=marker
    result=header+struct.pack("<II",0x40000020,len(segment))+segment
    result.extend(bytes((len(result)//16+1)*16-1-len(result)))
    result.append(0xef ^ __import__('functools').reduce(int.__xor__,segment,0))
    result.extend(hashlib.sha256(result).digest());return bytes(result)


class MainImageTests(unittest.TestCase):
    def test_new_contract_binds_chip_project_layout_and_both_integrity_checks(self):
        data=fixture();info=image.validate_image(data)
        self.assertEqual(info["layout"],"S31ONE03")
        self.assertEqual(info["bytes"],len(data))
        self.assertEqual(info["sha256"],hashlib.sha256(data).hexdigest())
        for invalid in (fixture(chip=9),fixture(project=b"dango-recovery"),
                        fixture(marker=b"MOSAICO_S31_AB_LAYOUT_V2_6M_NAND\0")):
            with self.assertRaises(ValueError): image.validate_image(invalid)
        for offset in (24,40,1000,len(data)-33,len(data)-1):
            broken=bytearray(data);broken[offset]^=1
            with self.assertRaises(ValueError): image.validate_image(broken)
        with self.assertRaises(ValueError): image.validate_image(data+b"trailing")


if __name__=="__main__":unittest.main()
