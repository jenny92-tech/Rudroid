# INPUT:  Temporary PortMaster-Unity source trees and commands
# OUTPUT: Source selection, CLI forwarding and source verification checks
# POS:    Regression tests for the desktop Unity preparation entry
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS))
import unity_port
from pmunity_rudroid.check import port_check


@unittest.skipUnless(sys.version_info >= (3, 11), "PortMaster-Unity requires Python 3.11+")
class UnityPortTests(unittest.TestCase):
    def setUp(self):
        work = tempfile.TemporaryDirectory(prefix="unity port test ")
        self.addCleanup(work.cleanup)
        self.work = Path(work.name).resolve()
        self.source = self.work / f"portmaster-unity-{unity_port.REVISION}"
        module = self.source / "convert" / "pmunity"
        module.mkdir(parents=True)
        (module / "__init__.py").touch()
        (module / "__main__.py").write_text(
            "import json, pathlib, sys\n"
            "print(json.dumps([str(pathlib.Path.cwd()), *sys.argv[1:]]))\n"
            "sys.exit(27 if '--fail' in sys.argv else 0)\n",
            encoding="utf-8",
        )
        # An unrelated module in the caller's directory must never be imported.
        (self.work / "pmunity.py").write_text("raise RuntimeError('wrong module')\n", encoding="utf-8")

    def run_cli(self, *args, env=None):
        return subprocess.run(
            [sys.executable, str(TOOLS / "unity_port.py"), *args], cwd=self.work,
            env=env, capture_output=True, text=True,
        )

    def test_port_commands_keep_cwd_arguments_and_exit_status(self):
        for command in unity_port.PORT_COMMANDS:
            with self.subTest(command=command):
                result = self.run_cli("--source", self.source.name, command, "with spaces", "--fail")
                self.assertEqual(result.returncode, 27, result.stderr)
                self.assertEqual(json.loads(result.stdout), [str(self.work), "port", command, "with spaces", "--fail"])
                self.assertIn("unverified", result.stderr)

    def test_check_appends_its_hook_after_custom_hooks_only_once(self):
        module = self.source / "convert" / "pmunity" / "__main__.py"
        module.write_text(
            "from pmunity_rudroid.check import port_check\n"
            "import os\n"
            "print(os.environ['PMUNITY_PORT_HOOKS'])\n"
        )
        env = dict(os.environ, PMUNITY_SOURCE=str(self.source),
                   PMUNITY_PORT_HOOKS="custom.first,pmunity_rudroid.check,custom.second")
        result = self.run_cli("check", "Game", env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "custom.first,custom.second,pmunity_rudroid.check")

    def test_mono_check_distinguishes_route_profile_and_device_evidence(self):
        report = {
            "build": "windows", "backend": "mono", "unity_version": "2017.4.9f1",
            "unity_line": "2017.4", "scripting_runtime": ".NET 4.x", "route": "android",
            "routes": [{"route": "mono-arm64", "ok": True}], "notes": [],
            "code": {"assemblies": ["Example"], "pinvoke": {"SaveLib": ["Example"]},
                     "unreadable": ["Other.dll"]}, "plugins": [["Plugins/SaveLib.dll", "desktop"]],
        }
        port_check(report)
        result = report["rudroid"]
        self.assertEqual(report["route"], "android")
        self.assertEqual(result["route"], "mono-arm64")
        self.assertIsNone(result["player_profile_candidate"])
        self.assertIsNone(result["reference_title_same_engine_and_backend"])
        self.assertEqual(result["pinvoke_modules"], {"SaveLib": ["Example"]})
        self.assertEqual(result["native_plugins"], ["Plugins/SaveLib.dll"])
        self.assertIn("incomplete", " ".join(report["notes"]))
        report["routes"][0].update(ok=False, why=["incompatible class libraries"])
        port_check(report)
        self.assertIsNone(report["rudroid"]["route"])
        self.assertEqual(report["rudroid"]["reasons"], ["incompatible class libraries"])
        report["routes"][0]["ok"] = True
        report["code"] = {}
        port_check(report)
        self.assertIsNone(report["rudroid"]["route"])
        self.assertIsNone(report["rudroid"]["pinvoke_modules"])

    def test_apk_uses_inspected_abi_and_backend(self):
        for abi, backend, expected in [
            ("arm64-v8a", "il2cpp", "apk"), ("armeabi-v7a", "il2cpp", None),
            ("arm64-v8a", "mono", None),
        ]:
            with self.subTest(abi=abi, backend=backend):
                report = {"build": "android", "backend": backend, "notes": [],
                          "routes": [{"route": "apk", "ok": True}],
                          "abis": ["armeabi-v7a", "arm64-v8a"],
                          "apk_info": {"abi": abi, "plugins": ["libgame.so"]}}
                port_check(report)
                self.assertEqual(report["rudroid"]["route"], expected)
                self.assertEqual(report["rudroid"]["native_plugins"], ["libgame.so"])
                self.assertIsNone(report["rudroid"]["pinvoke_modules"])

    def test_preparation_commands_use_environment_and_forward_options(self):
        env = dict(os.environ, PMUNITY_SOURCE=self.source.name)
        for args in (
            ["player", "fetch", "6000.0.66f2", "Android player", "--installer", "Unity Setup.exe"],
            ["convert", "Game_Data", "Output Overlay", "--set", "settings.graphics_apis=[11]"],
            ["android-data", "--src", "Game Data", "--managed-file", "a b.dll", "--managed-file", "c.dll"],
        ):
            with self.subTest(command=args[0]):
                result = self.run_cli(*args, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), [str(self.work), *args])
        self.assertFalse(list(self.source.rglob("__pycache__")))

    def prepare_fixture(self):
        data = self.work / "Original Data"
        player = self.work / "Android player"
        config = self.work / "Game recipe.json"
        override = self.work / "Patched Assembly.dll"
        output = self.work / "Android output"

        (data / "Managed").mkdir(parents=True)
        (data / "globalgamemanagers").write_bytes(b"original settings")
        (data / "Managed" / "Game.dll").write_bytes(b"desktop assembly")
        (data / "Unconverted").write_bytes(b"unchanged file")
        (self.work / "goggame-000.info").write_bytes(b"title metadata")
        config.write_text("{}", encoding="utf-8")
        override.write_bytes(b"patched assembly")
        il2cpp = player / "Variations" / "il2cpp"
        arm64 = il2cpp / "Release" / "Libs" / "arm64-v8a"
        arm64.mkdir(parents=True)
        (arm64 / "libunity.so").write_bytes(b"\x7fELF\x02\x01" + b"\0" * 12 + b"\xb7\0")
        (il2cpp / "Managed").mkdir()
        (il2cpp / "Managed" / "UnityEngine.dll").write_bytes(b"android assembly")

        (self.source / "convert" / "pmunity" / "__main__.py").write_text(
            """import json
import os
from pathlib import Path
import sys
import zipfile

command, *args = sys.argv[1:]
scenario = os.environ.get("FIXTURE_SCENARIO")

def arg(name):
    return args[args.index(name) + 1]

if command == "convert":
    assert arg("--steps") == "shaders,settings,audio,quality"
    assert arg("--set") == "settings.graphics_apis=[11]"
    assert json.loads(Path(arg("--config")).read_text()) == {}
    data = Path(args[0])
    assert data.name == "Original Data" and data.parent != Path.cwd()
    assert (data / "Unconverted").read_bytes() == b"unchanged file"
    (data / "globalgamemanagers").write_bytes(b"converted settings")
    if scenario == "convert-error":
        sys.exit(27)
    sys.exit(0)

assert command == "android-data" and "--mono64" in args and arg("--pack") == "all"
assert arg("--streaming-assets") == "both" and "--data-path" in args
data = Path(arg("--src"))
player = Path(arg("--pkg"))
override = Path(arg("--managed-file"))
assert override.parent != Path.cwd()
assert (data.parent / "goggame-000.info").read_bytes() == b"title metadata"
assert (data / "globalgamemanagers").read_bytes() == b"converted settings"
assert player != Path.cwd() / "Android player"
stage = Path(arg("--out"))
archives = stage / "zip"
archives.mkdir(parents=True)
if scenario == "pack-error":
    sys.exit(17)
if scenario == "no-artifacts":
    sys.exit(0)
(archives / "Game_Data").mkdir()
name, package = arg("--name"), arg("--package")
apk_path = archives / (name + ".apk")
manifest = {
    "package": package, "apk": apk_path.name, "abi": "arm64-v8a", "scripting": "mono",
    "unity_version": "6000.0.66f2",
    "build_id": "build", "version_code": 1, "meta_data": {"unity.build-id": "build"},
    "data_path": "Game_Data", "streaming_assets": {"mode": "both", "folder": "Game_Data/StreamingAssets"},
}
(archives / "android_package.json").write_text(json.dumps(manifest))
report = {"verify": {"problems": [] if scenario != "verify-error" else [["asset", "wrong platform"]]},
          "pack": {"apk": str(apk_path)}}
(stage / "android_data_report.json").write_text(json.dumps(report))
files = {
    "globalgamemanagers": (data / "globalgamemanagers").read_bytes(),
    "Managed/mscorlib.dll": b"mscorlib assembly",
    "Managed/Patched Assembly.dll": override.read_bytes(),
    "Resources/unity_builtin_extra": b"built-in extras",
    "Resources/unity default resources": b"built-in resources",
}
with zipfile.ZipFile(apk_path, "w") as apk:
    for rel, payload in files.items():
        member = "assets/bin/Data/" + rel
        file = stage / name / member
        file.parent.mkdir(parents=True, exist_ok=True)
        if rel == "Managed/Patched Assembly.dll":
            os.link(override, file)
        else:
            file.write_bytes(payload)
        if scenario != "incomplete-apk" or rel != "globalgamemanagers":
            apk.writestr(member, payload)
if scenario == "destination-race":
    Path("Android output").mkdir()
""",
            encoding="utf-8",
        )

        args = [
            "--source", str(self.source), "prepare",
            "--src", str(data), "--pkg", str(player), "--out", str(output),
            "--name", "Test Game", "--package", "com.test.game", "--config", str(config),
            "--steps", "shaders,settings,audio,quality", "--set", "settings.graphics_apis=[11]",
            "--managed-file", str(override), "--streaming-assets", "both", "--data-path",
        ]
        return SimpleNamespace(data=data, player=player, override=override, output=output, args=args)

    def test_prepare_packages_a_complete_copy_and_keeps_inputs_independent(self):
        fixture = self.prepare_fixture()
        result = self.run_cli(*fixture.args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((fixture.data / "globalgamemanagers").read_bytes(), b"original settings")
        apk = fixture.output / "zip" / "Test Game.apk"
        report = json.loads((fixture.output / "android_data_report.json").read_text())
        self.assertEqual(Path(report["pack"]["apk"]), apk)
        packaged_override = fixture.output / "Test Game/assets/bin/Data/Managed/Patched Assembly.dll"
        packaged_override.write_bytes(b"local edit")
        self.assertEqual(fixture.override.read_bytes(), b"patched assembly")
        self.assertFalse(list(self.work.glob(".rudroid-prepare-*")))

    def test_prepare_does_not_publish_when_a_step_fails_or_produces_incomplete_data(self):
        fixture = self.prepare_fixture()
        for scenario, code in (
            ("convert-error", 27), ("pack-error", 17), ("no-artifacts", 2), ("verify-error", 2),
            ("incomplete-apk", 2), ("destination-race", 2),
        ):
            with self.subTest(scenario=scenario):
                result = self.run_cli(*fixture.args, env=dict(os.environ, FIXTURE_SCENARIO=scenario))
                self.assertEqual(result.returncode, code, result.stderr)
                if scenario == "destination-race":
                    self.assertTrue(fixture.output.is_dir())
                    self.assertFalse(list(fixture.output.iterdir()))
                else:
                    self.assertFalse(fixture.output.exists())
                self.assertEqual((fixture.data / "globalgamemanagers").read_bytes(), b"original settings")
                self.assertFalse(list(self.work.glob(".rudroid-prepare-*")))

    def test_prepare_rejects_existing_overlapping_and_previously_converted_data(self):
        fixture = self.prepare_fixture()
        fixture.output.mkdir()
        result = self.run_cli(*fixture.args)
        self.assertIn("Output already exists", result.stderr)
        overlapping = fixture.args.copy()
        overlapping[overlapping.index("--out") + 1] = str(fixture.data / "new-output")
        result = self.run_cli(*overlapping)
        self.assertIn("Input and output overlap", result.stderr)
        fixture.output.rmdir()
        (fixture.data / ".pmunity.json").write_text("{}", encoding="utf-8")
        result = self.run_cli(*fixture.args)
        self.assertIn("without a pmunity conversion journal", result.stderr)
        self.assertFalse(fixture.output.exists())

    def test_explicit_source_overrides_environment(self):
        env = dict(os.environ, PMUNITY_SOURCE="missing source")
        result = self.run_cli("--source", self.source.name, "check", "Game Data", env=env)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_new_preserves_existing_upstream_hooks(self):
        module = self.source / "convert" / "pmunity" / "__main__.py"
        module.write_text(
            "import os\n"
            "print(os.environ['PMUNITY_PORT_HOOKS'])\n",
            encoding="utf-8",
        )
        env = dict(os.environ, PMUNITY_PORT_HOOKS="local,custom.extension")
        result = self.run_cli("new", "game", env=dict(env, PMUNITY_SOURCE=self.source.name))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "local,custom.extension")

    def test_checkout_requires_pinned_revision_but_allows_local_changes(self):
        (self.source / ".git").touch()
        with patch.object(unity_port, "git", return_value="wrong revision"):
            with self.assertRaisesRegex(ValueError, "wrong revision"):
                with unity_port.source_tree(self.source):
                    self.fail("Wrong revision was accepted")
        with patch.object(unity_port, "git", side_effect=[unity_port.REVISION, " M convert/pmunity/__main__.py"]):
            with patch("sys.stderr") as stderr, unity_port.source_tree(self.source) as source:
                self.assertEqual(source, self.source)
            self.assertIn("local changes (unverified)", stderr.write.call_args_list[0].args[0])

    def test_archive_is_verified_before_extraction_and_cleaned_up(self):
        archive = self.work / "source archive.tar.gz"
        with tarfile.open(archive, "w:gz") as snapshot:
            snapshot.add(self.source, arcname=self.source.name)
        result = self.run_cli("--source", str(archive), "check", "game")
        self.assertEqual(result.returncode, 2)
        self.assertIn("archive SHA-256", result.stderr)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        with patch.object(unity_port, "ARCHIVE_SHA256", digest):
            with unity_port.source_tree(archive) as source:
                self.assertTrue((source / "convert" / "pmunity" / "__main__.py").is_file())
            self.assertFalse(source.parent.exists())


if __name__ == "__main__":
    unittest.main()
