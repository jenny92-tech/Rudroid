# INPUT:  synthetic Godot v2/v3 packs, bounded parser and MD5 verifier
# OUTPUT:  directory boundary, encryption, corruption and CTEX classification tests
# POS:    verifies the separate Godot audit adapter without game files
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
import hashlib
from pathlib import Path
import struct
import tempfile
import unittest

from tools.audit_pck import audit, directory


class PckAuditTest(unittest.TestCase):
    def pack(self, root, version=3, flags=0, payload=b"content", name="res://asset.dat"):
        encoded = name.encode() + b"\0"
        encoded += bytes((-len(encoded)) % 4)
        entry = struct.pack("<I", len(encoded)) + encoded
        entry += struct.pack("<QQ16sI", 0, len(payload), hashlib.md5(payload).digest(), 0)
        table = struct.pack("<I", 1) + entry
        prefix = b"GDPC" + struct.pack("<5IQ", version, 4, 5, 1, flags, 256)
        if version == 2:
            header = prefix + bytes(64) + table
        else:
            header = prefix + struct.pack("<Q", 96) + bytes(56) + table
        path = Path(root) / "sample.pck"
        path.write_bytes(header + bytes(256 - len(header)) + payload)
        return path

    def test_v2_and_v3_directory_entries_and_digests(self):
        with tempfile.TemporaryDirectory() as root:
            for version in (2, 3):
                report = audit(self.pack(root, version), True)
                self.assertEqual(report["status"], "complete")
                self.assertEqual(report["summary"]["digests_verified"], 1)

    def test_encrypted_directory_is_not_misread_as_cleartext(self):
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaisesRegex(ValueError, "encrypted directory"):
                directory(self.pack(root, flags=1))

    def test_sparse_pack_is_not_misreported_as_standalone(self):
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaisesRegex(ValueError, "sparse"):
                directory(self.pack(root, flags=4))

    def test_truncated_entry_range_is_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.pack(root)
            path.write_bytes(path.read_bytes()[:-1])
            with self.assertRaisesRegex(ValueError, "entry outside"):
                directory(path)

    def test_modified_payload_fails_digest_verification(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.pack(root)
            with path.open("r+b") as handle:
                handle.seek(-1, 2)
                handle.write(b"X")
            result = audit(path, True)
            self.assertEqual(result["status"], "issues_found")
            self.assertEqual(result["summary"]["errors"], 1)

    def test_ctex_without_platform_suffix_has_a_bounded_summary_key(self):
        with tempfile.TemporaryDirectory() as root:
            payload = bytearray(52)
            payload[:4] = b"GST2"
            struct.pack_into("<3I", payload, 4, 1, 16, 16)
            struct.pack_into("<IHHII", payload, 36, 0, 16, 16, 4, 37)
            path = self.pack(root, payload=bytes(payload), name="res://image.png-fingerprint.ctex")
            result = audit(path, True)
            self.assertEqual(result["summary"]["variants"], {"unqualified": 1})
            self.assertEqual(result["textures"][0]["mip_levels"], 5)


if __name__ == "__main__":
    unittest.main()
