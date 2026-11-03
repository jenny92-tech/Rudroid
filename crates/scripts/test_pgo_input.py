#!/usr/bin/env python3
# INPUT:  pgo-input.py, temporary files and Python unittest
# OUTPUT:  snapshot isolation, content identity and invalid-input regressions
# POS:    verifies training inputs cannot silently reuse a different profile
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("pgo_input", Path(__file__).with_name("pgo-input.py"))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class ProfileSnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.source = self.root / "training.profdata"
        self.source.write_bytes(b"first training profile")
        self.cache = self.root / "inputs"

    def tearDown(self):
        self.temp.cleanup()

    def test_same_path_changed_bytes_get_a_different_compiler_identity(self):
        first = profile.freeze(self.source, self.cache)
        self.source.write_bytes(b"second training profile")
        second = profile.freeze(self.source, self.cache)
        self.assertNotEqual(first, second)
        self.assertEqual((self.cache / (first + ".profdata")).read_bytes(), b"first training profile")
        self.assertEqual((self.cache / (second + ".profdata")).read_bytes(), b"second training profile")

    def test_identical_content_reuses_one_frozen_input(self):
        first = profile.freeze(self.source, self.cache)
        self.assertEqual(first, profile.freeze(self.source, self.cache))
        self.assertEqual(len(list(self.cache.iterdir())), 1)

    def test_existing_corruption_is_rejected(self):
        digest = profile.freeze(self.source, self.cache)
        snapshot = self.cache / (digest + ".profdata")
        snapshot.chmod(0o644)
        snapshot.write_bytes(b"corrupt")
        with self.assertRaises(ValueError):
            profile.freeze(self.source, self.cache)

    def test_empty_directory_and_oversized_inputs_are_rejected(self):
        self.source.write_bytes(b"")
        with self.assertRaises(ValueError):
            profile.freeze(self.source, self.cache)
        with self.assertRaises((OSError, ValueError)):
            profile.freeze(self.root, self.cache)
        with self.source.open("wb") as stream:
            stream.truncate(profile.LIMIT + 1)
        with self.assertRaises(ValueError):
            profile.freeze(self.source, self.cache)
        self.assertFalse(self.cache.exists())


if __name__ == "__main__":
    unittest.main()
