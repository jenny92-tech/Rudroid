#!/usr/bin/env python3
# INPUT:  immutable upstream archives and the existing Docker target cache
# OUTPUT:  SHA256-verified WSI and Vulkan-Headers sources plus a provenance receipt
# POS:    prepares optional native inputs before the fixed Docker source freeze
"""No compiler or system package installation; normal GLES builds never call this."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

SOURCES = {
    "wsi": ("ginkage/vulkan-wsi-layer", "8b18c35cb323e39b50776bd8ca8b1dbc7353f373",
            "7cdba92c919aed35b614e13ac97508bbb00d26d48665256e826bbc9691511a83"),
    "headers": ("KhronosGroup/Vulkan-Headers", "29f979ee5aa58b7b005f805ea8df7a855c39ff37",
                "dfc96c55d7e481c42b917706ff52205d47c0f0957c8c2a60766ee781f16d18bd"),
    "protocols": ("wayland-mirror/wayland-protocols", "b0a25f26d3e8b5ad24773881a9a895dd9cc75466",
                  "8bee3fa1506aa8f371aa4a8a01818aa3d5f205ed69ffb699fe3e24baf4964b75"),
}

# Header-only Linux UAPI; neither the kernel nor libdrm is built or installed.
HEADERS = {
    "drm_fourcc.h": ("drm/drm_fourcc.h", "56a7352082da1b357d2ba33106d219b6fa0b37ccb88d593439ac9d98ad291915"),
    "drm.h": ("drm/drm.h", "8e486feade7383ac6a2a1744b1a985d652a813aeb73667b24c1e96dace62daaa"),
    "drm_mode.h": ("drm/drm_mode.h", "a0f37c79bd82bfa26d9e3db8ffa3782904575fcc9bd3e902916d2acf4e6c9154"),
    "linux/dma-heap.h": ("linux/dma-heap.h", "2870b36ee84ed6ecb03cf9f2dd856302348258439a088138c4b0170d86882324"),
}

MAX_ARCHIVE = 32 * 1024 * 1024
MAX_EXPANDED = 128 * 1024 * 1024
MAX_MEMBERS = 20000


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


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


def _tree_digests(root):
    no_link_ancestors(root)
    if any(p.is_symlink() for p in root.rglob("*")):
        raise ValueError(f"cached tree contains a symlink: {root}")
    return {p.relative_to(root).as_posix(): digest(p) for p in root.rglob("*") if p.is_file()}


def _expected_patch():
    native_build = Path(__file__).resolve().parents[1] / "native-build"
    patch = native_build / "patches/vulkan-wsi-native-allocation.patch"
    match = re.search(
        r'WSI_PATCH_SHA256: &str = "([0-9a-f]{64})"',
        (native_build / "src/vulkan_wsi.rs").read_text(),
    )
    if not match:
        raise ValueError("cannot locate WSI_PATCH_SHA256 in native-build/src/vulkan_wsi.rs")
    expected_patch = match.group(1)
    if digest(patch) != expected_patch:
        raise ValueError("WSI patch does not match its fingerprinted native-build pin")
    return patch, expected_patch


def _expected_layer_json(wsi_dir):
    manifest = json.loads((wsi_dir / "layer/VkLayer_window_system_integration.json").read_text())
    layer = manifest["layer"]
    layer["name"] = "VK_LAYER_RUDROID_wayland"
    layer["description"] = "Rudroid private Wayland WSI (Arm upstream)"
    layer.pop("disable_environment", None)  # Explicit private layer; the firmware's implicit copy is disabled.
    instance = {"VK_KHR_wayland_surface", "VK_KHR_surface", "VK_KHR_get_surface_capabilities2", "VK_EXT_surface_maintenance1"}
    device = {"VK_KHR_swapchain", "VK_KHR_present_id"}
    layer["instance_extensions"] = [e for e in layer["instance_extensions"] if e["name"] in instance]
    layer["device_extensions"] = [e for e in layer["device_extensions"] if e["name"] in device]
    return json.dumps(manifest, indent=2) + "\n"


def prepare(cache, offline=False):
    no_link_ancestors(cache)
    if offline:
        if not cache.is_dir():
            raise ValueError("missing verified offline Vulkan input cache")
    else:
        cache.mkdir(parents=True, exist_ok=True)
    receipt = {}
    for name, (repo, commit, expected) in SOURCES.items():
        url = f"https://codeload.github.com/{repo}/tar.gz/{commit}"
        archive = cache / f"{name}-{commit}.tar.gz"
        no_link_ancestors(archive)
        if not archive.exists():
            if offline:
                raise ValueError(f"missing verified offline archive: {archive.name}")
            with urllib.request.urlopen(url, timeout=45) as response:
                data = response.read(MAX_ARCHIVE + 1)
            if len(data) > MAX_ARCHIVE or hashlib.sha256(data).hexdigest() != expected:
                raise ValueError(f"unverified upstream archive: {url}")
            atomic_write_bytes(archive, data)
        if digest(archive) != expected:
            raise ValueError(f"archive changed: {archive}")
        dest = cache / name
        no_link_ancestors(dest)
        with tarfile.open(archive) as tar:
            members = tar.getmembers()
            if (
                len(members) > MAX_MEMBERS
                or any(m.size < 0 for m in members)
                or sum(m.size for m in members) > MAX_EXPANDED
            ):
                raise ValueError(f"upstream archive exceeds extraction budget: {archive.name}")
            roots = {PurePosixPath(m.name).parts[0] for m in members}
            if len(roots) != 1:
                raise ValueError("upstream archive needs one root")
            files = {}
            for member in members:
                parts = PurePosixPath(member.name).parts
                if member.name.startswith("/") or ".." in parts or not (member.isdir() or member.isfile()):
                    raise ValueError(f"unsupported archive entry: {member.name}")
                if member.isfile():
                    rel = PurePosixPath(*parts[1:]).as_posix()
                    if rel in files:
                        raise ValueError(f"duplicate archive entry: {member.name}")
                    with tar.extractfile(member) as stream:
                        files[rel] = hashlib.sha256(stream.read()).hexdigest()
            if dest.exists():
                if _tree_digests(dest) != files:
                    raise ValueError(f"extracted upstream source changed: {dest}")
            elif offline:
                raise ValueError(f"missing extracted offline source: {dest}")
            else:
                with tempfile.TemporaryDirectory(prefix="extract-", dir=cache) as temporary:
                    for member in members:
                        relative = PurePosixPath(*PurePosixPath(member.name).parts[1:])
                        path = Path(temporary) / relative
                        if member.isdir():
                            path.mkdir(parents=True, exist_ok=True)
                        else:
                            path.parent.mkdir(parents=True, exist_ok=True)
                            with tar.extractfile(member) as stream, path.open("wb") as output:
                                shutil.copyfileobj(stream, output)
                    os.replace(temporary, dest)
        receipt[name] = {"url": url, "commit": commit, "archive_sha256": expected, "files": files}

    patch, expected_patch = _expected_patch()
    dest = cache / "wsi-patched"
    no_link_ancestors(dest)
    receipt_path = cache / "receipt.json"
    prior_receipt = None
    if receipt_path.is_file() and not receipt_path.is_symlink():
        try:
            prior_receipt = json.loads(receipt_path.read_text())
        except (OSError, ValueError):
            prior_receipt = None

    if offline:
        if not dest.is_dir():
            raise ValueError(f"missing offline patched WSI source: {dest}")
        actual_patched = _tree_digests(dest)
        if shutil.which("git"):
            with tempfile.TemporaryDirectory(prefix="wsi-verify-") as temporary:
                staging = Path(temporary) / "wsi"
                shutil.copytree(cache / "wsi", staging)
                subprocess.run(["git", "apply", "--no-index", str(patch)], cwd=staging, check=True)
                files = _tree_digests(staging)
            if actual_patched != files:
                raise ValueError(f"patched WSI source changed: {dest}")
        else:
            prior_patch = (prior_receipt or {}).get("wsi_patch")
            if not isinstance(prior_patch, dict) or prior_patch.get("sha256") != expected_patch:
                raise ValueError("offline WSI receipt patch pin mismatch; re-run --cache on host")
            files = prior_patch.get("files")
            if actual_patched != files:
                raise ValueError(f"patched WSI source changed: {dest}")
    else:
        with tempfile.TemporaryDirectory(prefix="patch-", dir=cache) as temporary:
            staging = Path(temporary) / "wsi"
            shutil.copytree(cache / "wsi", staging)
            subprocess.run(["git", "apply", "--no-index", str(patch)], cwd=staging, check=True)
            files = _tree_digests(staging)
            if dest.exists():
                actual = _tree_digests(dest)
                if actual != files:
                    prior_patch = (prior_receipt or {}).get("wsi_patch")
                    if (
                        isinstance(prior_patch, dict)
                        and prior_patch.get("sha256") != expected_patch
                        and prior_patch.get("files") == actual
                    ):
                        # Cleanly migrate an unmodified previous pinned patch tree.
                        backup = Path(temporary) / "wsi-patched-old"
                        os.replace(dest, backup)
                        os.replace(staging, dest)
                    else:
                        raise ValueError(f"patched WSI source changed: {dest}")
            else:
                os.replace(staging, dest)
    receipt["wsi_patch"] = {"sha256": expected_patch, "files": files}

    receipt["linux_uapi"] = {}
    for name, (upstream, expected) in HEADERS.items():
        url = "https://raw.githubusercontent.com/torvalds/linux/v6.6/include/uapi/" + upstream
        path = cache / "uapi" / name
        no_link_ancestors(path)
        if not path.exists():
            if offline:
                raise ValueError(f"missing offline UAPI header: {path}")
            with urllib.request.urlopen(url, timeout=30) as response:
                data = response.read(256 * 1024 + 1)
            if len(data) > 256 * 1024 or hashlib.sha256(data).hexdigest() != expected:
                raise ValueError(f"unverified header: {url}")
            atomic_write_bytes(path, data)
        if digest(path) != expected:
            raise ValueError(f"header changed: {path}")
        receipt["linux_uapi"][name] = {"url": url, "sha256": expected}

    layer_path = cache / "layer.json"
    no_link_ancestors(layer_path)
    layer_content = _expected_layer_json(cache / "wsi")
    if offline:
        if not layer_path.is_file() or layer_path.read_text() != layer_content:
            raise ValueError(f"offline layer manifest changed or missing: {layer_path}")
    elif not layer_path.exists() or layer_path.read_text() != layer_content:
        atomic_write_text(layer_path, layer_content)
    receipt["layer_manifest_sha256"] = digest(layer_path)

    no_link_ancestors(receipt_path)
    receipt_content = json.dumps(receipt, sort_keys=True, indent=2) + "\n"
    if offline:
        if not receipt_path.is_file() or receipt_path.read_text() != receipt_content:
            raise ValueError(f"offline Vulkan receipt changed or missing: {receipt_path}")
    elif not receipt_path.exists() or receipt_path.read_text() != receipt_content:
        atomic_write_text(receipt_path, receipt_content)
    print("Vulkan native inputs verified (WSI MIT; Vulkan-Headers Apache-2.0/MIT).")


def package(runtime, max_glibc):
    no_link_ancestors(runtime)
    library = runtime / "libVkLayer_window_system_integration.so"
    layer_json = runtime / "VkLayer_RUDROID_wayland.json"
    no_link_ancestors(library)
    no_link_ancestors(layer_json)
    if not library.is_file() or not layer_json.is_file():
        raise ValueError("Vulkan WSI runtime output is incomplete")
    audit_path = runtime / "linux-abi.json"
    subprocess.run(
        [
            sys.executable,
            str(Path(__file__).with_name("linux_abi.py")),
            "--file",
            str(library),
            "--max-glibc",
            max_glibc,
            "--output",
            str(audit_path),
        ],
        check=True,
    )
    requirements = json.loads(audit_path.read_text())
    member = next(iter(requirements["members"].values()))
    needed = member.get("needed", [])
    if "libc.so.6" not in needed or any(n in needed for n in ("libc.so", "liblog.so")):
        raise ValueError("Vulkan WSI layer must link Linux GNU libc, not Android Bionic")
    minimum = member.get("glibc_max_required") or "none"
    manifest = {
        "purpose": "optional ARM64 glibc Wayland Vulkan WSI layer",
        "minimum_glibc": minimum,
        "files": {
            p.name: {"size": p.stat().st_size, "sha256": digest(p)}
            for p in sorted(runtime.iterdir())
            if p.is_file() and not p.is_symlink() and p.name != "manifest.json"
        },
    }
    atomic_write_text(runtime / "manifest.json", json.dumps(manifest, sort_keys=True, indent=2) + "\n")
    print("Vulkan WSI Release bundle audited; minimum glibc=" + minimum)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--cache", type=Path)
    group.add_argument("--verify", type=Path)
    group.add_argument("--runtime", type=Path)
    parser.add_argument("--max-glibc", default="2.33")
    args = parser.parse_args()
    try:
        if args.cache:
            prepare(args.cache, offline=False)
        elif args.verify:
            prepare(args.verify, offline=True)
        else:
            package(args.runtime, args.max_glibc)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, "vulkan-input: " + str(error) + "\n")
