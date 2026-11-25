#!/usr/bin/env python3
# INPUT:  pinned malisx2/Mesa/crate archives and the existing fixed target cache
# OUTPUT:  verified offline Linux build inputs or an audited Release ICD bundle
# POS:    prepares the optional third-party driver without installing tools or changing system drivers
"""Prepare data before Docker; verify/build/package offline inside the fixed image."""
import argparse
import configparser
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

REVISION = "d0422ed5eb5d8971f118f14d2550ce04aa1a0a5a"
MESA_REVISION = "81113fce01b0e3974cdbb2683449e8619d0639e0"
SOURCES = {
    "malisx2": ("bmdhacks/malisx2", REVISION,
                "157df6d7cbb9922a61f280551adbc732475403073cd17d7407f6e4535e93035d"),
    "mesa": ("bmdhacks/armsx2-turnip", MESA_REVISION,
             "64547b81af15ad80f648a088cd1a80fa94768ed8821cf7ed93e25d2a744a21ad"),
}
WRAPS = {"bitflags-2-rs", "paste-1-rs", "proc-macro2-1-rs", "quote-1-rs",
         "rustc-hash-2-rs", "syn-2-rs", "unicode-ident-1-rs", "xml-rs"}
MAX_ARCHIVE = 160 * 1024 * 1024
MAX_EXPANDED = 512 * 1024 * 1024
C11_PATCH = {
    "path": "src/vk/mali_shader.c", "patch_file": "malisx2-c11-label.patch",
    "patch_sha256": "9df2d29090d4ac8933e846762025b03ba575b7b63cd4a05a0300374e0b7c0ed2",
    "original_sha256": "d580d5d98ec328ea73c731213fbd281381617e9a2f5988f3ce81a0174320a452",
    "patched_sha256": "63e060fb59591aabfd59d4e028ae7fdf73802a657982056ca711da4c396dbf2b",
    "purpose": "GCC 10 C11 label before declaration: add an empty statement; no algorithm change",
}


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def no_link_ancestors(path):
    for item in (path.absolute(), *path.absolute().parents):
        if item.is_symlink():
            raise ValueError("cache path contains a symlink: " + str(item))


def check_output(runtime):
    """Check before mkdir/strip, and again before publishing metadata."""
    no_link_ancestors(runtime)
    for name in ["libvulkan_malisx2.so", "mali_icd.json", "sources.json", "linux-abi.json", "licenses", C11_PATCH["patch_file"]]:
        no_link_ancestors(runtime / name)


def relative(name):
    if not name or "\\" in name or "\0" in name:
        raise ValueError("unsupported source path: " + repr(name))
    parts = name.rstrip("/").split("/")
    if any(part in ("", ".", "..") for part in parts) or PurePosixPath(name).is_absolute():
        raise ValueError("unsafe source path: " + name)
    return PurePosixPath(*parts)


def link_target(name, target):
    if not target or target.startswith("/") or "\\" in target:
        raise ValueError("unsafe source link: " + name)
    parts = list(PurePosixPath(name).parent.parts)
    for part in target.split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            if not parts:
                raise ValueError("source link escapes archive: " + name)
            parts.pop()
        else:
            parts.append(part)
    return "/".join(parts)


def inventory(root, ignored=(), empty_metadata=()):
    no_link_ancestors(root)
    result = {}
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in list(dirs) + files:
            path = Path(directory) / name
            name = path.relative_to(root).as_posix()
            if name in empty_metadata:
                # Meson's source/subprojects lock is not source code. Only
                # this declared exact path may differ from the pinned archive.
                info = path.lstat()
                if not stat.S_ISREG(info.st_mode) or info.st_size != 0:
                    raise ValueError("invalid empty build metadata: " + name)
                continue
            if name in ignored:
                no_link_ancestors(path)
                if path.name in dirs:
                    dirs.remove(path.name)
                continue
            if path.is_symlink():
                target = os.readlink(path)
                link_target(name, target)
                try:
                    resolved = path.resolve(strict=True)
                except (OSError, RuntimeError) as error:
                    raise ValueError("invalid cached source link: " + name) from error
                if not resolved.is_relative_to(root.resolve()):
                    raise ValueError("cached source link escapes root: " + name)
                result[name] = {"link": target}
            elif path.is_dir():
                result[name] = {"directory": True}
            elif path.is_file():
                result[name] = {"sha256": digest(path), "executable": bool(path.stat().st_mode & 0o111)}
            else:
                raise ValueError("nonregular cached source: " + name)
    return result


def source_archive(archive, destination, expected, overlays=None, ignored=(), empty_metadata=()):
    """Validate all metadata before writes; internal links are created last."""
    no_link_ancestors(archive)
    no_link_ancestors(destination)
    if digest(archive) != expected:
        raise ValueError("source archive checksum mismatch: " + archive.name)
    overlays = overlays or {}
    with tarfile.open(archive) as tar:
        members = tar.getmembers()
        if len(members) > 50000 or any(m.size < 0 for m in members) or sum(m.size for m in members) > MAX_EXPANDED:
            raise ValueError("source archive exceeds extraction budget")
        roots = {relative(m.name).parts[0] for m in members}
        if len(roots) != 1:
            raise ValueError("source archive must have one root")
        entries, expected_files = {}, {}
        for member in members:
            parts = relative(member.name).parts[1:]
            if not parts:
                if not member.isdir():
                    raise ValueError("source root is not a directory")
                continue
            name = PurePosixPath(*parts).as_posix()
            if name in entries:
                raise ValueError("duplicate source archive entry: " + name)
            if not (member.isdir() or member.isfile() or member.issym()):
                raise ValueError("unsupported source archive entry: " + name)
            entries[name] = member
            if member.isdir():
                expected_files[name] = {"directory": True}
            elif member.issym():
                link_target(name, member.linkname)
                expected_files[name] = {"link": member.linkname}
            else:
                with tar.extractfile(member) as stream:
                    h = hashlib.sha256()
                    for block in iter(lambda: stream.read(1024 * 1024), b""):
                        h.update(block)
                expected_files[name] = {"sha256": h.hexdigest(), "executable": bool(member.mode & 0o111)}
        for name in entries:
            if any(str(parent) in entries and not entries[str(parent)].isdir()
                   for parent in PurePosixPath(name).parents if str(parent) != "."):
                raise ValueError("source entry has a nondirectory parent: " + name)
        for name, member in entries.items():
            if member.issym() and link_target(name, member.linkname) not in entries:
                raise ValueError("source link target is missing: " + name)
        for name, data in overlays.items():
            relative(name)
            expected_files[name] = {"sha256": hashlib.sha256(data).hexdigest(), "executable": False}
        # Some crate archives omit explicit parent directory entries.
        for name in list(expected_files):
            for parent in PurePosixPath(name).parents:
                if str(parent) != ".":
                    expected_files.setdefault(str(parent), {"directory": True})
        if destination.exists():
            if inventory(destination, ignored, empty_metadata) != expected_files:
                raise ValueError("cached source changed: " + str(destination))
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix="source-", dir=destination.parent) as temporary:
                staging = Path(temporary)
                for name, member in entries.items():
                    path = staging / name
                    if member.isdir():
                        path.mkdir(parents=True, exist_ok=True)
                    elif member.isfile():
                        path.parent.mkdir(parents=True, exist_ok=True)
                        with tar.extractfile(member) as stream, path.open("wb") as out:
                            shutil.copyfileobj(stream, out)
                        path.chmod(0o755 if member.mode & 0o111 else 0o644)
                for name, data in overlays.items():
                    path = staging / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(data)
                    path.chmod(0o644)
                for name, member in entries.items():
                    if member.issym():
                        (staging / name).symlink_to(member.linkname)
                if inventory(staging) != expected_files:
                    raise ValueError("extracted source inventory mismatch")
                os.replace(staging, destination)
    tree = hashlib.sha256(json.dumps(expected_files, sort_keys=True).encode()).hexdigest()
    return {"archive_sha256": expected, "tree_sha256": tree, "entries": len(expected_files)}


def acquire(cache, filename, url, expected, offline):
    path = cache / filename
    no_link_ancestors(path)
    if not path.exists():
        if offline:
            raise ValueError("missing verified offline input: " + filename)
        with tempfile.NamedTemporaryFile(prefix="download-", dir=cache, delete=False) as out:
            temporary = Path(out.name)
            try:
                size = 0
                with urllib.request.urlopen(url, timeout=45) as response:
                    while block := response.read(1024 * 1024):
                        size += len(block)
                        if size > MAX_ARCHIVE:
                            raise ValueError("source archive exceeds download budget")
                        out.write(block)
                out.flush()
                if digest(temporary) != expected:
                    raise ValueError("download checksum mismatch: " + filename)
                os.replace(temporary, path)
            finally:
                if temporary.exists():
                    temporary.unlink()
    if digest(path) != expected:
        raise ValueError("cached archive changed: " + filename)
    return path


def recipes(archive):
    result = {}
    with tarfile.open(archive) as tar:
        for member in tar:
            name = PurePosixPath(member.name)
            if len(name.parts) != 3 or name.parts[1] != "subprojects" or name.suffix != ".wrap":
                continue
            config = configparser.ConfigParser(interpolation=None)
            config.read_string(tar.extractfile(member).read().decode())
            if config.sections() != ["wrap-file"]:
                raise ValueError("unexpected Rust wrap recipe")
            wrap = dict(config["wrap-file"])
            if set(wrap) != {"directory", "source_url", "source_filename", "source_hash", "patch_directory"}:
                raise ValueError("unsupported Rust wrap fields")
            for field in ["directory", "source_filename", "patch_directory"]:
                if len(relative(wrap[field]).parts) != 1:
                    raise ValueError("unsafe Rust wrap path")
            if not re.fullmatch(r"https://static\.crates\.io/crates/[A-Za-z0-9_-]+/[0-9.]+/download", wrap["source_url"]) or not re.fullmatch("[0-9a-f]{64}", wrap["source_hash"]):
                raise ValueError("unverified Rust wrap source")
            result[name.stem] = wrap
    if set(result) != WRAPS:
        raise ValueError("unexpected Rust wrap dependency set")
    return result


def c11_overlay(archive):
    patch = Path(__file__).with_name(C11_PATCH["patch_file"])
    if digest(patch) != C11_PATCH["patch_sha256"]:
        raise ValueError("C11 compatibility patch changed")
    with tarfile.open(archive) as tar:
        members = [m for m in tar if "/".join(PurePosixPath(m.name).parts[1:]) == C11_PATCH["path"]]
        if len(members) != 1 or not members[0].isfile():
            raise ValueError("C11 compatibility source is missing")
        data = tar.extractfile(members[0]).read()
    before, after = b"found:\n\n   nir_builder b", b"found: ;\n\n   nir_builder b"
    if hashlib.sha256(data).hexdigest() != C11_PATCH["original_sha256"] or data.count(before) != 1:
        raise ValueError("C11 compatibility original source changed")
    patched = data.replace(before, after)
    if hashlib.sha256(patched).hexdigest() != C11_PATCH["patched_sha256"]:
        raise ValueError("C11 compatibility patched source identity mismatch")
    return {C11_PATCH["path"]: patched}


def write_receipt(cache, receipt):
    output = cache / "inputs.json"
    no_link_ancestors(output)
    content = json.dumps(receipt, sort_keys=True, indent=2) + "\n"
    if not output.exists() or output.read_text() != content:
        fd, temp = tempfile.mkstemp(prefix="receipt-", dir=cache)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as temporary:
                temporary.write(content)
                temporary.flush()
                os.fsync(temporary.fileno())
            os.replace(temp, output)
        except BaseException:
            Path(temp).unlink(missing_ok=True)
            raise


def prepare(cache, offline=False):
    path = cache / "source" / C11_PATCH["path"]
    no_link_ancestors(path)
    if not offline and path.is_file() and digest(path) == C11_PATCH["original_sha256"]:
        # An old cache may migrate only after every original component AND its
        # canonical receipt passes the same complete read-only verification.
        receipt = _prepare_inputs(cache, offline=True, patched=False)
        archive = cache / f"malisx2-{REVISION}.tar.gz"
        overlays = c11_overlay(archive)
        no_link_ancestors(path)
        if digest(path) != C11_PATCH["original_sha256"]:
            raise ValueError("C11 source changed during verified migration")
        fd, temp = tempfile.mkstemp(prefix="c11-", dir=path.parent)
        try:
            with os.fdopen(fd, "wb") as temporary:
                temporary.write(overlays[C11_PATCH["path"]])
                temporary.flush()
                os.fsync(temporary.fileno())
                os.fchmod(temporary.fileno(), 0o644)
            os.replace(temp, path)
        except BaseException:
            Path(temp).unlink(missing_ok=True)
            raise
        wraps = recipes(archive)
        ignored = ["mesa", *["subprojects/" + w["directory"] for w in wraps.values()]]
        receipt["malisx2"].update(source_archive(archive, cache / "source", SOURCES["malisx2"][2],
            overlays=overlays, ignored=ignored, empty_metadata=("subprojects/.wraplock",)))
        receipt["linux_build_patch"] = dict(C11_PATCH)
        write_receipt(cache, receipt)
        print("Verified canonical source cache migrated with the single C11 label overlay")
        return receipt
    return _prepare_inputs(cache, offline=offline, patched=True)


def _prepare_inputs(cache, offline=False, patched=True):
    no_link_ancestors(cache)
    if offline and not cache.is_dir():
        raise ValueError("missing verified offline input cache")
    if not offline:
        cache.mkdir(parents=True, exist_ok=True)
    archives, receipt = {}, {}
    for name, (repository, revision, expected) in SOURCES.items():
        url = f"https://codeload.github.com/{repository}/tar.gz/{revision}"
        archives[name] = acquire(cache, f"{name}-{revision}.tar.gz", url, expected, offline)
        receipt[name] = {"url": url, "revision": revision}
    wraps = recipes(archives["malisx2"])
    source = cache / "source"
    ignored = ["mesa", *["subprojects/" + w["directory"] for w in wraps.values()]]
    if offline and any(not (source / name).is_dir() for name in [".", *ignored]):
        raise ValueError("missing prepared offline source tree; run --cache before Docker")
    overlays = c11_overlay(archives["malisx2"]) if patched else {}
    receipt["malisx2"].update(source_archive(archives["malisx2"], source, SOURCES["malisx2"][2], overlays=overlays,
                                           ignored=ignored, empty_metadata=("subprojects/.wraplock",)))
    receipt["mesa"].update(source_archive(archives["mesa"], source / "mesa", SOURCES["mesa"][2]))
    receipt["crates"] = {}
    for name, wrap in sorted(wraps.items()):
        archive = acquire(cache, wrap["source_filename"], wrap["source_url"], wrap["source_hash"], offline)
        patch = source / "subprojects/packagefiles" / wrap["patch_directory"]
        overlays = {p.relative_to(patch).as_posix(): p.read_bytes() for p in patch.rglob("*") if p.is_file()}
        if not overlays or any(p.is_symlink() for p in patch.rglob("*")):
            raise ValueError("missing or linked Rust wrap overlay: " + name)
        item = source_archive(archive, source / "subprojects" / wrap["directory"], wrap["source_hash"], overlays)
        receipt["crates"][name] = {"url": wrap["source_url"], **item}
    if patched:
        receipt["linux_build_patch"] = dict(C11_PATCH)
    content = json.dumps(receipt, sort_keys=True, indent=2) + "\n"
    output = cache / "inputs.json"
    if offline:
        if not output.is_file() or output.is_symlink() or output.read_text() != content:
            raise ValueError("offline source receipt changed or missing")
    else:
        write_receipt(cache, receipt)
    print("malisx2 Linux inputs verified; no source code executed or tools installed")
    return receipt


def preflight(output):
    if platform.system() != "Linux" or platform.machine() not in ("aarch64", "arm64"):
        raise ValueError("malisx2 build requires the fixed Linux ARM64 image")
    if sys.version_info < (3, 10):
        raise ValueError("Mesa build generators require Python >=3.10; rebuild the fixed CPython 3.11 image")
    commands = {"meson": ["meson", "--version"], "ninja": ["ninja", "--version"],
                "rustc": ["rustc", "--version"], "bindgen": ["bindgen", "--version"],
                "gcc": ["gcc", "--version"], "g++": ["g++", "--version"],
                "strip": ["strip", "--version"], "pkg-config": ["pkg-config", "--version"]}
    missing = [name for name in commands if not shutil.which(name)]
    for module in ["Mako", "PyYAML", "MarkupSafe"]:
        try:
            importlib.metadata.version(module)
        except importlib.metadata.PackageNotFoundError:
            missing.append("Python " + module)
    if not Path("/usr/lib/llvm-11/lib/libclang.so").is_file():
        missing.append("libclang-11-dev")
    if missing:
        raise ValueError("fixed image is missing malisx2 build dependencies: " + ", ".join(missing) +
                         "; approve the reviewed fixed-image recipe before rebuilding it")
    versions = {name: subprocess.check_output(args, text=True).splitlines()[0] for name, args in commands.items()}
    def version(text):
        match = re.search(r"\d+\.\d+(?:\.\d+)?", text)
        if not match:
            raise ValueError("cannot identify build tool version: " + text)
        return tuple(map(int, match.group().split(".")))
    if version(versions["meson"]) < (1, 9, 2) or version(versions["rustc"]) < (1, 85):
        raise ValueError("malisx2 requires reviewed Meson >=1.9.2 and Rust >=1.85 in the fixed image")
    bindgen = version(versions["bindgen"])
    if bindgen < (0, 71, 1) or bindgen in [(0, 72, 0), (0, 73, 0), (0, 73, 1)]:
        raise ValueError("malisx2 requires bindgen >=0.71.1 excluding 0.72.0/0.73.0/0.73.1")
    backend = Path(importlib.metadata.distribution("meson").locate_file("mesonbuild/backend/ninjabackend.py"))
    fix = "file.get_subdir() if isinstance(file, build.CustomTargetIndex) else file.subdir"
    if backend.read_text().count(fix) != 1:
        raise ValueError("fixed Meson image is missing the reviewed CustomTargetIndex compatibility fix")
    versions["meson_ninja_backend_sha256"] = digest(backend)
    versions["build_recipe_sha256"] = digest(Path(__file__).with_name("rudroid-rust-gles.Dockerfile"))
    versions["python"] = platform.python_version()
    versions["python_executable"] = str(Path(sys.executable).resolve())
    versions["python_modules"] = {name: importlib.metadata.version(name) for name in ["Mako", "PyYAML", "MarkupSafe"]}
    subprocess.run(["pkg-config", "--print-errors", "--exists", "libdrm", "gbm", "libudev"], check=True)
    content = json.dumps(versions, sort_keys=True, indent=2) + "\n"
    if str(output) == "-":
        print(content, end="")
    else:
        no_link_ancestors(output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(content)
        machine = output.parent / "python.ini"
        no_link_ancestors(machine)
        python = versions["python_executable"]
        if not re.fullmatch(r"/[A-Za-z0-9_./-]+", python):
            raise ValueError("unexpected fixed Python executable path")
        machine.write_text("[binaries]\npython = '" + python + "'\n")


def check_python_build(build):
    no_link_ancestors(build / "tools.json")
    no_link_ancestors(build / "build.ninja")
    tools = json.loads((build / "tools.json").read_text())
    commands = [line for line in (build / "build.ninja").read_text().splitlines()
                if "vk_cmd_queue_gen.py" in line and "COMMAND =" in line]
    if not commands or any(tools["python_executable"] not in line for line in commands):
        raise ValueError("Meson did not select the preflight Python for Mesa generators; reconfigure the fixed output")


def package(runtime, inputs, build, ceiling):
    # Statically inspect the actual library; an Android pack cannot pass this gate.
    from linux_abi import Elf, audit_elf
    check_output(runtime)
    no_link_ancestors(build)
    no_link_ancestors(build / "meson-info/intro-buildoptions.json")
    no_link_ancestors(build / "tools.json")
    receipt = prepare(inputs, offline=True)
    options = {item["name"]: item["value"] for item in json.loads((build / "meson-info/intro-buildoptions.json").read_text())}
    if options.get("buildtype") != "release" or options.get("b_ndebug") not in (True, "true") or options.get("measurement") is not False or options.get("debug") is not False or options.get("strip") is not True:
        raise ValueError("malisx2 artifact requires stripped Release, NDEBUG, no measurement and no debug")
    library = runtime / "libvulkan_malisx2.so"
    no_link_ancestors(library)
    blob = library.read_bytes()
    elf = Elf(blob)
    if elf.unpack("<H", 16)[0] != 3:
        raise ValueError("malisx2 must be an ARM64 shared library")
    audit = audit_elf(blob, ceiling)
    if "libc.so.6" not in audit["needed"] or any(n in audit["needed"] for n in ("libc.so", "liblog.so")):
        raise ValueError("malisx2 requires a Linux GNU library, not Android Bionic")
    (runtime / "linux-abi.json").write_text(json.dumps(audit, sort_keys=True, indent=2) + "\n")
    licenses = runtime / "licenses"
    no_link_ancestors(licenses)
    source = inputs / "source"
    # Rebuild licenses cleanly in a sibling staging directory so stale files never linger.
    notices = []
    licenses_staging = Path(tempfile.mkdtemp(prefix=".licenses-", dir=runtime))
    try:
        for directory, dirs, files in os.walk(source, followlinks=False):
            dirs.sort()
            files.sort()
            for name in files:
                path = Path(directory) / name
                if path.is_symlink():
                    continue
                relative_name = path.relative_to(source)
                if re.match(r"^(license|licence|copying|notice)([._-]|$)", name, re.I):
                    dest = licenses_staging / relative_name
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(path, dest)
                if path.suffix in (".c", ".cpp", ".h", ".rs"):
                    with path.open("r", errors="replace") as stream:
                        prefix = stream.read(16384)
                    match = re.match(r"\s*(/\*.*?\*/|(?://[^\n]*\n)+)", prefix, re.S)
                    if match and re.search(r"copyright|SPDX", match.group(), re.I):
                        notices.append(str(relative_name) + "\n" + match.group().strip() + "\n")
        notices.sort()
        (licenses_staging / "source-header-notices.txt").write_text("\n".join(notices))
        if licenses.exists():
            shutil.rmtree(licenses)
        os.replace(licenses_staging, licenses)
    except BaseException:
        shutil.rmtree(licenses_staging, ignore_errors=True)
        raise
    shutil.copyfile(Path(__file__).with_name(C11_PATCH["patch_file"]), runtime / C11_PATCH["patch_file"])
    manifest = {"file_format_version": "1.0.0", "ICD": {"library_path": "./libvulkan_malisx2.so", "api_version": "1.1.0"}}
    (runtime / "mali_icd.json").write_text(json.dumps(manifest, indent=2) + "\n")
    minimum_glibc = audit["glibc_max_required"] or "none"
    metadata = {"platform": "linux-aarch64", "profile": "Release", "source_revision": REVISION,
                "mesa_revision": MESA_REVISION, "library_sha256": digest(library),
                "minimum_glibc": minimum_glibc, "sources": receipt,
                "build_options": {k: options[k] for k in ("buildtype", "b_ndebug", "measurement", "debug", "strip")},
                "tools": json.loads((build / "tools.json").read_text()),
                "build_recipe_sha256": digest(Path(__file__).with_name("rudroid-rust-gles.Dockerfile")),
                "scope": "optional third-party ICD; kernel ABI, WSI and gameplay require target validation"}
    (runtime / "sources.json").write_text(json.dumps(metadata, sort_keys=True, indent=2) + "\n")
    print("malisx2 Linux Release artifact audited; minimum glibc=" + minimum_glibc)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--cache", type=Path)
    group.add_argument("--verify", type=Path)
    group.add_argument("--preflight", type=Path)
    group.add_argument("--runtime", type=Path)
    group.add_argument("--check-output", type=Path)
    group.add_argument("--check-python-build", type=Path)
    parser.add_argument("--inputs", type=Path)
    parser.add_argument("--build", type=Path)
    parser.add_argument("--max-glibc", default="2.33")
    args = parser.parse_args()
    try:
        if args.cache:
            prepare(args.cache)
        elif args.verify:
            prepare(args.verify, offline=True)
        elif args.preflight:
            preflight(args.preflight)
        elif args.check_output:
            check_output(args.check_output)
        elif args.check_python_build:
            check_python_build(args.check_python_build)
        elif not args.inputs or not args.build:
            parser.error("--runtime requires --inputs and --build")
        else:
            package(args.runtime, args.inputs, args.build, args.max_glibc)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, "malisx2-input: " + str(error) + "\n")
