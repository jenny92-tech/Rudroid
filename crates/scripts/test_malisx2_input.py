#!/usr/bin/env python3
# INPUT:  malisx2 preparation/packaging and isolated archive/ELF fixtures
# OUTPUT: checksum, extraction, offline, Release and GNU artifact boundary regressions
# POS:    validates optional driver acquisition without compilers, Docker, devices or remote execution
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from test_linux_abi import elf_bytes

spec = importlib.util.spec_from_file_location("malisx2_input", Path(__file__).with_name("malisx2-input.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class Malisx2InputTests(unittest.TestCase):
    def archive(self, root, items):
        path = root / "input.tar.gz"
        with tarfile.open(path, "w:gz") as archive:
            for name, payload, kind in items:
                info = tarfile.TarInfo(name)
                if kind == "link":
                    info.type = tarfile.SYMTYPE
                    info.linkname = payload
                    archive.addfile(info)
                elif kind == "hardlink":
                    info.type = tarfile.LNKTYPE
                    info.linkname = payload
                    archive.addfile(info)
                elif kind == "directory":
                    info.type = tarfile.DIRTYPE
                    archive.addfile(info)
                else:
                    info.size = len(payload)
                    info.mode = 0o755 if kind == "executable" else 0o644
                    archive.addfile(info, io.BytesIO(payload))
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_checksum_failure_never_extracts(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve();archive, _ = self.archive(root, [("src/main.c", b"code", "file")])
            with self.assertRaisesRegex(ValueError, "checksum"):
                module.source_archive(archive, root / "out", "0" * 64)
            self.assertFalse((root / "out").exists())

    def test_internal_links_overlay_executability_and_exact_reuse(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            archive, digest = self.archive(root, [("src/NOTICE", b"terms", "file"),
                ("src/bin/run", b"script", "executable"), ("src/alias", "bin/run", "link")])
            out = root / "out"
            first = module.source_archive(archive, out, digest, {"meson.build": b"fixed overlay"})
            self.assertTrue((out / "bin/run").stat().st_mode & 0o111)
            self.assertEqual((out / "alias").read_bytes(), b"script")
            self.assertEqual(first, module.source_archive(archive, out, digest, {"meson.build": b"fixed overlay"}))
            (out / "bin/run").write_text("modified")
            with self.assertRaisesRegex(ValueError, "cached source changed"):
                module.source_archive(archive, out, digest, {"meson.build": b"fixed overlay"})

    def test_unsafe_members_duplicate_roots_and_links_fail_before_publish(self):
        cases = [
            [("src/../outside", b"x", "file")],
            [("/src/outside", b"x", "file")],
            [("src/a", b"x", "file"), ("other/b", b"y", "file")],
            [("src/a", b"x", "file"), ("src/a", b"y", "file")],
            [("src/a", "../outside", "link")],
            [("src/a", "missing", "link")],
            [("src/a", "b", "link"), ("src/b", "a", "link")],
            [("src/a", "b", "hardlink")],
            [("src/a", "dir", "link"), ("src/dir", "", "directory"), ("src/a/write", b"x", "file")],
        ]
        for entries in cases:
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as temp:
                root = Path(temp).resolve();archive, digest = self.archive(root, entries)
                with self.assertRaises(ValueError):
                    module.source_archive(archive, root / "out", digest)
                self.assertFalse((root / "out").exists())
                self.assertFalse((root / "outside").exists())

    def test_indirect_link_escape_is_rejected_without_outside_mutation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve();outside = root / "outside";outside.write_text("original")
            entries = [("src/a", "", "directory"), ("src/x", "", "directory"),
                       ("src/a/link", "../x", "link"), ("src/target", b"inside", "file"),
                       ("src/escape", "a/link/../../outside", "link"), ("src/outside", b"inside", "file")]
            archive, digest = self.archive(root, entries)
            with self.assertRaises(ValueError):
                module.source_archive(archive, root / "out", digest)
            self.assertEqual(outside.read_text(), "original")
            self.assertFalse((root / "out").exists())

    def test_cache_ancestor_link_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve();archive, digest = self.archive(root, [("src/a", b"x", "file")])
            (root / "elsewhere").mkdir();(root / "alias").symlink_to(root / "elsewhere")
            with self.assertRaisesRegex(ValueError, "symlink"):
                module.source_archive(archive, root / "alias/out", digest)
            self.assertFalse((root / "elsewhere/out").exists())

    def test_excess_entries_and_permissions_changes_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve();archive, digest = self.archive(root, [("src/run", b"x", "executable")])
            out = root / "out";module.source_archive(archive, out, digest)
            (out / "extra").write_text("unexpected")
            with self.assertRaisesRegex(ValueError, "cached source changed"):
                module.source_archive(archive, out, digest)
            (out / "extra").unlink();(out / "run").chmod(0o644)
            with self.assertRaisesRegex(ValueError, "cached source changed"):
                module.source_archive(archive, out, digest)

    def test_only_the_exact_regular_empty_wrap_lock_is_source_metadata(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            archive, digest = self.archive(root, [("src/subprojects", "", "directory"), ("src/main.c", b"code", "file")])
            out = root / "out";baseline = module.source_archive(archive, out, digest)
            lock = out / "subprojects/.wraplock";lock.touch()
            metadata = ("subprojects/.wraplock",)
            self.assertEqual(baseline, module.source_archive(archive, out, digest, empty_metadata=metadata))
            with self.assertRaisesRegex(ValueError, "cached source changed"):
                module.source_archive(archive, out, digest)
            lock.write_bytes(b"not metadata")
            with self.assertRaisesRegex(ValueError, "invalid empty build metadata"):
                module.source_archive(archive, out, digest, empty_metadata=metadata)
            lock.unlink();other = root / "empty";other.touch();lock.symlink_to(other)
            with self.assertRaisesRegex(ValueError, "invalid empty build metadata"):
                module.source_archive(archive, out, digest, empty_metadata=metadata)
            lock.unlink();lock.mkdir()
            with self.assertRaisesRegex(ValueError, "invalid empty build metadata"):
                module.source_archive(archive, out, digest, empty_metadata=metadata)
            lock.rmdir();lock.touch()
            cache = out / "__pycache__";cache.mkdir();(cache / "generator.pyc").write_bytes(b"bytecode")
            with self.assertRaisesRegex(ValueError, "cached source changed"):
                module.source_archive(archive, out, digest, empty_metadata=metadata)

    def test_offline_missing_source_does_not_use_network(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(module.urllib.request, "urlopen") as fetch:
            with self.assertRaisesRegex(ValueError, "missing verified offline input"):
                module.prepare(Path(temp).resolve(), offline=True)
            fetch.assert_not_called()

    def c11_cache_fixture(self, root):
        cache = root / "inputs";cache.mkdir()
        original = b"void f(void) {\nfound:\n\n   nir_builder b = nir_builder_at(nir_before_instr(&atest->instr));\n}\n"
        patched = original.replace(b"found:\n", b"found: ;\n")
        spec = {**module.C11_PATCH, "original_sha256": hashlib.sha256(original).hexdigest(),
                "patched_sha256": hashlib.sha256(patched).hexdigest()}
        def put(filename, entries):
            directory = root / ("archive-" + filename);directory.mkdir()
            path, sha = self.archive(directory, entries);shutil.move(path, cache / filename)
            return sha
        entries = [("upstream/" + spec["path"], original, "file"), ("upstream/untouched.c", b"original", "file")]
        for name in sorted(module.WRAPS):
            filename = name + ".tar.gz"
            sha = put(filename, [("crate/src/lib.rs", b"// crate source", "file")])
            recipe = ("[wrap-file]\ndirectory = " + name + "-1.0\nsource_url = https://static.crates.io/crates/" +
                      name + "/1.0/download\nsource_filename = " + filename + "\nsource_hash = " + sha +
                      "\npatch_directory = " + name + "\n")
            entries.extend([("upstream/subprojects/" + name + ".wrap", recipe.encode(), "file"),
                            ("upstream/subprojects/packagefiles/" + name + "/meson.build", b"project('fixture')", "file")])
        driver_sha = put("malisx2-" + module.REVISION + ".tar.gz", entries)
        mesa_sha = put("mesa-" + module.MESA_REVISION + ".tar.gz", [("mesa/untouched.c", b"mesa original", "file")])
        sources = {"malisx2": ("bmdhacks/malisx2", module.REVISION, driver_sha),
                   "mesa": ("bmdhacks/armsx2-turnip", module.MESA_REVISION, mesa_sha)}
        return cache, sources, spec, original, patched

    def test_c11_fresh_prepare_migration_reuse_verify_and_receipt(self):
        for migrate in [False, True]:
            with self.subTest(migrate=migrate), tempfile.TemporaryDirectory() as temp:
                root = Path(temp).resolve();cache, sources, spec, original, patched = self.c11_cache_fixture(root)
                with patch.object(module, "SOURCES", sources), patch.object(module, "C11_PATCH", spec):
                    if migrate:
                        module._prepare_inputs(cache, patched=False)
                        before = {p: p.stat().st_mtime_ns for p in (cache / "source").rglob("*")
                                  if p.is_file() and p != cache / "source" / spec["path"]}
                        with self.assertRaisesRegex(ValueError, "cached source changed"):
                            module.prepare(cache, offline=True)
                        self.assertEqual((cache / "source" / spec["path"]).read_bytes(), original)
                    object_file = root / "kept.o";object_file.write_bytes(b"compiled object");object_time = object_file.stat().st_mtime_ns
                    receipt = module.prepare(cache)
                    self.assertEqual((cache / "source" / spec["path"]).read_bytes(), patched)
                    self.assertEqual(receipt["linux_build_patch"], spec)
                    self.assertEqual(receipt["malisx2"]["revision"], module.REVISION)
                    if migrate:
                        self.assertEqual(before, {p: p.stat().st_mtime_ns for p in before})
                    times = {p: p.stat().st_mtime_ns for p in cache.rglob("*") if p.is_file()}
                    self.assertEqual(module.prepare(cache), receipt)
                    self.assertEqual(module.prepare(cache, offline=True), receipt)
                    self.assertEqual(times, {p: p.stat().st_mtime_ns for p in times})
                    self.assertEqual(object_file.stat().st_mtime_ns, object_time)
                    self.assertEqual(json.loads((cache / "inputs.json").read_text())["linux_build_patch"], spec)

    def test_c11_migration_rejects_tampering_before_any_source_write(self):
        for kind in ["base", "mesa", "crate", "receipt", "target"]:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                root = Path(temp).resolve();cache, sources, spec, original, _ = self.c11_cache_fixture(root)
                with patch.object(module, "SOURCES", sources), patch.object(module, "C11_PATCH", spec):
                    module._prepare_inputs(cache, patched=False)
                    paths = {"base": cache / "source/untouched.c", "mesa": cache / "source/mesa/untouched.c",
                             "crate": cache / "source/subprojects/bitflags-2-rs-1.0/src/lib.rs",
                             "receipt": cache / "inputs.json", "target": cache / "source" / spec["path"]}
                    paths[kind].write_bytes(b"tampered")
                    source = cache / "source" / spec["path"]
                    before = source.read_bytes()
                    with self.assertRaises(ValueError):
                        module.prepare(cache)
                    self.assertEqual(source.read_bytes(), before)

    def test_tool_preflight_reports_missing_dependencies_without_installing(self):
        with tempfile.TemporaryDirectory() as temp, \
             patch.object(module.platform, "system", return_value="Linux"), \
             patch.object(module.platform, "machine", return_value="aarch64"), \
             patch.object(module.shutil, "which", return_value=None), \
             patch.object(module.subprocess, "check_output") as execute:
            output = Path(temp).resolve() / "tools.json"
            with self.assertRaisesRegex(ValueError, "fixed image is missing.*meson.*bindgen"):
                module.preflight(output)
            execute.assert_not_called()
            self.assertFalse(output.exists())

    def test_python39_is_rejected_before_configuring_mesa(self):
        with tempfile.TemporaryDirectory() as temp, \
             patch.object(module.platform, "system", return_value="Linux"), \
             patch.object(module.platform, "machine", return_value="aarch64"), \
             patch.object(module.sys, "version_info", (3, 9, 99)), \
             patch.object(module.subprocess, "check_output") as execute:
            with self.assertRaisesRegex(ValueError, "Python >=3.10"):
                module.preflight(Path(temp).resolve() / "tools.json")
            execute.assert_not_called()

    def test_mesa_generator_must_use_the_preflight_python(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            (root / "tools.json").write_text(json.dumps({"python_executable": "/usr/local/bin/python3.11"}))
            ninja = root / "build.ninja"
            ninja.write_text(" COMMAND = /usr/local/bin/python3.11 /source/vk_cmd_queue_gen.py --xml vk.xml\n")
            module.check_python_build(root)
            for text in [" COMMAND = /usr/bin/python3 /source/vk_cmd_queue_gen.py\n", "no generator command"]:
                ninja.write_text(text)
                with self.assertRaisesRegex(ValueError, "Meson did not select"):
                    module.check_python_build(root)

    def package_fixture(self, root):
        runtime = root / "runtime";runtime.mkdir()
        (runtime / "libvulkan_malisx2.so").write_bytes(elf_bytes())
        inputs = root / "inputs";(inputs / "source").mkdir(parents=True)
        (inputs / "source/LICENSE").write_text("upstream terms")
        (inputs / "source/LICENSE_CC0").write_text("additional upstream terms")
        build = root / "objects";(build / "meson-info").mkdir(parents=True)
        options = {"buildtype": "release", "b_ndebug": "true", "measurement": False, "debug": False, "strip": True}
        self.write_options(build, options)
        (build / "tools.json").write_text(json.dumps({"rustc": "rustc 1.88.0"}))
        return runtime, inputs, build, options

    def write_options(self, build, options):
        (build / "meson-info/intro-buildoptions.json").write_text(json.dumps([
            {"name": name, "value": value} for name, value in options.items()]))

    def test_release_gnu_bundle_has_pinned_identity_relative_icd_and_terms(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(module, "prepare", return_value={"checked": True}):
            runtime, inputs, build, _ = self.package_fixture(Path(temp).resolve())
            module.package(runtime, inputs, build, "2.33")
            sources = json.loads((runtime / "sources.json").read_text())
            self.assertEqual(sources["platform"], "linux-aarch64")
            self.assertEqual(sources["profile"], "Release")
            self.assertEqual(sources["source_revision"], module.REVISION)
            self.assertEqual(sources["library_sha256"], module.digest(runtime / "libvulkan_malisx2.so"))
            self.assertEqual(sources["build_recipe_sha256"],
                             module.digest(Path(module.__file__).with_name("rudroid-rust-gles.Dockerfile")))
            self.assertEqual(json.loads((runtime / "mali_icd.json").read_text())["ICD"],
                             {"library_path": "./libvulkan_malisx2.so", "api_version": "1.1.0"})
            self.assertEqual((runtime / "licenses/LICENSE").read_text(), "upstream terms")
            self.assertEqual((runtime / "licenses/LICENSE_CC0").read_text(), "additional upstream terms")
            self.assertEqual(module.digest(runtime / module.C11_PATCH["patch_file"]), module.C11_PATCH["patch_sha256"])

    def test_diagnostic_options_and_bionic_binary_never_receive_manifest(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(module, "prepare", return_value={}):
            runtime, inputs, build, options = self.package_fixture(Path(temp).resolve())
            for key, value in [("buildtype", "debug"), ("measurement", True), ("debug", True), ("b_ndebug", "false"), ("strip", False)]:
                with self.subTest(key=key):
                    self.write_options(build, {**options, key: value})
                    with self.assertRaisesRegex(ValueError, "Release"):
                        module.package(runtime, inputs, build, "2.33")
                    self.assertFalse((runtime / "sources.json").exists())
            self.write_options(build, options)
            (runtime / "libvulkan_malisx2.so").write_bytes(elf_bytes().replace(b"libc.so.6\0", b"libc.so\0\0\0"))
            with self.assertRaisesRegex(ValueError, "not Android Bionic"):
                module.package(runtime, inputs, build, "2.33")
            self.assertFalse((runtime / "sources.json").exists())

    def test_packaging_rejects_linked_metadata_before_mutation(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(module, "prepare", return_value={}):
            root = Path(temp).resolve();runtime, inputs, build, _ = self.package_fixture(root)
            outside = root / "untouched";outside.write_text("original")
            (runtime / "linux-abi.json").symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "symlink"):
                module.package(runtime, inputs, build, "2.33")
            self.assertEqual(outside.read_text(), "original")

    def test_real_kit_checks_output_before_mkdir_or_strip(self):
        # Execute the maintained shell. Only build tools are fixtures; the
        # --check-output invocation runs the real Python gate. A normal output
        # reaches a strip fixture that genuinely writes its output file, so a
        # misplaced gate would overwrite the external sentinel and fail.
        script = Path(__file__).with_name("malisx2-kit.sh").resolve()
        for alias in ["none", "runtime", "ancestor", "library", "receipt", "licenses"]:
            with self.subTest(alias=alias), tempfile.TemporaryDirectory() as temp:
                root = Path(temp).resolve();project = root / "project";scripts = project / "scripts"
                scripts.mkdir(parents=True)
                shutil.copyfile(Path(module.__file__), scripts / "malisx2-input.py")
                binaries = root / "bin";binaries.mkdir()
                shim = ("#!/bin/sh\n"
                        "[ \"$PYTHONDONTWRITEBYTECODE\" = 1 ] || exit 97\n"
                        "if [ \"$1\" = -m ]; then shift 2; exec meson \"$@\"; fi\n"
                        "case \"$2\" in --preflight|--verify|--runtime) exit 0;; esac\n"
                        "exec " + shlex.quote(sys.executable) + " \"$@\"\n")
                meson = ('#!/bin/sh\nobjects="$CARGO_TARGET_DIR/native-objects/malisx2"\n'
                         'mkdir -p "$objects"\n'
                         'printf \'%s\\n\' \'{"python_executable":"/usr/local/bin/python3.11"}\' > "$objects/tools.json"\n'
                         'printf \'%s\\n\' \' COMMAND = /usr/local/bin/python3.11 /source/vk_cmd_queue_gen.py\' > "$objects/build.ninja"\n')
                commands = {"python3": shim, "meson": meson, "ninja": "#!/bin/sh\nexit 0\n",
                            "strip": '#!/bin/sh\nprintf reached > "$MALISX2_TEST_STRIP_MARKER"\nprintf stripped > "$3"\n'}
                for name, content in commands.items():
                    path = binaries / name;path.write_text(content);path.chmod(0o755)
                target = root / "target";runtime = target / "release/vulkan-malisx2"
                outside = root / "outside";outside.mkdir()
                sentinel = outside / "libvulkan_malisx2.so";sentinel.write_text("original")
                runtime.parent.mkdir(parents=True)
                if alias == "runtime":
                    runtime.symlink_to(outside)
                elif alias == "ancestor":
                    runtime.parent.rmdir();runtime.parent.symlink_to(outside)
                elif alias != "none":
                    runtime.mkdir()
                    name = {"library": "libvulkan_malisx2.so", "receipt": "sources.json", "licenses": "licenses"}[alias]
                    (runtime / name).symlink_to(outside if alias == "licenses" else sentinel)
                marker = root / "strip-reached"
                environment = {**os.environ, "PATH": str(binaries) + os.pathsep + os.defpath,
                               "CARGO_TARGET_DIR": str(target), "MALISX2_TEST_STRIP_MARKER": str(marker)}
                result = subprocess.run(["bash", str(script)], cwd=project, env=environment,
                                        capture_output=True, text=True)
                self.assertEqual(sentinel.read_text(), "original")
                if alias == "none":
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertTrue(marker.exists())
                    self.assertEqual((runtime / "libvulkan_malisx2.so").read_text(), "stripped")
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("symlink", result.stderr)
                    self.assertFalse(marker.exists(), "output gate ran after strip")
                    if alias == "ancestor":
                        self.assertFalse((outside / "vulkan-malisx2").exists(), "output gate ran after mkdir")


class DriverEntryTests(unittest.TestCase):
    def entry(self, *options):
        with tempfile.TemporaryDirectory(prefix="driver entry ") as temporary:
            root=Path(temporary);scripts=root/'crates/scripts';scripts.mkdir(parents=True)
            entry=scripts/'docker-rust.sh'
            shutil.copyfile(Path(__file__).with_name('docker-rust.sh'),entry)
            tools=root/'bin';tools.mkdir()
            docker=tools/'docker'
            docker.write_text(f'#!{sys.executable}\n'+'''import json,os,sys
args=sys.argv[1:]
if args[:2]==['image','inspect']: print('linux/arm64')
elif args and args[0]=='run':
    with open(os.environ['ENTRY_LOG'],'w') as out: json.dump(args,out)
''')
            python=tools/'python3'
            python.write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> "$PREP_LOG"\n')
            docker.chmod(0o755);python.chmod(0o755)
            log=root/'run.json';prep=root/'prepare.txt'
            env=dict(os.environ,PATH=str(tools)+os.pathsep+os.environ['PATH'],
                     ENTRY_LOG=str(log),PREP_LOG=str(prep))
            env.pop('RUDROID_VAMPIRE_GAME_DIR',None)
            if '--driver-only' in options:
                env['RUDROID_VAMPIRE_GAME_DIR']='/nonexistent/unused-private-game'
            result=subprocess.run(['bash',str(entry),'build','--release',*options],
                                  env=env,capture_output=True,text=True,timeout=10)
            return result,json.loads(log.read_text()) if log.exists() else [],prep.read_text() if prep.exists() else ''

    def test_driver_only_build_has_no_cargo_or_wsi_prerequisite(self):
        result,args,prep=self.entry('--malisx2','--driver-only')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('scripts/malisx2-kit.sh',args)
        self.assertNotIn('scripts/vulkan-kit.sh',args)
        self.assertNotIn('--workspace',args)
        self.assertFalse(any('/game-data/' in arg for arg in args))
        self.assertIn('--network=none',args)
        self.assertIn('malisx2-input.py',prep)
        self.assertNotIn('vulkan-input.py',prep)

    def test_complete_kit_keeps_the_explicit_addon_build_path(self):
        result,args,prep=self.entry('--malisx2')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('scripts/vulkan-kit.sh',args)
        self.assertIn('--workspace',args)
        self.assertIn('vulkan-input.py',prep)

    def test_driver_only_rejects_unrelated_kit_options_before_docker(self):
        for options in (('--driver-only',),('--malisx2','--driver-only','--thin-lto')):
            result,args,_=self.entry(*options)
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(args,[])


if __name__ == "__main__":
    unittest.main()
