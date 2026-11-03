# INPUT:  temporary resource trees and mocked bundle worker
# OUTPUT:  one-shot publication, complete layouts, source immutability and failure regressions
# POS:    verifies the single public preparation operation without licensed game data
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import unity_assets as assets


class PreparationTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.source, self.output = self.root / "original 游戏", self.root / "ready 游戏"
        self.source.mkdir()
        self.one = self.write("base/common.bundle", b"UnityFS\0one")
        self.two = self.write("assetpacks/PAD/common.bundle", b"UnityFS\0two")
        self.audio = self.write("assetpacks/PAD/resources.resource", b"FSB5original audio")
        self.library = self.write("lib/arm64-v8a/libunity.so", b"ELF library")
        (self.source / "StreamingAssets/empty").mkdir(parents=True)
        self.calls = []
        self.failure = False
        worker = patch.object(assets, "_run_bundle", side_effect=self.worker)
        worker.start()
        self.addCleanup(worker.stop)

    def write(self, relative, content):
        path = self.source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        return path

    def worker(self, source, output, target):
        self.calls.append((source.relative_to(self.source).as_posix(), target))
        output.write_bytes(source.read_bytes())
        if self.failure and len(self.calls) == 2:
            raise ValueError("injected conversion failure")
        return {"textures": 1, "changed": 1, "decisions": {"astc-8x8": 1},
                "payload_before": 65536, "payload_after": 4096}

    def prepare(self, **changes):
        with contextlib.redirect_stdout(io.StringIO()):
            return assets.prepare(**{"source": self.source, "output": self.output, **changes})

    def test_one_call_produces_complete_ready_layout_and_preserves_originals(self):
        originals = {path: path.read_bytes() for path in (self.one, self.two, self.audio, self.library)}
        result = self.prepare()
        for original, data in originals.items():
            self.assertEqual(original.read_bytes(), data)
            self.assertEqual((self.output / original.relative_to(self.source)).read_bytes(), data)
        self.assertTrue((self.output / "StreamingAssets/empty").is_dir())
        self.assertEqual(result["changed_textures"], 2)
        self.assertEqual({target for _, target in self.calls}, {"astc-1g"})
        self.assertEqual(json.loads((self.output / assets.REPORT).read_text()), result)
        self.assertEqual({p.name for p in self.root.iterdir()}, {self.source.name, self.output.name})
        self.assertFalse(any(p.is_symlink() for p in self.output.rglob("*")))

    def test_failure_never_exposes_partial_output_and_cleans_staging(self):
        self.failure = True
        with self.assertRaisesRegex(ValueError, "conversion failure"):
            self.prepare()
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.root.iterdir()), [self.source])

    def test_existing_output_is_preserved_without_starting_conversion(self):
        self.output.mkdir()
        marker = self.output / "personal"
        marker.write_bytes(b"keep")
        with self.assertRaisesRegex(ValueError, "already exists"):
            self.prepare()
        self.assertEqual(marker.read_bytes(), b"keep")
        self.assertEqual(self.calls, [])

    def test_input_output_overlap_is_rejected(self):
        for output in (self.source, self.source / "output", self.root):
            with self.subTest(output=output), self.assertRaisesRegex(ValueError, "separate"):
                self.prepare(output=output)
        self.assertEqual(self.calls, [])

    def test_changed_source_and_sidecars_abort_publication(self):
        for path in (self.one, self.audio):
            with self.subTest(path=path):
                original = path.read_bytes()
                def mutate(source, output, target):
                    result = self.worker(source, output, target)
                    path.write_bytes(b"changed")
                    return result
                with patch.object(assets, "_run_bundle", side_effect=mutate):
                    with self.assertRaisesRegex(ValueError, "input.*changed"):
                        self.prepare()
                self.assertFalse(self.output.exists())
                path.write_bytes(original)

    def test_new_files_during_preparation_are_detected(self):
        def mutate(source, output, target):
            result = self.worker(source, output, target)
            self.write("added.bin", b"new")
            return result
        with patch.object(assets, "_run_bundle", side_effect=mutate):
            with self.assertRaisesRegex(ValueError, "tree changed"):
                self.prepare()
        self.assertFalse(self.output.exists())

    def test_symlink_inputs_and_prepared_inputs_are_rejected(self):
        link = self.source / "external.resource"
        link.symlink_to(self.audio)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.prepare()
        link.unlink()
        (self.source / assets.REPORT).write_text("prepared")
        with self.assertRaisesRegex(ValueError, "already prepared"):
            self.prepare()
        self.assertEqual(self.calls, [])

    def test_opaque_bundle_files_are_copied_without_parsing(self):
        opaque = self.write("native-plugin.bundle", b"not a UnityFS file")
        self.prepare(target="astc-4g")
        self.assertEqual((self.output / opaque.name).read_bytes(), opaque.read_bytes())
        self.assertEqual({target for _, target in self.calls}, {"astc-4g"})

    def test_bundle_discovery_uses_content_instead_of_names_or_extensions(self):
        self.one.unlink()
        self.two.unlink()
        names = {"base/a9df809ab17c", "assetpacks/PAD/payload.dat", "StreamingAssets/renamed.assets"}
        for name in names:
            self.write(name, b"UnityFS\0original pixels")
        opaque = self.write("not-unity.unity3d", b"ordinary file")
        result = self.prepare()
        self.assertEqual(result["bundles"], len(names))
        self.assertEqual({name for name, _ in self.calls}, names)
        self.assertEqual((self.output / opaque.name).read_bytes(), opaque.read_bytes())

    def test_unknown_target_fails_before_writing(self):
        with self.assertRaisesRegex(ValueError, "unknown target"):
            self.prepare(target="1g-must-support-every-gpu")
        self.assertFalse(self.output.exists())
        self.assertEqual(self.calls, [])

    def test_new_cli_needs_only_input_output_and_device_profile(self):
        with contextlib.redirect_stdout(io.StringIO()):
            assets.main([str(self.source), str(self.output), "--target", "astc-2g"])
        self.assertTrue(self.output.is_dir())
        self.assertEqual({target for _, target in self.calls}, {"astc-2g"})


if __name__ == "__main__":
    unittest.main()
