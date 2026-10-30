"""Release-boundary tests use synthetic images; no device or network access."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile
import zlib

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("firmware_release", ROOT / "scripts/release/package.py")
API = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(API)


def application(version="0.3.0", marker=True):
    header = bytearray(24)
    header[:4] = bytes([0xE9, 1, 2, 0x40])
    header[23] = 1
    struct.pack_into("<I", header, 4, 0x40000020)
    struct.pack_into("<H", header, 12, 32)
    segment = bytearray(512)
    struct.pack_into("<I", segment, 0, 0xABCD5432)
    segment[16:16 + len(version)] = version.encode()
    segment[48:59] = b"mosaico-s31"
    if marker:
        value = b"MOSAICO_S31_AB_LAYOUT_V2_6M_NAND\0"
        segment[256:256 + len(value)] = value
    data = header + struct.pack("<II", 0x40000020, len(segment)) + segment
    checksum = 0xEF
    for value in segment:
        checksum ^= value
    data.extend(b"\0" * ((len(data) // 16 + 1) * 16 - 1 - len(data)))
    data.append(checksum)
    return bytes(data) + hashlib.sha256(data).digest()


def encode_table(rows):
    data = b"".join(struct.pack("<HBBII16sI", 0x50AA, row["kind"], row["subtype"],
                                row["offset"], row["size"], row["name"].encode(), 0) for row in rows)
    data += b"\xeb\xeb" + b"\xff" * 14 + hashlib.md5(data).digest()
    return data.ljust(4096, b"\xff")


class ReleaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.table = API.partition_table()
        rows = API.layout.parse_table(cls.table)
        legacy = [dict(row) for row in rows if row["name"] not in ("ota_1", "system")]
        next(row for row in legacy if row["name"] == "main_app")["size"] = 0x7D0000
        legacy.extend([dict(name="ui_apps",kind=1,subtype=0x41,offset=0x9E0000,size=0x320000,flags=0),
                       dict(name="system",kind=1,subtype=0x83,offset=0xD00000,size=0x2EE000,flags=0)])
        cls.legacy_table = encode_table(legacy)
        raw = bytearray(b"\xff" * API.OFFICIAL["bytes"])
        raw[0x8000:0x9000] = cls.legacy_table
        boot = b"\xe9MOSAICO-BOOT-02!\0mosaico_boot_v2\0".ljust(128, b"\0")
        parts = {"bootloader": (0x2000, boot), "vibe_mode": (0x20000, b"public-rescue" * 8),
                 "ui_apps": (0x9E0000, b"public-ui" * 8)}
        cls.pins = {"version": "test", "bytes": len(raw), "components": {}}
        for name, (offset, payload) in parts.items():
            raw[offset:offset + len(payload)] = payload
            cls.pins["components"][name] = {"offset": offset, "bytes": len(payload), "sha256": API.sha(payload)}
        cls.official = bytes(raw)
        cls.pins["sha256"] = API.sha(raw)
        cls.app = application()
        cls.system = b"synthetic-system".ljust(API.SYSTEM_BYTES, b"\xff")
        cls.full, cls.components, cls.rows, cls.info = API.assemble(
            cls.official, cls.app, cls.system, cls.table, "0.3.0", cls.pins)

    def bundle(self, directory, *, full=None, mutate=None, skins=False, mutate_zip=None):
        full = self.full if full is None else full
        stem = "dangoOS-v0.3.0-esp32s31"
        manifest = {"schema": 1, "project": "dangoOS", "tag": "v0.3.0", "version": "0.3.0",
                    "source_commit": "1" * 40, "source_dirty": False, "chip": "esp32s31",
                    "flash_bytes": API.FLASH_BYTES, "layout": "MOSAICO_S31_AB_LAYOUT_V2_6M_NAND",
                    "upstream": self.pins, "partitions": self.rows, "components": copy.deepcopy(self.components),
                    "application": self.info,
                    "images": {"ota": {"file": stem + "-ota.bin", "bytes": len(self.app), "sha256": API.sha(self.app)},
                               "full": {"file": stem + "-full.bin", "bytes": len(full), "sha256": API.sha(full), "flash_offset": 0}}}
        resources = {}
        if skins:
            binary = b"synthetic independent NAND skin"
            metadata = {"schema_version": 1, "id": "eclipse", "name": "月蚀", "version": "test",
                        "canvas": [352, 336], "binary_bytes": len(binary), "binary_crc32": zlib.crc32(binary)}
            resources = {"dango-skin/eclipse/fox.bin": binary,
                         "dango-skin/eclipse/skin.json": API.json_bytes(metadata)}
            manifest["nand_skins"] = [{"id": "eclipse", "name": "月蚀", "version": "test", "files": {
                name: {"bytes": len(resources[f"dango-skin/eclipse/{name}"]),
                       "sha256": API.sha(resources[f"dango-skin/eclipse/{name}"])} for name in ("fox.bin", "skin.json")}}]
            resources.update({"install.py": b"# installer", "tools/console.py": b"# transport",
                              "tools/ota_image.py": b"# image validator", "requirements.txt": b"pyserial==3.5\n"})
        if mutate:
            mutate(manifest)
        files = {stem + "-ota.bin": self.app, stem + "-full.bin": full, "manifest.json": API.json_bytes(manifest),
                 "INSTALL.md": b"Synthetic install instructions\n"}
        for mode in ("ota", "full"):
            name = stem + f"-{mode}.bin"
            payload = {name: files[name], "manifest.json": files["manifest.json"], "INSTALL.md": files["INSTALL.md"], **resources}
            if mutate_zip:
                mutate_zip(mode, payload)
            payload["SHA256SUMS"] = API.checksums(payload)
            archive = directory / (stem + f"-{mode}.zip")
            API.write_zip(archive, payload)
            files[archive.name] = archive.read_bytes()
        files["SHA256SUMS"] = API.checksums(files)
        for name, payload in files.items():
            (directory / name).write_bytes(payload)

    def verify(self, directory):
        with patch.object(API, "OFFICIAL", self.pins), patch.object(API, "partition_table", return_value=self.table):
            return API.verify(directory)

    def test_public_base_requires_whole_image_and_component_hashes(self):
        data = bytearray(self.official)
        data[0x2002] ^= 1
        with self.assertRaisesRegex(ValueError, "base image"):
            API.official_components(data, self.pins)
        pins = copy.deepcopy(self.pins)
        pins["sha256"] = API.sha(data)
        with self.assertRaisesRegex(ValueError, "bootloader checksum"):
            API.official_components(data, pins)

    def test_full_install_selects_slot_zero_and_erases_user_regions(self):
        self.assertEqual(len(self.full), 16 * 1024 * 1024)
        self.assertEqual(self.full[0x210000:0x210000 + len(self.app)], self.app)
        self.assertEqual(struct.unpack_from("<I", self.full, 0x9000)[0], 1)
        self.assertEqual(struct.unpack_from("<I", self.full, 0x9018)[0], 2)
        self.assertEqual(self.full[0x901C:0x9020], bytes.fromhex("9a984347"))
        for start, end in [(0, 0x2000), (0xB000, 0x20000), (0x1E0000, 0x210000),
                           (0x810000, 0xE10000)]:
            self.assertEqual(self.full[start:end], b"\xff" * (end - start))

    def test_ota_rejects_wrong_version_chip_marker_and_corruption(self):
        with self.assertRaisesRegex(ValueError, "version"):
            API.checked_application(self.app, "0.3.1")
        with self.assertRaisesRegex(ValueError, "marker"):
            API.checked_application(application(marker=False), "0.3.0")
        for offset in [12, 350, len(self.app) - 1]:
            damaged = bytearray(self.app)
            damaged[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                API.checked_application(damaged, "0.3.0")

    def test_full_install_rejects_legacy_layout_and_wrong_system_size(self):
        with self.assertRaisesRegex(ValueError, "A/B contract"):
            API.assemble(self.official, self.app, self.system, self.legacy_table, "0.3.0", self.pins)
        with self.assertRaisesRegex(ValueError, "partition size"):
            API.assemble(self.official, self.app, self.system[:-1], self.table, "0.3.0", self.pins)

    def test_overlap_and_flash_overflow_are_rejected(self):
        for parts in [{"boot": (0x2000, b"1234"), "table": (0x2002, b"56")},
                      {"overflow": (API.FLASH_BYTES - 1, b"12")}]:
            with self.assertRaisesRegex(ValueError, "overlapping or out-of-range"):
                API.merge_parts(parts)

    def test_bundle_roundtrip_and_zip_reproducibility(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.bundle(directory)
            self.assertEqual(self.verify(directory)["version"], "0.3.0")
            archive = directory / "dangoOS-v0.3.0-esp32s31-full.zip"
            before = archive.read_bytes()
            self.bundle(directory)
            self.assertEqual(archive.read_bytes(), before)

    def test_nonblank_user_data_rejected_despite_updated_outer_checksums(self):
        data = bytearray(self.full)
        data[0x810000] = 0x42
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.bundle(directory, full=bytes(data))
            with self.assertRaisesRegex(ValueError, "non-erased gaps"):
                self.verify(directory)

    def test_ota_and_full_archives_both_include_checked_nand_skins(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.bundle(directory, skins=True)
            manifest = self.verify(directory)
            for mode in ("ota", "full"):
                with zipfile.ZipFile(directory / f"dangoOS-v0.3.0-esp32s31-{mode}.zip") as archive:
                    binary = archive.read("dango-skin/eclipse/fox.bin")
                    self.assertEqual(API.sha(binary), manifest["nand_skins"][0]["files"]["fox.bin"]["sha256"])
                    self.assertIn("install.py", archive.namelist())
                    self.assertIn("tools/console.py", archive.namelist())

    def test_nand_payload_and_tools_fail_verification_even_with_fresh_checksums(self):
        mutations = [
            (lambda payload: payload.pop("dango-skin/eclipse/fox.bin"), "missing or changed"),
            (lambda payload: payload.update({"dango-skin/eclipse/source.png": b"private"}), "inventory"),
            (lambda payload: payload.update({"dango-skin/eclipse/fox.bin": b"damaged"}), "missing or changed"),
            (lambda payload: payload.pop("install.py"), "tools missing"),
        ]
        for mode in ("ota", "full"):
            for mutation, message in mutations:
                with self.subTest(mode=mode, message=message), tempfile.TemporaryDirectory() as temporary:
                    directory = Path(temporary)
                    self.bundle(directory, skins=True, mutate_zip=lambda current, payload: mutation(payload) if current == mode else None)
                    with self.assertRaisesRegex(ValueError, message):
                        self.verify(directory)

    def test_nand_catalog_checks_metadata_after_hashes_match(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.bundle(directory, skins=True)
            manifest = json.loads((directory / "manifest.json").read_bytes())
            with zipfile.ZipFile(directory / "dangoOS-v0.3.0-esp32s31-ota.zip") as archive:
                resources = {name: archive.read(name) for name in archive.namelist() if name.startswith("dango-skin/")}
            path = "dango-skin/eclipse/skin.json"
            metadata = json.loads(resources[path])
            metadata["binary_crc32"] ^= 1
            resources[path] = API.json_bytes(metadata)
            manifest["nand_skins"][0]["files"]["skin.json"] = {"bytes": len(resources[path]), "sha256": API.sha(resources[path])}
            with self.assertRaisesRegex(ValueError, "manifest mismatch"):
                API.verify_nand_skins(manifest["nand_skins"], resources)

    def test_component_offset_and_dirty_manifest_are_rejected(self):
        mutations = [(lambda manifest: manifest["components"]["application"].update(offset=0), "component map"),
                     (lambda manifest: manifest.update(source_dirty=True), "dirty source")]
        for mutate, message in mutations:
            with self.subTest(message=message), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.bundle(directory, mutate=mutate)
                with self.assertRaisesRegex(ValueError, message):
                    self.verify(directory)

    def test_checksums_cover_all_files_and_reject_duplicates(self):
        for files in [{"a": b"payload", "SHA256SUMS": b""},
                      {"a": b"payload", "SHA256SUMS": API.checksums({"a": b"payload"}) * 2},
                      {"a": b"changed", "SHA256SUMS": API.checksums({"a": b"payload"})}]:
            with self.assertRaises(ValueError):
                API.verify_checksums(files)

    def test_release_inventory_rejects_missing_instructions_and_extra_files(self):
        for add_extra in (False, True):
            with self.subTest(add_extra=add_extra), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.bundle(directory)
                if add_extra:
                    (directory / "unexpected.bin").write_bytes(b"not a release asset")
                else:
                    (directory / "INSTALL.md").unlink()
                files = {path.name: path.read_bytes() for path in directory.iterdir() if path.name != "SHA256SUMS"}
                (directory / "SHA256SUMS").write_bytes(API.checksums(files))
                with self.assertRaisesRegex(ValueError, "asset inventory"):
                    self.verify(directory)


if __name__ == "__main__":
    unittest.main()
