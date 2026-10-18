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


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def prepare(cache):
    cache.mkdir(parents=True, exist_ok=True)
    receipt = {}
    for name, (repo, commit, expected) in SOURCES.items():
        url = f"https://codeload.github.com/{repo}/tar.gz/{commit}"
        archive = cache / f"{name}-{commit}.tar.gz"
        if not archive.exists():
            with urllib.request.urlopen(url, timeout=45) as response:
                data = response.read(32 * 1024 * 1024 + 1)
            if len(data) > 32 * 1024 * 1024 or hashlib.sha256(data).hexdigest() != expected:
                raise ValueError(f"unverified upstream archive: {url}")
            temporary = archive.with_suffix(".partial")
            temporary.write_bytes(data)
            os.replace(temporary, archive)
        if digest(archive) != expected:
            raise ValueError(f"archive changed: {archive}")
        dest = cache / name
        with tarfile.open(archive) as tar:
            members = tar.getmembers()
            roots = {PurePosixPath(m.name).parts[0] for m in members}
            if len(roots) != 1:
                raise ValueError("upstream archive needs one root")
            files = {}
            for member in members:
                parts = PurePosixPath(member.name).parts
                if member.name.startswith("/") or ".." in parts or not (member.isdir() or member.isfile()):
                    raise ValueError(f"unsupported archive entry: {member.name}")
                if member.isfile():
                    with tar.extractfile(member) as stream:
                        files[str(PurePosixPath(*parts[1:]))] = hashlib.sha256(stream.read()).hexdigest()
            if dest.exists():
                actual = {p.relative_to(dest).as_posix(): digest(p) for p in dest.rglob("*") if p.is_file()}
                if actual != files or any(p.is_symlink() for p in dest.rglob("*")):
                    raise ValueError(f"extracted upstream source changed: {dest}")
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
    native_build = Path(__file__).resolve().parents[1] / "native-build"
    patch = native_build / "patches/vulkan-wsi-native-allocation.patch"
    expected_patch = re.search(r'WSI_PATCH_SHA256: &str = "([0-9a-f]{64})"',
                               (native_build / "src/vulkan_wsi.rs").read_text()).group(1)
    if digest(patch) != expected_patch:
        raise ValueError("WSI patch does not match its fingerprinted native-build pin")
    with tempfile.TemporaryDirectory(prefix="patch-", dir=cache) as temporary:
        staging = Path(temporary) / "wsi"
        shutil.copytree(cache / "wsi", staging)
        subprocess.run(["git", "apply", "--no-index", str(patch)], cwd=staging, check=True)
        files = {p.relative_to(staging).as_posix(): digest(p) for p in staging.rglob("*") if p.is_file()}
        dest = cache / "wsi-patched"
        if dest.exists():
            actual = {p.relative_to(dest).as_posix(): digest(p) for p in dest.rglob("*") if p.is_file()}
            if actual != files or any(p.is_symlink() for p in dest.rglob("*")):
                raise ValueError(f"patched WSI source changed: {dest}")
        else:
            os.replace(staging, dest)
    receipt["wsi_patch"] = {"sha256": expected_patch, "files": files}
    receipt["linux_uapi"] = {}
    for name, (upstream, expected) in HEADERS.items():
        url = "https://raw.githubusercontent.com/torvalds/linux/v6.6/include/uapi/" + upstream
        path = cache / "uapi" / name
        if not path.exists():
            with urllib.request.urlopen(url, timeout=30) as response:
                data = response.read(256 * 1024)
            if hashlib.sha256(data).hexdigest() != expected:
                raise ValueError(f"unverified header: {url}")
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        if digest(path) != expected:
            raise ValueError(f"header changed: {path}")
        receipt["linux_uapi"][name] = {"url": url, "sha256": expected}
    manifest = json.loads((cache / "wsi/layer/VkLayer_window_system_integration.json").read_text())
    layer = manifest["layer"]
    layer["name"] = "VK_LAYER_RUDROID_wayland"
    layer["description"] = "Rudroid private Wayland WSI (Arm upstream)"
    layer.pop("disable_environment", None)  # Explicit private layer; the firmware's implicit copy is disabled.
    instance = {"VK_KHR_wayland_surface", "VK_KHR_surface", "VK_KHR_get_surface_capabilities2", "VK_EXT_surface_maintenance1"}
    device = {"VK_KHR_swapchain", "VK_KHR_present_id"}
    layer["instance_extensions"] = [e for e in layer["instance_extensions"] if e["name"] in instance]
    layer["device_extensions"] = [e for e in layer["device_extensions"] if e["name"] in device]
    layer_path = cache / "layer.json"
    content = json.dumps(manifest, indent=2) + "\n"
    if not layer_path.exists() or layer_path.read_text() != content:
        layer_path.write_text(content)
    receipt["layer_manifest_sha256"] = digest(layer_path)
    receipt_path = cache / "receipt.json"
    content = json.dumps(receipt, sort_keys=True, indent=2) + "\n"
    if not receipt_path.exists() or receipt_path.read_text() != content:
        receipt_path.write_text(content)
    print("Vulkan native inputs verified (WSI MIT; Vulkan-Headers Apache-2.0/MIT).")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", required=True, type=Path)
    prepare(parser.parse_args().cache)
