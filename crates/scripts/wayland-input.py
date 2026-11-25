#!/usr/bin/env python3
# INPUT:  pinned SDL source archive, pinned Debian Vulkan loader and fixed target cache
# OUTPUT:  verified optional Wayland client inputs or an audited private runtime manifest
# POS:    prepare external client libraries without installing packages or changing GPU drivers
"""Only --portable-wayland builds consume these optional native inputs."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

SDL_VERSION = "2.32.6"
SDL_SHA = "6a7a40d6c2e00016791815e1a9f4042809210bdf10cc78d2c75b45c4f52f93ad"
SDL_PREFIX = f"SDL2-{SDL_VERSION}/"
SDL_URL = f"https://www.libsdl.org/release/SDL2-{SDL_VERSION}.tar.gz"
LOADER_SHA = "60afb6acac8e4ed51048bcdb98e5f4bcdb1b75d372791f3d7838ec021fede0d6"
LOADER_URL = ("https://archive.debian.org/debian/pool/main/v/vulkan-loader/"
              "libvulkan1_1.3.224.0-1~bpo11+1_arm64.deb")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def no_link_ancestors(path):
    for item in (path.absolute(), *path.absolute().parents):
        if item.is_symlink():
            raise ValueError("cache path contains a symlink: " + str(item))


def atomic_write_bytes(path, data):
    no_link_ancestors(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp = tempfile.mkstemp(prefix=".tmp-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, path)
    except BaseException:
        Path(temp).unlink(missing_ok=True)
        raise


def atomic_write_text(path, text):
    atomic_write_bytes(path, text.encode("utf-8"))


def verified_sdl(archive, destination, expected=SDL_SHA):
    data = archive.read_bytes()
    if sha(data) != expected:
        raise ValueError("SDL source archive checksum mismatch")
    files = {}
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        for member in tar:
            if not member.name.startswith(SDL_PREFIX):
                continue
            name = member.name[len(SDL_PREFIX):]
            # Android application examples contain links and are not Linux library inputs.
            if name.startswith(("android-project/", "android-project-ant/")):
                continue
            parts = PurePosixPath(name)
            if parts.is_absolute() or ".." in parts.parts or not (member.isdir() or member.isfile()):
                raise ValueError("unsupported SDL source entry: " + member.name)
            if member.isfile():
                if name in files:
                    raise ValueError("duplicate SDL source entry")
                files[name] = (tar.extractfile(member).read(), member.mode & 0o111 != 0)
    if "configure" not in files or "LICENSE.txt" not in files:
        raise ValueError("incomplete SDL source archive")
    expected_files = {name: sha(data) for name, (data, _) in files.items()}
    if destination.exists():
        if any(p.is_symlink() for p in destination.rglob("*")):
            raise ValueError("cached SDL source contains a symlink")
        actual = {p.relative_to(destination).as_posix(): sha(p.read_bytes())
                  for p in destination.rglob("*") if p.is_file()}
        if actual != expected_files:
            raise ValueError("cached SDL source changed")
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="extract-", dir=destination.parent) as temporary:
            for name, (data, executable) in files.items():
                p = Path(temporary) / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(data)
                p.chmod(0o755 if executable else 0o644)
            os.replace(temporary, destination)
    return {"url": SDL_URL, "sdl_version": SDL_VERSION,
            "archive_sha256": expected, "files": expected_files}


def loader_files(data):
    if sha(data) != LOADER_SHA or data[:8] != b"!<arch>\n":
        raise ValueError("Vulkan loader package checksum mismatch")
    offset = 8
    while offset + 60 <= len(data):
        header = data[offset:offset + 60]
        size = int(header[48:58])
        if size < 0 or offset + 60 + size > len(data) or header[58:60] != b"`\n":
            raise ValueError("invalid Debian archive member")
        body = data[offset + 60:offset + 60 + size]
        offset += 60 + size + size % 2
        if header[:16].decode().strip().rstrip("/").startswith("data.tar"):
            result = {}
            with tarfile.open(fileobj=io.BytesIO(body)) as tar:
                for member in tar:
                    if member.isfile() and member.name.endswith("/libvulkan.so.1.3.224"):
                        result["libvulkan.so.1"] = tar.extractfile(member).read()
                    elif member.isfile() and member.name.endswith("/libvulkan1/copyright"):
                        result["LICENSE-Vulkan-Loader.txt"] = tar.extractfile(member).read()
            if len(result) != 2:
                raise ValueError("Vulkan loader package missing library or copyright")
            return result
    raise ValueError("Vulkan loader package missing data archive")


def prepare(cache):
    no_link_ancestors(cache)
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"SDL2-{SDL_VERSION}.tar.gz"
    no_link_ancestors(archive)
    if not archive.exists():
        with urllib.request.urlopen(SDL_URL, timeout=30) as response:
            data = response.read(16 * 1024 * 1024 + 1)
        if len(data) > 16 * 1024 * 1024 or sha(data) != SDL_SHA:
            raise ValueError("SDL source archive checksum mismatch")
        atomic_write_bytes(archive, data)
    sdl = verified_sdl(archive, cache / f"sdl-{SDL_VERSION}")
    archive = cache / "vulkan-loader.deb"
    no_link_ancestors(archive)
    if not archive.exists():
        with urllib.request.urlopen(LOADER_URL, timeout=30) as response:
            data = response.read(4 * 1024 * 1024 + 1)
        if len(data) > 4 * 1024 * 1024:
            raise ValueError("Vulkan loader package exceeds size limit")
        loader_files(data)
        atomic_write_bytes(archive, data)
    files = loader_files(archive.read_bytes())
    destination = cache / "loader"
    no_link_ancestors(destination)
    destination.mkdir(exist_ok=True)
    for name, data in files.items():
        path = destination / name
        no_link_ancestors(path)
        if path.exists() and path.read_bytes() != data:
            raise ValueError("cached Vulkan loader changed: " + name)
        atomic_write_bytes(path, data)
    receipt = {"sdl": sdl, "loader": {"url": LOADER_URL, "version": "1.3.224.0-1~bpo11+1",
               "package_sha256": LOADER_SHA, "files": {k: sha(v) for k, v in files.items()}}}
    atomic_write_text(cache / "sources.json", json.dumps(receipt, sort_keys=True, indent=2) + "\n")
    print("Optional Wayland client inputs verified (SDL zlib; Vulkan loader Apache-2.0).")


def package(runtime):
    # These optional external libraries have a separate target ABI requirement.
    # The core kit's independent 2.28 gate is not relaxed.
    no_link_ancestors(runtime)
    libraries = [runtime / name for name in ["libSDL2-2.0.so.0", "libvulkan.so.1"]]
    if any(not p.is_file() or p.is_symlink() for p in libraries):
        raise ValueError("optional client build is incomplete")
    audit = runtime / "linux-abi.json"
    no_link_ancestors(audit)
    subprocess.run([sys.executable, str(Path(__file__).with_name("linux_abi.py")),
                    *[arg for p in libraries for arg in ["--file", str(p)]],
                    "--max-glibc", "2.33", "--output", str(audit)], check=True)
    requirements = json.loads(audit.read_text())
    versions = [m["glibc_max_required"] for m in requirements["members"].values() if m.get("glibc_max_required")]
    minimum = max(versions, key=lambda s: tuple(map(int, s.split(".")))) if versions else "none"
    manifest = {"purpose": "optional ARM64 glibc Wayland clients; existing system GPU driver",
                "minimum_glibc": minimum, "sdl_version": SDL_VERSION, "vulkan_loader_version": "1.3.224",
                "files": {p.name: {"size": p.stat().st_size, "sha256": sha(p.read_bytes())}
                          for p in sorted(runtime.iterdir())
                          if p.is_file() and not p.is_symlink() and p.name != "manifest.json"}}
    atomic_write_text(runtime / "manifest.json", json.dumps(manifest, sort_keys=True, indent=2) + "\n")
    print("Optional Wayland client bundle audited; minimum glibc=" + minimum)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--cache", type=Path)
    group.add_argument("--runtime", type=Path)
    args = parser.parse_args()
    if args.cache:
        prepare(args.cache)
    else:
        package(args.runtime)
