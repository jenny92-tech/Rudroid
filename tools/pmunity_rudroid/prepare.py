# INPUT: Desktop Mono Unity data, local Android player and a conversion recipe
# OUTPUT: A verified PortMaster-Unity android-data directory published once
# POS: Rudroid host preparation around the upstream converter and packer
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import tempfile
import zipfile

if __package__.startswith("tools."):
    from ..unity_assets import _inventory, _publish_directory
else:
    from unity_assets import _inventory, _publish_directory


def parse_args(argv):
    parser = argparse.ArgumentParser(
        prog="unity_port.py prepare",
        description="Prepare desktop Mono data for a local ARM64 IL2CPP Android player.",
        allow_abbrev=False,
    )
    parser.add_argument("--src", type=Path, required=True, help="original desktop Unity *_Data directory")
    parser.add_argument("--pkg", type=Path, required=True, help="prepared ARM64 IL2CPP Android player directory")
    parser.add_argument("--name", required=True, help="game name used for the output directory and APK")
    parser.add_argument("--package", required=True, help="Android package identifier")
    parser.add_argument("--out", type=Path, required=True, help="new destination directory")
    parser.add_argument("--config", type=Path, required=True, help="upstream convert JSON recipe")
    parser.add_argument("--steps", help="comma-separated upstream convert steps")
    parser.add_argument("--set", dest="overrides", action="append", default=[], metavar="SECTION.KEY=VALUE")
    parser.add_argument("--managed-file", type=Path, action="append", default=[], metavar="DLL")
    parser.add_argument("--streaming-assets", choices=("obb", "both", "loose"), default="obb")
    parser.add_argument("--data-path", action="store_true")
    return parser.parse_args(argv)


def _copy_tree(source, destination):
    # Upstream may hard-link unchanged files into the published tree. Pass
    # independent copies so those links cannot reach the caller's originals.
    shutil.copytree(source, destination)
    for root, _, files in os.walk(destination):
        path = Path(root)
        path.chmod(path.stat().st_mode | stat.S_IWUSR)
        for name in files:
            file = path / name
            if not file.is_symlink():
                file.chmod(file.stat().st_mode | stat.S_IWUSR)


def _validate_package(stage, output, args, overrides):
    report_path = stage / "android_data_report.json"
    report = json.loads(report_path.read_text(encoding="utf-8"))
    problems = report.get("verify", {}).get("problems")
    if problems != []:
        raise ValueError(f"Upstream android-data verification did not pass: {problems!r}")

    archives = stage / "zip"
    manifest = json.loads((archives / "android_package.json").read_text(encoding="utf-8"))
    build_id = manifest.get("build_id")
    unity_version = manifest.get("unity_version")
    expected = {
        "package": args.package, "apk": f"{args.name}.apk", "abi": "arm64-v8a",
        "scripting": "mono", "version_code": 1,
    }
    if (any(manifest.get(key) != value for key, value in expected.items())
            or not isinstance(build_id, str) or not build_id
            or not isinstance(unity_version, str) or not unity_version.strip()
            or manifest.get("meta_data", {}).get("unity.build-id") != build_id):
        raise ValueError("Upstream android_package.json does not describe the requested ARM64 Mono package")

    data_dir = Path("assets/bin/Data")
    tree = stage / args.name / data_dir
    required = [
        tree / "globalgamemanagers",
        tree / "Managed" / "mscorlib.dll",
        tree / "Resources" / "unity_builtin_extra",
    ]
    for dll in (tree / "Managed").glob("*.dll"):
        if dll not in required:
            required.append(dll)
    required.extend(path for path in (
        tree / "Resources" / "unity default resources", tree / "unity default resources",
    ) if path.is_file())
    if (not (tree / "Managed" / "mscorlib.dll").is_file()
            or not any(path.name == "unity default resources" for path in required)):
        raise ValueError("Upstream android-data tree is missing managed assemblies (mscorlib.dll) or default resources")

    pack = report["pack"]
    with zipfile.ZipFile(archives / expected["apk"]) as apk:
        for file in required:
            member = file.relative_to(stage / args.name).as_posix()
            if not file.is_file() or not file.stat().st_size or apk.getinfo(member).file_size != file.stat().st_size:
                raise ValueError(f"Missing or incomplete data in upstream android-data tree or APK: {member}")
            with file.open("rb") as tree_file, apk.open(member) as apk_file:
                if hashlib.file_digest(tree_file, "sha256").digest() != hashlib.file_digest(apk_file, "sha256").digest():
                    raise ValueError(f"Tree and APK disagree on required data: {member}")
        for override in overrides:
            loose = tree / "Managed" / override.name
            if not loose.is_file():
                raise ValueError(f"Loose Managed tree is missing supplied assembly: {override.name}")
            with override.open("rb") as supplied:
                expected_digest = hashlib.file_digest(supplied, "sha256").digest()
            with loose.open("rb") as loose_file:
                if hashlib.file_digest(loose_file, "sha256").digest() != expected_digest:
                    raise ValueError(f"Loose Managed tree does not contain the supplied assembly: {override.name}")
            with apk.open(f"{data_dir.as_posix()}/Managed/{override.name}") as packed:
                if hashlib.file_digest(packed, "sha256").digest() != expected_digest:
                    raise ValueError(f"APK does not contain the supplied managed assembly: {override.name}")

    for kind in ("main", "patch"):
        key = f"{kind}_obb"
        if key in manifest or key in pack:
            expected_obb = f"{kind}.1.{args.package}.obb"
            if manifest.get(key) != expected_obb:
                raise ValueError(f"Invalid {key} entry in upstream android_package.json")
            with zipfile.ZipFile(archives / expected_obb) as obb:
                obb.getinfo(f"assets/{build_id}")

    if args.data_path and (manifest.get("data_path") != "Game_Data" or not (archives / "Game_Data").is_dir()):
        raise ValueError("Upstream package is missing the requested loose Game_Data directory")
    if args.streaming_assets != "obb" and manifest.get("streaming_assets") != {
        "mode": args.streaming_assets, "folder": "Game_Data/StreamingAssets",
    }:
        raise ValueError("Upstream package is missing the requested StreamingAssets mode")

    # The upstream manifest is portable. Its diagnostic report records absolute
    # archive paths, which must point at the published tree after the move.
    for key in ("apk", "main_obb", "patch_obb"):
        if key in manifest:
            pack[key] = str(output / "zip" / manifest[key])
    report_path.write_text(json.dumps(report, indent=1), encoding="utf-8")


def prepare(args, upstream_source, run):
    output = args.out.parent.resolve() / args.out.name
    if output.exists() or output.is_symlink():
        raise ValueError(f"Output already exists: {output}")
    if (not args.name or not args.name.strip() or args.name.startswith(".")
            or any(character in args.name for character in "/\\\0")
            or args.name.casefold() in {"zip", "android_data_report.json", "make_android_data.log", "plugins.map"}):
        raise ValueError("--name must be a single game name distinct from upstream output names")
    if not args.package or not args.package.strip() or any(character in args.package for character in "/\\\0"):
        raise ValueError("--package must be a package identifier without path separators")

    for raw_input in (args.src, args.pkg, args.config, *args.managed_file):
        if raw_input.is_symlink():
            raise ValueError(f"Symlink inputs are not allowed: {raw_input}")
    source, player, config = (path.resolve(strict=True) for path in (args.src, args.pkg, args.config))
    managed = [file.resolve(strict=True) for file in args.managed_file]
    if not output.parent.is_dir():
        raise ValueError(f"Output parent is not a directory: {output.parent}")
    for path in (source, player, config, upstream_source, *managed):
        # samefile also detects alternate casing on case-insensitive filesystems.
        if path.is_relative_to(output) or any(parent.samefile(path) for parent in output.parents):
            raise ValueError(f"Input and output overlap: {path} and {output}")
    if not source.is_dir() or not (source / "globalgamemanagers").is_file() or not any(
        file.is_file() for file in (source / "Managed").glob("*.dll")
    ):
        raise ValueError(f"Expected complete, unpacked desktop Mono data: {source}")
    if not config.is_file():
        raise ValueError(f"Expected a convert JSON recipe file: {config}")
    if any(not file.is_file() or original.suffix != ".dll" for original, file in zip(args.managed_file, managed)):
        raise ValueError("--managed-file requires an existing .dll file")
    if len({file.name.casefold() for file in args.managed_file}) != len(managed):
        raise ValueError("--managed-file names must be distinct (case-insensitive)")

    il2cpp = player / "Variations" / "il2cpp"
    player_unity = il2cpp / "Release" / "Libs" / "arm64-v8a" / "libunity.so"
    if not player_unity.is_file() or not any(file.is_file() for file in (il2cpp / "Managed").glob("*.dll")):
        raise ValueError(f"Expected a prepared ARM64 IL2CPP Android player with managed engine modules: {player}")
    with player_unity.open("rb") as handle:
        header = handle.read(20)
    if not header.startswith(b"\x7fELF\x02\x01") or header[18:20] != b"\xb7\x00":
        raise ValueError(f"Android player libunity.so is not an AArch64 ELF: {player_unity}")

    # Reject symlinks/special files and detect concurrent changes while taking
    # the two working copies. A copied journal could replay outside the scratch.
    with tempfile.TemporaryDirectory(prefix=".rudroid-prepare-", dir=output.parent) as work:
        work = Path(work)
        data = work / "desktop" / source.name
        isolated_player = work / "player"
        for original, copy in ((source, data), (player, isolated_player)):
            before = _inventory(original)
            if original == source and Path(".pmunity.json") in before:
                raise ValueError(f"Expected original data without a pmunity conversion journal: {source}")
            _copy_tree(original, copy)
            if before != _inventory(original):
                raise ValueError(f"Input changed while taking a working copy: {original}")
        for info in source.parent.glob("goggame-*.info"):
            if info.is_symlink():
                raise ValueError(f"Symlink metadata file is not allowed: {info}")
            if info.is_file():
                shutil.copyfile(info, data.parent / info.name)

        conversion = ["convert", str(data), "--config", str(config)]
        if args.steps is not None:
            conversion.extend(("--steps", args.steps))
        for override in args.overrides:
            conversion.extend(("--set", override))
        status = run(conversion)
        if status:
            return status

        stage = work / "package"
        packaging = [
            "android-data", "--src", str(data), "--pkg", str(isolated_player),
            "--name", args.name, "--package", args.package, "--out", str(stage),
            "--mono64", "--pack", "all", "--streaming-assets", args.streaming_assets,
        ]
        if args.data_path:
            packaging.append("--data-path")
        overrides = []
        if managed:
            (work / "managed").mkdir()
            for original, file in zip(args.managed_file, managed):
                override = work / "managed" / original.name
                shutil.copyfile(file, override)
                overrides.append(override)
                packaging.extend(("--managed-file", str(override)))
        status = run(packaging)
        if status:
            return status
        try:
            _validate_package(stage, output, args, overrides)
        except (FileNotFoundError, KeyError, TypeError, AttributeError, zipfile.BadZipFile) as exc:
            raise ValueError(f"Upstream android-data did not produce a complete package: {exc}") from exc
        _publish_directory(stage, output)
    print(f"Prepared ARM64 Mono Android data: {output}")
    return 0
