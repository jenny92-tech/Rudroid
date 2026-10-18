#!/usr/bin/env python3
# INPUT:  optional Wayland source preparation and isolated synthetic archives
# OUTPUT:  checksum, archive escape, link and cache tampering rejection checks
# POS:    keep external client preparation fail-closed before native compilation
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("wayland_input", Path(__file__).with_name("wayland-input.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class InputTests(unittest.TestCase):
    def archive(self, root, extra=None):
        path = root / "source.crate"
        with tarfile.open(path, "w:gz") as archive:
            for name, data in [("configure", b"#!/bin/sh\n"), ("LICENSE.txt", b"upstream terms")]:
                info = tarfile.TarInfo(module.SDL_PREFIX + name)
                info.size = len(data)
                info.mode = 0o755 if name == "configure" else 0o644
                archive.addfile(info, io.BytesIO(data))
            if extra is not None:
                archive.addfile(extra, io.BytesIO(b"x") if extra.isfile() else None)
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_wrong_checksum_never_extracts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, _ = self.archive(root)
            with self.assertRaisesRegex(ValueError, "checksum"):
                module.verified_sdl(archive, root / "sdl", "0" * 64)
            self.assertFalse((root / "sdl").exists())

    def test_extracts_executable_and_refuses_mutated_cache(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, digest = self.archive(root)
            module.verified_sdl(archive, root / "sdl", digest)
            self.assertTrue((root / "sdl/configure").stat().st_mode & 0o111)
            module.verified_sdl(archive, root / "sdl", digest)
            (root / "sdl/configure").write_text("changed")
            with self.assertRaisesRegex(ValueError, "cached SDL"):
                module.verified_sdl(archive, root / "sdl", digest)

    def test_source_links_and_traversal_never_escape(self):
        for link in [False, True]:
            with self.subTest(link=link), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                info = tarfile.TarInfo(module.SDL_PREFIX + ("alias" if link else "../outside"))
                if link:
                    info.type = tarfile.SYMTYPE
                    info.linkname = "../outside"
                else:
                    info.size = 1
                archive, digest = self.archive(root, info)
                with self.assertRaisesRegex(ValueError, "unsupported"):
                    module.verified_sdl(archive, root / "sdl", digest)
                self.assertFalse((root / "outside").exists())
                self.assertFalse((root / "sdl").exists())

    def test_unused_android_application_links_are_excluded(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            link = tarfile.TarInfo(module.SDL_PREFIX + "android-project-ant/src")
            link.type = tarfile.SYMTYPE
            link.linkname = "../android-project/app/src/main/java"
            archive, digest = self.archive(root, link)
            module.verified_sdl(archive, root / "sdl", digest)
            self.assertFalse((root / "sdl/android-project-ant").exists())

    def test_cache_symlink_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, digest = self.archive(root)
            module.verified_sdl(archive, root / "sdl", digest)
            (root / "sdl/alias").symlink_to(root / "missing")
            with self.assertRaisesRegex(ValueError, "symlink"):
                module.verified_sdl(archive, root / "sdl", digest)

    def test_unverified_loader_package_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "checksum"):
            module.loader_files(b"!<arch>\n")


if __name__ == "__main__":
    unittest.main()
