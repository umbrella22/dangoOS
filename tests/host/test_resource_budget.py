"""Resource gates use a generated application and ELF, without device access."""
import copy
import importlib.util
from pathlib import Path
import struct
import unittest

from test_release import application

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("resource_budget", ROOT / "scripts/check/resource_budget.py")
API = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(API)


def elf(bss=4096, stack=48000):
    names = b"\0.names\0.bss\0.stack\0"
    offset = (52 + len(names) + 3) & ~3
    rows = [bytes(40), struct.pack("<10I", 1, 3, 0, 0, 52, len(names), 0, 0, 1, 0),
            struct.pack("<10I", 8, 8, 3, 0x2F000000, 0, bss, 0, 0, 4, 0),
            struct.pack("<10I", 13, 8, 3, 0x2F000000 + bss, 0, stack, 0, 0, 4, 0)]
    header = struct.pack("<16sHHIIIIIHHHHHH", b"\x7fELF\x01\x01\x01".ljust(16, b"\0"),
                         2, 243, 1, 0x40000020, 0, offset, 0, 52, 0, 0, 40, len(rows), 1)
    return (header + names).ljust(offset, b"\0") + b"".join(rows)


class ResourceBudgetTests(unittest.TestCase):
    def setUp(self):
        self.image = application()
        self.policy = {
            "baseline_commit": "1" * 40, "application_slot_bytes": 6291456,
            "minimum_slot_headroom_bytes": 524288, "minimum_main_stack_bytes": 32768,
            "baseline": {"image_bytes": len(self.image), "internal_allocated_sections_bytes": 4096,
                         "rtc_allocated_sections_bytes": 0, "main_stack_bytes": 48000},
            "maximum_growth_bytes": {"image_bytes": 65536, "internal_allocated_sections_bytes": 256},
        }

    def test_static_growth_is_visible_even_when_the_linker_shrinks_the_stack(self):
        good = API.assess(elf(), self.image, self.policy)
        changed = API.assess(elf(bss=4608, stack=47488), self.image, self.policy)
        self.assertEqual(good["status"], "pass")
        self.assertEqual(changed["status"], "fail")
        self.assertEqual(changed["growth"]["internal_allocated_sections_bytes"], 512)
        self.assertIsNone(changed["runtime_measurements"])

    def test_slot_headroom_and_stack_reserve_are_independent_gates(self):
        policy = copy.deepcopy(self.policy)
        policy["minimum_slot_headroom_bytes"] = 6291456 - len(self.image) + 1
        self.assertIn("slot headroom", API.assess(elf(), self.image, policy)["failures"][0])
        report = API.assess(elf(stack=32764), self.image, self.policy)
        self.assertTrue(any("main stack" in message for message in report["failures"]))

    def test_nobits_sections_count_and_malformed_or_wrong_target_elf_is_rejected(self):
        self.assertEqual(API.elf_usage(elf())["internal_allocated_sections_bytes"], 4096)
        for data in (elf()[:100], b"\0" * 52):
            with self.assertRaises(ValueError):
                API.elf_usage(data)
        data = bytearray(elf())
        struct.pack_into("<H", data, 18, 62)
        with self.assertRaisesRegex(ValueError, "RISC-V"):
            API.elf_usage(data)

    def test_budget_never_accepts_an_invalid_or_old_layout_ota_image(self):
        for image in (self.image[:-1], application(marker=False)):
            with self.assertRaises(ValueError):
                API.assess(elf(), image, self.policy)
        policy = copy.deepcopy(self.policy)
        policy["application_slot_bytes"] *= 2
        with self.assertRaisesRegex(ValueError, "slot differs"):
            API.assess(elf(), self.image, policy)


if __name__ == "__main__":
    unittest.main()
