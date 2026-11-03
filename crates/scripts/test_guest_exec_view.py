#!/usr/bin/env python3
# INPUT:  guest-exec-view.py, temporary ELF headers and mocked namespace/mount calls
# OUTPUT:  path, fallback, propagation, cleanup and exec ownership regressions
# POS:    checks launcher isolation without mounting anything on the test host
import importlib.util
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "guest_view", Path(__file__).with_name("guest-exec-view.py"))
view = importlib.util.module_from_spec(spec)
spec.loader.exec_module(view)


class GuestExecViewTests(unittest.TestCase):
    def setUp(self):
        self.platform = patch.object(view.sys, "platform", "linux")
        self.platform.start()
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.library = self.root / "lib/arm64-v8a"
        self.library.mkdir(parents=True)
        self.source = self.library / "libmain.so"
        header = bytearray(20)
        header[:7] = b"\x7fELF\x02\x01\x01"
        header[18:20] = (183).to_bytes(2, "little")
        self.source.write_bytes(header)

    def tearDown(self):
        self.temporary.cleanup()
        self.platform.stop()

    def test_source_scope_and_abi(self):
        self.assertEqual(view.library_directory(self.root, "lib/arm64-v8a"), self.library)
        for relative in (".", str(self.library), "../"):
            with self.subTest(relative=relative), self.assertRaises(ValueError):
                view.library_directory(self.root, relative)
        for data in (b"broken", b"\x7fELF\x01\x01\x01" + bytes(13)):
            self.source.write_bytes(data)
            with self.assertRaises(ValueError):
                view.library_directory(self.root, "lib/arm64-v8a")

    def test_symlink_escape_and_symlink_libraries(self):
        (self.root / "escape").symlink_to(self.root.parent, target_is_directory=True)
        with self.assertRaises(ValueError):
            view.library_directory(self.root, "escape")
        (self.library / "libalias.so").symlink_to(self.source)
        with self.assertRaises(ValueError):
            view.library_directory(self.root, "lib/arm64-v8a")

    def test_already_executable_never_unshares_or_mounts(self):
        with patch.object(view.os, "statvfs", return_value=SimpleNamespace(f_flag=0)), \
                patch.object(view, "unshare_mounts") as namespace, \
                patch.object(view.subprocess, "run") as run:
            self.assertEqual(view.prepare_view(self.library), "already executable")
            namespace.assert_not_called()
            run.assert_not_called()

    def test_namespace_failure_never_mounts(self):
        with patch.object(view.os, "statvfs", return_value=SimpleNamespace(f_flag=view.ST_NOEXEC)), \
                patch.object(view.shutil, "which", return_value="/bin/mount"), \
                patch.object(view, "unshare_mounts", side_effect=PermissionError("denied")), \
                patch.object(view.subprocess, "run") as run:
            self.assertIn("fallback", view.prepare_view(self.library))
            run.assert_not_called()

    def test_private_propagation_precedes_bind_and_readonly_exec(self):
        flags = [SimpleNamespace(f_flag=view.ST_NOEXEC), SimpleNamespace(f_flag=view.ST_RDONLY)]
        with patch.object(view.os, "statvfs", side_effect=flags), \
                patch.object(view.shutil, "which", return_value="/bin/mount"), \
                patch.object(view, "unshare_mounts") as namespace, \
                patch.object(view.subprocess, "run") as run:
            self.assertEqual(view.prepare_view(self.library), "private readonly executable view")
            namespace.assert_called_once()
            self.assertEqual([call.args[0] for call in run.call_args_list], [
                ["/bin/mount", "--make-rprivate", "/"],
                ["/bin/mount", "--bind", str(self.library), str(self.library)],
                ["/bin/mount", "-o", "remount,bind,ro,exec,nosuid,nodev", str(self.library)],
            ])

    def test_partial_bind_is_undone_before_fallback(self):
        flags = [SimpleNamespace(f_flag=view.ST_NOEXEC), SimpleNamespace(f_flag=view.ST_NOEXEC)]
        with patch.object(view.os, "statvfs", side_effect=flags), \
                patch.object(view.shutil, "which", side_effect=lambda name: "/bin/" + name), \
                patch.object(view, "unshare_mounts"), \
                patch.object(view.subprocess, "run") as run:
            self.assertIn("fallback", view.prepare_view(self.library))
            self.assertEqual(run.call_args_list[-1].args[0], ["/bin/umount", str(self.library)])

    def test_failed_partial_cleanup_refuses_to_continue(self):
        flags = [SimpleNamespace(f_flag=view.ST_NOEXEC), SimpleNamespace(f_flag=view.ST_NOEXEC)]

        def run(arguments, **kwargs):
            if arguments[0].endswith("/umount"):
                raise view.subprocess.CalledProcessError(1, arguments)

        with patch.object(view.os, "statvfs", side_effect=flags), \
                patch.object(view.shutil, "which", side_effect=lambda name: "/bin/" + name), \
                patch.object(view, "unshare_mounts"), \
                patch.object(view.subprocess, "run", side_effect=run):
            with self.assertRaises(view.subprocess.CalledProcessError):
                view.prepare_view(self.library)

    def test_literal_exec_argv_replaces_python(self):
        with patch.object(view, "prepare_view", return_value="already executable"), \
                patch.object(view.os, "execvp") as execute:
            view.main(["--game-root", str(self.root), "--library-dir", "lib/arm64-v8a",
                       "--", "/bin/game", "literal;argument"])
            execute.assert_called_once_with("/bin/game", ["/bin/game", "literal;argument"])

    def test_non_linux_fallback_has_no_native_calls(self):
        with patch.object(view.sys, "platform", "darwin"), \
                patch.object(view, "unshare_mounts") as namespace, \
                patch.object(view.subprocess, "run") as run:
            self.assertIn("fallback", view.prepare_view(self.library))
            namespace.assert_not_called()
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
