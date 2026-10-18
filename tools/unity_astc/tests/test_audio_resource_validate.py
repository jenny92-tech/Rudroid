#!/usr/bin/env python3
# INPUT:  tempfile, unittest, pathlib, tools.unity_astc.audio_resource_validate
# OUTPUT:  AudioResourceValidateTest unittest 回归用例
# POS:    验证外部 AudioClip 资源头识别与错误报告
import tempfile
import unittest
from pathlib import Path

from tools.unity_astc.audio_resource_validate import AudioRef, validate_ref


class AudioResourceValidateTest(unittest.TestCase):
    def test_magic_must_fit_inside_the_declared_clip_range(self):
        with tempfile.TemporaryDirectory() as td:
            (Path(td) / "audio.resource").write_bytes(b"FSB5more data")
            problems = validate_ref(Path(td), AudioRef("too short", "audio.resource", 0, 2))
            self.assertTrue(problems)
            self.assertIn("invalid audio header", problems[0])

    def test_windows_resource_paths_resolve_by_the_actual_basename(self):
        with tempfile.TemporaryDirectory() as td:
            (Path(td) / "audio.resource").write_bytes(b"FSB5payload")
            self.assertEqual(validate_ref(Path(td), AudioRef("clip", r"C:\data\audio.resource", 0, 11)), [])

    def test_symlink_resource_and_out_of_bounds_ranges_are_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "real.resource").write_bytes(b"FSB5payload")
            (root / "link.resource").symlink_to(root / "real.resource")
            self.assertIn("symlink", validate_ref(root, AudioRef("clip", "link.resource", 0, 11))[0])
            self.assertIn("out of bounds", validate_ref(root, AudioRef("clip", "real.resource", 5, 100))[0])

    def test_accepts_external_clip_starting_with_fsb5(self):
        with tempfile.TemporaryDirectory() as td:
            data_dir = Path(td)
            (data_dir / "sharedassets1.resource").write_bytes(
                b"gap" + b"FSB5" + b"\x00" * 12)

            problems = validate_ref(
                data_dir,
                AudioRef("Title", "sharedassets1.resource", 3, 16),
            )

            self.assertEqual(problems, [])

    def test_rejects_external_clip_that_does_not_start_with_audio_header(self):
        with tempfile.TemporaryDirectory() as td:
            data_dir = Path(td)
            (data_dir / "sharedassets1.resource").write_bytes(
                b"gap" + b"not-a-valid-audio-chunk")

            problems = validate_ref(
                data_dir,
                AudioRef("Title", "sharedassets1.resource", 3, 16),
            )

            self.assertTrue(problems)
            self.assertIn("invalid audio header", problems[0])
            self.assertIn("Title", problems[0])


if __name__ == "__main__":
    unittest.main()
