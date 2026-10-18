#!/usr/bin/env python3
# INPUT:  linux_abi.py/deploy-select.py and synthetic ELF64 GNU version tables
# OUTPUT:  ABI-floor, weak-node, malformed-file and pre-copy deployment regression results
# POS:    protects handheld compatibility without needing a newer/older Linux image
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

from linux_abi import AbiError, audit_elf, version_tuple


def elf_bytes(version="GLIBC_2.28", weak=True, flags=0, machine=183, rodata=b"GLIBC_9.99\0"):
    strings = b"\0libc.so.6\0" + version.encode() + b"\0statx\0"
    symbol_name = strings.index(b"statx")
    symbols = bytes(24) + struct.pack("<IBBHQQ", symbol_name, 0x22 if weak else 0x12, 0, 0, 0, 0)
    versym = struct.pack("<HH", 0, 2)
    verneed = struct.pack("<HHIII", 1, 1, 1, 16, 0)
    verneed += struct.pack("<IHHII", 0, flags, 2, strings.index(version.encode()), 0)
    dynamic = struct.pack("<qQqQ", 1, 1, 0, 0)
    # [null, strtab, dynsym, versym, verneed, dynamic, rodata]
    entries = [(3, strings, 0, 0, 1), (11, symbols, 1, 1, 24),
               (0x6fffffff, versym, 2, 0, 2), (0x6ffffffe, verneed, 1, 1, 0),
               (6, dynamic, 1, 0, 16), (1, rodata, 0, 0, 0)]
    body, sections = b"", [bytes(64)]
    for kind, payload, link, info, size in entries:
        sections.append(struct.pack("<IIQQQQIIQQ", 0, kind, 0, 0, 64 + len(body), len(payload), link, info, 1, size))
        body += payload
    header = b"\x7fELF\x02\x01\x01" + bytes(9)
    header += struct.pack("<HHIQQQIHHHHHH", 3, machine, 1, 0, 0, 64 + len(body), 0, 64, 0, 0, 64, len(sections), 0)
    return header + body + b"".join(sections)


class LinuxAbiTests(unittest.TestCase):
    def test_real_version_tables_override_rodata_and_preserve_weak_requirements(self):
        report = audit_elf(elf_bytes())
        self.assertEqual(report["glibc_max_required"], "2.28")
        self.assertEqual(report["needed"], ["libc.so.6"])
        self.assertEqual(report["imports"], [{"name": "statx", "weak": True,
                                           "library": "libc.so.6", "version": "GLIBC_2.28", "flags": 0}])
        with self.assertRaisesRegex(AbiError, "exceeds.*2.17"):
            audit_elf(elf_bytes(), "2.17")
        # Weak VERSION nodes are also kept in this conservative portability gate.
        with self.assertRaises(AbiError):
            audit_elf(elf_bytes(flags=2), "2.17")

    def test_numerical_ceiling_and_private_nodes(self):
        self.assertLess(version_tuple("2.9"), version_tuple("2.28"))
        self.assertEqual(version_tuple("2.28"), version_tuple("2.28.0"))
        self.assertEqual(audit_elf(elf_bytes("GLIBC_2.17", weak=False), "2.17")["glibc_max_required"], "2.17")
        for version in ("GLIBC_2.30", "GLIBC_2.38", "GLIBC_PRIVATE", "GLIBC_ABI_DT_RELR"):
            with self.subTest(version=version), self.assertRaises(AbiError):
                audit_elf(elf_bytes(version))
        for version in ("2", "2.28x", "-2.28"):
            with self.assertRaises(AbiError):
                version_tuple(version)

    def test_truncation_architecture_encoding_and_unknown_version_fail_closed(self):
        original = elf_bytes()
        for length in (0, 7, 16, 63, 80, len(original) - 1):
            with self.subTest(length=length), self.assertRaises(AbiError):
                audit_elf(original[:length])
        with self.assertRaisesRegex(AbiError, "ARM64"):
            audit_elf(elf_bytes(machine=62))
        wrong_endian = bytearray(original)
        wrong_endian[5] = 2
        with self.assertRaises(AbiError):
            audit_elf(bytes(wrong_endian))
        broken = bytearray(original)
        # VERSYM's second index must name a VERNEED entry, never a bare fallback.
        shoff = struct.unpack_from("<Q", original, 40)[0]
        versym_offset = struct.unpack_from("<Q", original, shoff + 3 * 64 + 24)[0]
        struct.pack_into("<H", broken, versym_offset + 2, 3)
        with self.assertRaisesRegex(AbiError, "unknown version index"):
            audit_elf(bytes(broken))

    def test_deployment_rejects_abi_drift_before_changing_managed_members(self):
        script = Path(__file__).with_name("deploy-select.py")
        with tempfile.TemporaryDirectory(prefix="rudroid-linux-abi-test-") as temp:
            root = Path(temp)
            build, dest = root / "build", root / "kit"
            build.mkdir()
            (build / "libandroid_bionic.so").write_bytes(elf_bytes("GLIBC_2.17"))
            command = [sys.executable, str(script), "--build", str(build), "--dest", str(dest), "android_bionic"]
            first = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(first.returncode, 0, first.stderr)
            previous = (dest / "libandroid_bionic.so").read_bytes()
            manifest = (dest / ".deploy-select-manifest.json").read_bytes()
            (build / "libandroid_bionic.so").write_bytes(elf_bytes("GLIBC_2.38"))
            failed = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(failed.returncode, 1, failed.stderr)
            self.assertIn("GLIBC_2.38", failed.stderr)
            self.assertEqual((dest / "libandroid_bionic.so").read_bytes(), previous)
            self.assertEqual((dest / ".deploy-select-manifest.json").read_bytes(), manifest)
            self.assertEqual(json.loads(manifest)["ids"], ["android_bionic"])


if __name__ == "__main__":
    unittest.main()
