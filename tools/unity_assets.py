#!/usr/bin/env python3
# INPUT:  original Unity resource directory, new output directory and optional target profile
# OUTPUT:  prepare(), main(); complete resource directory with automatic checks and fallback
# POS:    prepares resources in one invocation without caches, manual plans or runtime work
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2026 jenny92-tech and the Rudroid contributors
"""Generate ready Unity resources: unity_assets.py SOURCE OUTPUT [--target astc-1g|astc-2g|astc-4g]."""
from collections import Counter
import hashlib
import json
import errno
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

if __package__:
    from .unity_astc.profiles import DEFAULT_TARGET, TARGETS
else:
    from unity_astc.profiles import DEFAULT_TARGET, TARGETS

REPORT = ".rudroid-resources.json"


def _publish_directory(staging, output):
    """Publish a complete tree without replacing a concurrent creator's output.

    A preceding exists() check cannot protect POSIX rename: an empty directory
    created in that gap would be silently replaced. Use the filesystem's atomic
    exclusive rename, with no check-then-rename fallback on unsupported hosts.
    """
    if os.name == "nt":
        # Windows rename already refuses an existing destination.
        os.rename(staging, output)
        return
    import ctypes
    libc = ctypes.CDLL(None, use_errno=True)
    source, destination = os.fsencode(staging), os.fsencode(output)
    if sys.platform == "darwin":
        rename = getattr(libc, "renamex_np", None)
        if rename is None:
            raise OSError(errno.ENOTSUP, "exclusive directory publication is unavailable")
        rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        rename.restype = ctypes.c_int
        result = rename(source, destination, 0x4)  # RENAME_EXCL (Darwin sys/stdio.h)
    elif sys.platform.startswith("linux"):
        rename = getattr(libc, "renameat2", None)
        if rename is None:
            raise OSError(errno.ENOTSUP, "exclusive directory publication is unavailable")
        rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        rename.restype = ctypes.c_int
        result = rename(-100, source, -100, destination, 1)  # AT_FDCWD, RENAME_NOREPLACE
    else:
        raise OSError(errno.ENOTSUP, "exclusive directory publication is unavailable")
    if result:
        error = ctypes.get_errno()
        if error in (errno.EEXIST, errno.ENOTEMPTY):
            raise ValueError("output appeared during preparation; existing data was preserved")
        raise OSError(error, os.strerror(error), os.fspath(output))


def fingerprint(path):
    before = path.stat()
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for data in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(data)
    if _state(path) != _state_value(before):
        raise ValueError(f"input changed while reading: {path}")
    return {"sha256": digest.hexdigest(), "bytes": before.st_size}


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(dir=path.parent, prefix=".json-")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        os.replace(name, path)
    finally:
        Path(name).unlink(missing_ok=True)


def _state_value(stat):
    return tuple(getattr(stat, key) for key in ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns"))


def _state(path):
    return _state_value(path.stat())


def _inventory(source):
    files = {}
    for path in sorted(source.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"input contains a symlink: {path}")
        if path.is_file():
            files[path.relative_to(source)] = _state(path)
        elif path.is_dir():
            files[path.relative_to(source)] = None
        else:
            raise ValueError(f"input contains a special file: {path}")
    return files


def _is_bundle(path):
    with path.open("rb") as handle:
        return handle.read(8) == b"UnityFS\0"


def _run_worker(source, arguments, payload=None):
    command = [sys.executable, str(Path(__file__).resolve()), *arguments]
    data = json.dumps(payload) if payload is not None else None
    with subprocess.Popen(command, stdin=subprocess.PIPE if data is not None else subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
        try:
            while True:
                try:
                    stdout, stderr = process.communicate(input=data, timeout=30)
                    break
                except subprocess.TimeoutExpired:
                    data = None  # Retry an in-progress communicate, never resend stdin.
                    print(f"  processing {source.name}...", flush=True)
        except BaseException:
            process.terminate()
            process.communicate()
            raise
        if process.returncode:
            raise ValueError(f"{source.name}: resource worker failed\n{stderr[-4000:]}")
    return json.loads(stdout)


def _scan_font_bundle(source):
    return _run_worker(source, ['--_font_refs', str(source)])


def _run_bundle(source, output, target, protected_fonts=()):
    return _run_worker(source, ['--_bundle', str(source), str(output), target], list(protected_fonts))


def _font_closure(source, bundles):
    # A font and its atlas may live in different bundles. Index only stable
    # metadata in short-lived workers; keeping UnityPy environments in this
    # parent would defeat the one-bundle memory bound. Material edges are not
    # roots, so ordinary material textures remain eligible for optimization.
    roots, materials = set(), {}
    for relative in sorted(bundles):
        metadata = _scan_font_bundle(source / relative)
        roots.update(tuple(key) for key in metadata['roots'])
        for material in metadata['materials']:
            key = material['owner'], material['path_id']
            textures = {tuple(key) for key in material['textures']}
            if key in materials and materials[key] != textures:
                raise ValueError(f"ambiguous cross-bundle font material owner: {key}")
            materials[key] = textures
    protected, pending = set(roots), list(roots)
    while pending:
        for texture in materials.get(pending.pop(), ()):
            if texture not in protected:
                protected.add(texture)
                pending.append(texture)
    return sorted(protected)


def prepare(source, output, target=DEFAULT_TARGET):
    """Inspect, convert, fall back, validate and publish a complete directory."""
    start = time.monotonic()
    source, output = Path(source).expanduser(), Path(output).expanduser()
    if target not in TARGETS:
        raise ValueError(f"unknown target: {target}")
    if source.is_symlink() or output.is_symlink():
        raise ValueError("input and output must not be symlinks")
    source, output = source.resolve(strict=True), output.resolve()
    if not source.is_dir():
        raise ValueError("source must be the original game resource directory")
    if source == output or source in output.parents or output in source.parents:
        raise ValueError("input and output must be separate directory trees")
    if output.exists():
        raise ValueError(f"output already exists; use a new directory: {output}")
    if (source / REPORT).exists():
        raise ValueError("source is already prepared; use the original resources to avoid repeated lossy conversion")
    files = _inventory(source)
    bundles = {rel for rel, state in files.items() if state is not None and _is_bundle(source / rel)}
    if not bundles:
        raise ValueError("no UnityFS resources found in the input directory")
    protected_fonts = _font_closure(source, bundles)
    output.parent.mkdir(parents=True, exist_ok=True)
    summary = {"schema_version": 1, "target": target, "files": sum(state is not None for state in files.values()), "bundles": len(bundles),
               "changed_bundles": 0, "textures": 0, "changed_textures": 0,
               "texture_payload_before": 0, "texture_payload_after": 0,
               "reasons": {}, "unsupported_bundles": []}
    reasons = Counter()
    with tempfile.TemporaryDirectory(prefix=".rudroid-", dir=output.parent) as temporary:
        staging = Path(temporary) / "resources"
        staging.mkdir()
        done = 0
        for rel in files:
            original, destination = source / rel, staging / rel
            if files[rel] is None:
                destination.mkdir(parents=True, exist_ok=True)
                continue
            destination.parent.mkdir(parents=True, exist_ok=True)
            if rel in bundles:
                done += 1
                result = _run_bundle(original, destination, target, protected_fonts)
                summary["textures"] += result["textures"]
                summary["changed_textures"] += result["changed"]
                summary["changed_bundles"] += bool(result["changed"])
                summary["texture_payload_before"] += result["payload_before"]
                summary["texture_payload_after"] += result["payload_after"]
                reasons.update(result["decisions"])
                if result.get("unoptimized_bundle"):
                    summary["unsupported_bundles"].append({"path": rel.as_posix(), "reason": result["unoptimized_bundle"]})
                if done == 1 or done % 25 == 0 or done == len(bundles):
                    print(f"[{done}/{len(bundles)}] {rel}", flush=True)
            else:
                shutil.copy2(original, destination)
            if _state(original) != files[rel]:
                raise ValueError(f"input changed during preparation: {rel}")
        if _inventory(source) != files:
            raise ValueError("input tree changed during preparation")
        summary["reasons"] = dict(reasons)
        summary["seconds"] = round(time.monotonic() - start, 2)
        summary["scope"] = "offline resources; texture payload totals are not live RAM or gameplay acceptance"
        atomic_json(staging / REPORT, summary)
        _publish_directory(staging, output)
    print(f"Ready: {output}; {summary['changed_textures']} textures prepared", flush=True)
    return summary


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == '--_font_refs':
        if len(argv) != 2:
            raise ValueError('invalid internal font scanner arguments')
        if __package__:
            from .unity_astc.astc_retier import scan_font_references
        else:
            from unity_astc.astc_retier import scan_font_references
        print(json.dumps(scan_font_references(Path(argv[1]))))
        return
    if argv and argv[0] == "--_bundle":
        # Internal short-lived worker bounds memory to one bundle, without a second public CLI.
        if len(argv) != 4:
            raise ValueError("invalid internal worker arguments")
        if __package__:
            from .unity_astc.astc_retier import prepare_bundle
        else:
            from unity_astc.astc_retier import prepare_bundle
        protected_fonts = json.loads(sys.stdin.read() or '[]')
        print(json.dumps(prepare_bundle(Path(argv[1]), Path(argv[2]), argv[3], protected_fonts)))
        return
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="original game resource directory")
    parser.add_argument("output", type=Path, help="new directory containing the complete prepared resources")
    parser.add_argument("--target", choices=TARGETS, default=DEFAULT_TARGET,
                        help=f"ASTC-capable target with 1/2/4 GiB RAM (default: {DEFAULT_TARGET}); all conversion decisions are automatic")
    args = parser.parse_args(argv)
    try:
        prepare(args.source, args.output, args.target)
    except (OSError, ValueError, ImportError) as error:
        parser.exit(1, f"Resource preparation failed: {error}\n")


if __name__ == "__main__":
    main()
