# INPUT:  temporary resource trees and mocked bundle worker
# OUTPUT:  one-shot publication, complete layouts, source immutability and failure regressions
# POS:    verifies the single public preparation operation without licensed game data
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
import contextlib
import io
import json
from pathlib import Path
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
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
        self.font_tables = []
        self.scans = []
        self.failure = False
        worker = patch.object(assets, "_run_bundle", side_effect=self.worker)
        worker.start()
        self.addCleanup(worker.stop)
        scanner = patch.object(assets, '_scan_font_bundle', side_effect=self.scan)
        scanner.start()
        self.addCleanup(scanner.stop)

    def write(self, relative, content):
        path = self.source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        return path

    def scan(self, source):
        self.scans.append(source.relative_to(self.source).as_posix())
        return {'roots': [], 'materials': []}

    def worker(self, source, output, target, protected_fonts=()):
        self.calls.append((source.relative_to(self.source).as_posix(), target))
        self.font_tables.append(tuple(protected_fonts))
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

    def test_concurrent_empty_directory_is_not_replaced(self):
        publish = assets._publish_directory
        identity = []
        def race(staging, output):
            output.mkdir()
            identity.append(output.stat().st_ino)
            return publish(staging, output)
        with patch.object(assets, '_publish_directory', side_effect=race):
            with self.assertRaises((ValueError, FileExistsError)):
                self.prepare()
        self.assertEqual(self.output.stat().st_ino, identity[0])
        self.assertEqual(list(self.output.iterdir()), [])
        self.assertEqual({p.name for p in self.root.iterdir()}, {self.source.name, self.output.name})

    def test_concurrent_symlink_and_nonempty_output_are_preserved(self):
        publish = assets._publish_directory
        other = self.root / 'other'
        other.mkdir()
        (other / 'personal').write_bytes(b'keep')
        def symlink(staging, output):
            output.symlink_to(other, target_is_directory=True)
            return publish(staging, output)
        with patch.object(assets, '_publish_directory', side_effect=symlink):
            with self.assertRaises((ValueError, FileExistsError)):
                self.prepare()
        self.assertTrue(self.output.is_symlink())
        self.assertEqual((other / 'personal').read_bytes(), b'keep')
        self.output.unlink()
        def directory(staging, output):
            output.mkdir()
            (output / 'personal').write_bytes(b'keep')
            return publish(staging, output)
        with patch.object(assets, '_publish_directory', side_effect=directory):
            with self.assertRaises((ValueError, FileExistsError)):
                self.prepare()
        self.assertEqual((self.output / 'personal').read_bytes(), b'keep')

    def test_simultaneous_preparations_publish_exactly_one_complete_tree(self):
        publish = assets._publish_directory
        barrier = threading.Barrier(2)
        def synchronized(staging, output):
            barrier.wait(timeout=10)
            return publish(staging, output)
        def run():
            try:
                assets.prepare(self.source, self.output)
                return True
            except (ValueError, FileExistsError):
                return False
        with patch.object(assets, '_publish_directory', side_effect=synchronized), contextlib.redirect_stdout(io.StringIO()):
            with ThreadPoolExecutor(max_workers=2) as pool:
                results = list(pool.map(lambda _: run(), range(2)))
        self.assertEqual(sorted(results), [False, True])
        self.assertEqual((self.output / self.one.relative_to(self.source)).read_bytes(), self.one.read_bytes())
        self.assertEqual(json.loads((self.output / assets.REPORT).read_text())['bundles'], 2)
        self.assertEqual({p.name for p in self.root.iterdir()}, {self.source.name, self.output.name})

    def test_input_output_overlap_is_rejected(self):
        for output in (self.source, self.source / "output", self.root):
            with self.subTest(output=output), self.assertRaisesRegex(ValueError, "separate"):
                self.prepare(output=output)
        self.assertEqual(self.calls, [])

    def test_changed_source_and_sidecars_abort_publication(self):
        for path in (self.one, self.audio):
            with self.subTest(path=path):
                original = path.read_bytes()
                def mutate(source, output, target, protected_fonts=()):
                    result = self.worker(source, output, target, protected_fonts)
                    path.write_bytes(b"changed")
                    return result
                with patch.object(assets, "_run_bundle", side_effect=mutate):
                    with self.assertRaisesRegex(ValueError, "input.*changed"):
                        self.prepare()
                self.assertFalse(self.output.exists())
                path.write_bytes(original)

    def test_new_files_during_preparation_are_detected(self):
        def mutate(source, output, target, protected_fonts=()):
            result = self.worker(source, output, target, protected_fonts)
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

    def test_font_roots_follow_external_materials_without_protecting_unrelated_textures(self):
        self.write('textures.bundle', b'UnityFS\0textures')
        graph = {
            self.one: {'roots': [['mat-owner', 7]], 'materials': []},
            self.two: {'roots': [], 'materials': [
                {'owner':'mat-owner','path_id':7,'textures':[['tex-owner',19]]},
                {'owner':'unrelated','path_id':8,'textures':[['tex-owner',20]]}]},
        }
        def scan(source):
            self.assertFalse(self.calls, 'scan all bundles before converting any atlas')
            return graph.get(source, {'roots': [], 'materials': []})
        with patch.object(assets, '_scan_font_bundle', side_effect=scan):
            self.prepare()
        self.assertEqual(len(self.font_tables), 3)
        for keys in self.font_tables:
            self.assertIn(('tex-owner',19),keys)
            self.assertNotIn(('tex-owner',20),keys)

    def test_inconsistent_material_owners_abort_before_any_conversion(self):
        def scan(source):
            return {'roots': [], 'materials': [{'owner':'same-owner','path_id':7,
                'textures':[['texture', 1 if source==self.one else 2]]}]}
        with patch.object(assets, '_scan_font_bundle', side_effect=scan):
            with self.assertRaisesRegex(ValueError, 'ambiguous cross-bundle'):
                self.prepare()
        self.assertFalse(self.calls)
        self.assertFalse(self.output.exists())

    def test_scanner_failure_preserves_input_and_does_not_publish(self):
        original=self.one.read_bytes()
        with patch.object(assets, '_scan_font_bundle', side_effect=ValueError('bad font metadata')):
            with self.assertRaisesRegex(ValueError, 'bad font metadata'):
                self.prepare()
        self.assertEqual(self.one.read_bytes(), original)
        self.assertFalse(self.calls)
        self.assertFalse(self.output.exists())

    def test_large_font_index_uses_worker_stdin_and_preserves_unicode_keys(self):
        script=self.root/'metadata-worker.py'
        script.write_text('import json,sys\n'
                          'assert len(sys.argv)==5 and sys.argv[1]=="--_bundle"\n'
                          'keys=json.load(sys.stdin)\n'
                          'print(json.dumps({"count":len(keys),"last":keys[-1]}))\n')
        keys=[('中文-font-owner-'+str(i),i) for i in range(10000)]
        with patch.object(assets,'__file__',str(script)):
            # The class normally mocks the converter; exercise the real child
            # transport here with an inert metadata-only Python worker.
            result=assets._run_worker(self.one,['--_bundle',str(self.one),str(self.output),'astc-1g'],keys)
        self.assertEqual(result,{'count':10000,'last':list(keys[-1])})

    def test_new_cli_needs_only_input_output_and_device_profile(self):
        with contextlib.redirect_stdout(io.StringIO()):
            assets.main([str(self.source), str(self.output), "--target", "astc-2g"])
        self.assertTrue(self.output.is_dir())
        self.assertEqual({target for _, target in self.calls}, {"astc-2g"})


if __name__ == "__main__":
    unittest.main()
