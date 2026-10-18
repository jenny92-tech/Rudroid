#!/usr/bin/env python3
# INPUT:  deploy-select.py, kit-manifest.py, kit_markers.py, Python unittest, temporary build and deploy dirs
# OUTPUT:  Regression test results for deploy selection, kit manifests and test-build markers (ELF .dynsym / Mach-O)
# POS:    Verifies that duplicate/case-conflicting members, manifest path validation, partial-failure records and test-build rejection do not corrupt a deployment
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
DEPLOY = SCRIPTS / "deploy-select.py"
KIT_MANIFEST = SCRIPTS / "kit-manifest.py"
MANIFEST = ".deploy-select-manifest.json"
KIT_ID = b"rust-kit-0123456789abcdef"

sys.path.insert(0, str(SCRIPTS))
from kit_markers import TEST_BUILD_MARKERS, test_build_marker  # noqa: E402

MARKER = TEST_BUILD_MARKERS[1]


def elf_bytes(symbols=(), rodata=b""):
    """Minimal little-endian ELF64: section headers [null, .dynstr, .dynsym,
    .rodata]; `symbols` are (name, defined) pairs in .dynsym."""
    dynstr = b"\0"
    entries = [struct.pack("<IBBHQQ", 0, 0, 0, 0, 0, 0)]
    for name, defined in symbols:
        entries.append(struct.pack("<IBBHQQ", len(dynstr), 0x12, 0, 3 if defined else 0, 0, 0))
        dynstr += name.encode() + b"\0"
    dynsym = b"".join(entries)
    body = dynstr + dynsym + rodata
    dynstr_off = 64
    dynsym_off = dynstr_off + len(dynstr)
    rodata_off = dynsym_off + len(dynsym)
    shoff = 64 + len(body)
    header = b"\x7fELF" + bytes([2, 1, 1]) + bytes(9)
    header += struct.pack("<HHIQQQIHHHHHH", 3, 183, 1, 0, 0, shoff, 0, 64, 0, 0, 64, 4, 0)
    sections = [
        struct.pack("<IIQQQQIIQQ", 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
        struct.pack("<IIQQQQIIQQ", 0, 3, 0, 0, dynstr_off, len(dynstr), 0, 0, 1, 0),
        struct.pack("<IIQQQQIIQQ", 0, 11, 0, 0, dynsym_off, len(dynsym), 1, 1, 8, 24),
        struct.pack("<IIQQQQIIQQ", 0, 1, 0, 0, rodata_off, len(rodata), 0, 0, 1, 0),
    ]
    return header + body + b"".join(sections)


class DeploySelectionTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="rudroid-deploy-test-")
        self.root = Path(self._tmp.name)
        self.build = self.root / "build"
        self.dest = self.root / "kit"
        self.build.mkdir()
        self.dest.mkdir()
        for cap in ("android_base", "platform_log_stderr", "sdk_xlua"):
            (self.build / f"lib{cap}.so").write_bytes(cap.encode())
        (self.dest / "config.toml").write_text("user data")

    def tearDown(self):
        self._tmp.cleanup()

    def deploy(self, *args):
        command = [sys.executable, str(DEPLOY), "--build", str(self.build), "--dest", str(self.dest)]
        return subprocess.run(command + list(args), capture_output=True, text=True)

    def manifest(self):
        return json.loads((self.dest / MANIFEST).read_text())

    def members(self):
        return sorted(p.name for p in self.dest.iterdir() if p.suffix == ".so")

    def test_duplicate_cli_and_profile_members_preserve_managed_selection(self):
        profile = self.root / "selection.txt"
        profile.write_text("android_base\nplatform_log_stderr\nandroid_base\n")
        result = self.deploy("android_base", "--from-profile", str(profile))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.manifest()["ids"], ["android_base", "platform_log_stderr"])
        self.assertEqual(self.manifest()["managed"], ["libandroid_base.so", "libplatform_log_stderr.so"])
        result = self.deploy("android_base", "android_base")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.dest / "libplatform_log_stderr.so").exists())
        self.assertEqual((self.dest / "libandroid_base.so").read_bytes(), b"android_base")
        self.assertEqual((self.dest / "config.toml").read_text(), "user data")
        self.assertEqual(self.manifest()["ids"], ["android_base"])
        self.assertEqual([p.name for p in self.dest.iterdir() if p.name.endswith(".tmp")], [])

    def test_case_variant_ids_are_refused_before_touching_dest(self):
        result = self.deploy("android_base", "Android_base")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("differ only in case", result.stderr)
        self.assertEqual(self.members(), [])
        self.assertFalse((self.dest / MANIFEST).exists())

    def test_ids_resolve_case_exactly(self):
        # A case-insensitive build dir must not resolve `Android_base` to
        # `libandroid_base.so` (it would deploy `libAndroid_base.so`).
        result = self.deploy("Android_base")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("Android_base", result.stderr)
        self.assertEqual(self.members(), [])

    def test_ids_with_path_components_are_refused(self):
        result = self.deploy("../escape")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertEqual(self.members(), [])

    def test_manifest_entries_outside_dest_are_never_deleted(self):
        victim = self.root / "outside" / "victim.txt"
        victim.parent.mkdir()
        victim.write_text("keep me")
        (self.dest / MANIFEST).write_text(
            json.dumps({"format": 1, "managed": ["../outside/victim.txt"], "ids": [], "loader": None})
        )
        result = self.deploy("android_base")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("outside the deploy dir", result.stderr)
        self.assertEqual(victim.read_text(), "keep me")
        self.assertEqual(self.members(), [])

    def test_corrupt_manifest_is_refused_not_treated_as_legacy(self):
        self.assertEqual(self.deploy("android_base", "sdk_xlua").returncode, 0)
        (self.dest / MANIFEST).write_text('{"format": 1, "managed": [')
        result = self.deploy("android_base")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("cannot read deploy manifest", result.stderr)
        self.assertEqual(self.members(), ["libandroid_base.so", "libsdk_xlua.so"])

    def test_failed_move_records_what_is_on_disk(self):
        self.assertEqual(self.deploy("sdk_xlua").returncode, 0)
        # A non-empty directory where the second member goes: its rename
        # fails after the first member is already in place.
        blocker = self.dest / "libplatform_log_stderr.so"
        blocker.mkdir()
        (blocker / "occupied").write_text("x")
        result = self.deploy("android_base", "platform_log_stderr")
        self.assertEqual(result.returncode, 1, result.stderr)
        managed = self.manifest()["managed"]
        self.assertIn("libandroid_base.so", managed, "placed member must stay tracked")
        self.assertIn("libsdk_xlua.so", managed, "unremoved stale member must stay tracked")
        self.assertNotIn("libplatform_log_stderr.so", managed, "never placed, never claimed")
        # The next deploy cleans both up.
        (blocker / "occupied").unlink()
        blocker.rmdir()
        result = self.deploy("platform_log_stderr")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.members(), ["libplatform_log_stderr.so"])
        self.assertEqual(self.manifest()["managed"], ["libplatform_log_stderr.so"])

    def test_unremovable_stale_member_stays_managed(self):
        self.assertEqual(self.deploy("android_base").returncode, 0)
        manifest = self.manifest()
        manifest["managed"].append("libold.so")
        (self.dest / MANIFEST).write_text(json.dumps(manifest))
        stale = self.dest / "libold.so"
        stale.mkdir()
        (stale / "occupied").write_text("x")
        result = self.deploy("android_base")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(self.manifest()["managed"], ["libandroid_base.so", "libold.so"])

    def test_test_feature_artifacts_are_refused(self):
        (self.build / "libandroid_elf_load.so").write_bytes(
            elf_bytes([("rudroid_elf_exec_load", True), (MARKER, True)], KIT_ID)
        )
        result = self.deploy("android_base", "android_elf_load")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("test-feature build", result.stderr)
        self.assertEqual(self.members(), [])

    def test_marker_detection_only_counts_real_exports(self):
        path = self.root / "probe.so"
        macho = b"\xcf\xfa\xed\xfe" + bytes(28)
        cases = [
            (elf_bytes([(MARKER, True)]), MARKER),
            # The loader binary carries the names as rodata strings.
            (elf_bytes([("main", True)], b"\0" + MARKER.encode() + b"\0"), None),
            # An undefined import of the name is not an export.
            (elf_bytes([(MARKER, False)]), None),
            (elf_bytes([(MARKER + "_x", True)]), None),
            # Mach-O names carry a leading underscore.
            (macho + b"\0_" + MARKER.encode() + b"\0", MARKER),
            (macho + b"\0_" + MARKER.encode() + b"_x\0", None),
            (b"not a binary \0" + MARKER.encode() + b"\0", None),
        ]
        for data, expected in cases:
            path.write_bytes(data)
            self.assertEqual(test_build_marker(path), expected, data[:8])
        (self.build / "libandroid_elf_load.dylib").write_bytes(cases[4][0])
        result = self.deploy("android_elf_load")
        self.assertEqual(result.returncode, 1, "a Mach-O test build is refused too")

    def test_kit_manifest_refuses_test_feature_artifacts(self):
        for cap in ("android_base", "android_elf_load"):
            (self.build / f"lib{cap}.so").write_bytes(elf_bytes([("x", True)], KIT_ID))
        output = self.root / "kit.json"
        scan = [sys.executable, str(KIT_MANIFEST), "scan", "--build", str(self.build), "--output", str(output)]
        self.assertEqual(subprocess.run(scan, capture_output=True).returncode, 0)
        (self.build / "libandroid_elf_load.so").write_bytes(elf_bytes([(MARKER, True)], KIT_ID))
        result = subprocess.run(scan, capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(MARKER, result.stderr)
        verify = [sys.executable, str(KIT_MANIFEST), "verify", "--dir", str(self.build), "--manifest", str(output)]
        self.assertEqual(subprocess.run(verify, capture_output=True).returncode, 1)

    def test_markers_match_the_loader_list(self):
        source = (SCRIPTS.parent / "project/unity_loader/src/lib.rs").read_text()
        block = re.search(r"TEST_BUILD_MARKERS: \[&str; \d+\] =\s*\[(.*?)\];", source, re.S)
        self.assertIsNotNone(block)
        self.assertEqual(tuple(re.findall(r'"([^"]+)"', block.group(1))), TEST_BUILD_MARKERS)

    def test_pgo_training_members_are_rejected_before_deployment(self):
        marker = "rudroid_pgo_training_flush"
        (self.build / "libandroid_base.so").write_bytes(elf_bytes([(marker, True)], KIT_ID))
        result = self.deploy("android_base")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(marker, result.stderr)
        self.assertFalse((self.dest / "libandroid_base.so").exists())
        result = subprocess.run([sys.executable, str(KIT_MANIFEST), "scan", "--build", str(self.build),
                                 "--output", str(self.root / "train.json")], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(marker, result.stderr)


if __name__ == "__main__":
    unittest.main()
