#!/usr/bin/env python3
# INPUT:  standalone Godot PCK v2/v3, optional per-file digest verification
# OUTPUT:  read-only directory/CTEX/variant/integrity report
# POS:    audits Godot separately using bounded reads without loading an engine
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2026 jenny92-tech and the Rudroid contributors
"""Audit a standalone PCK. Encrypted and sparse packs fail closed.

Format reference: Godot core/io/file_access_pack.{h,cpp} and
scene/resources/compressed_texture.cpp; no Godot implementation is imported.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import struct

if __package__:
    from .unity_assets import atomic_json
else:
    from unity_assets import atomic_json


def read_exact(handle, count):
    value = handle.read(count)
    if len(value) != count:
        raise ValueError("truncated PCK directory/header")
    return value


def directory(path):
    length = path.stat().st_size
    with path.open("rb") as handle:
        if read_exact(handle, 4) != b"GDPC":
            raise ValueError("not a standalone Godot PCK")
        version, major, minor, patch, flags, base = struct.unpack("<5IQ", read_exact(handle, 28))
        if version not in (2, 3):
            raise ValueError(f"unsupported PCK version {version}")
        if flags & 1:
            raise ValueError("encrypted directory requires the game's authorized preparation path")
        if flags & 4:
            raise ValueError("sparse bundle requires a separate file provider")
        if version == 3:
            offset, = struct.unpack("<Q", read_exact(handle, 8))
            if not 40 <= offset <= length - 4:
                raise ValueError("PCK directory offset outside file")
            handle.seek(offset)
        else:
            read_exact(handle, 64)
        count, = struct.unpack("<I", read_exact(handle, 4))
        if count > 1_000_000:
            raise ValueError("PCK directory entry limit exceeded")
        entries = []
        for _ in range(count):
            sl, = struct.unpack("<I", read_exact(handle, 4))
            if not 1 <= sl <= 1024 * 1024:
                raise ValueError("invalid PCK path length")
            name = read_exact(handle, sl).rstrip(b"\0").decode("utf-8")
            offset, size, md5, entry_flags = struct.unpack("<QQ16sI", read_exact(handle, 36))
            absolute = base + offset
            removal = bool(entry_flags & 2)
            if not removal and (absolute > length or size > length - absolute):
                raise ValueError(f"entry outside PCK: {name}")
            entries.append({"path": name, "offset": absolute, "size": size,
                            "md5": md5.hex(), "encrypted": bool(entry_flags & 1),
                            "removal": removal})
    return {"format": version, "godot": f"{major}.{minor}.{patch}",
            "flags": flags, "file_bytes": length, "entries": entries}


def audit(path, verify_digests=False):
    result = directory(path)
    variants, variant_bytes, formats = Counter(), Counter(), Counter()
    textures, errors, digests = [], [], 0
    groups, paths = defaultdict(list), Counter()
    with path.open("rb") as handle:
        for entry in result["entries"]:
            name = entry["path"]
            paths[name] += 1
            if entry["removal"] or entry["encrypted"]:
                continue
            handle.seek(entry["offset"])
            if name.endswith(".ctex"):
                header = handle.read(min(52, entry["size"]))
                if len(header) < 52 or header[:4] != b"GST2":
                    errors.append(f"unsupported/truncated CTEX header: {name}")
                else:
                    version, logical_w, logical_h = struct.unpack_from("<3I", header, 4)
                    data_format, width, height, mips, fmt = struct.unpack_from("<IHHII", header, 36)
                    if version > 1:
                        errors.append(f"unsupported CTEX version {version}: {name}")
                    suffix = name.rsplit(".", 2)[-2]
                    variant = suffix if suffix in ("astc", "etc2", "s3tc", "bptc") else "unqualified"
                    variants[variant] += 1
                    variant_bytes[variant] += entry["size"]
                    formats[str(fmt)] += 1
                    key = name.rsplit(".", 2)[0] if variant != "unqualified" else name[:-5]
                    groups[key].append(variant)
                    textures.append({"path": name, "width": width, "height": height,
                                     "logical_width": logical_w, "logical_height": logical_h,
                                     "mip_levels": mips + 1, "image_format": fmt,
                                     "data_format": data_format, "bytes": entry["size"]})
            if verify_digests and entry["md5"] != "0" * 32:
                handle.seek(entry["offset"])
                md5 = hashlib.md5(usedforsecurity=False)
                left = entry["size"]
                while left:
                    data = handle.read(min(left, 1024 * 1024))
                    if not data:
                        raise ValueError(f"truncated entry: {name}")
                    md5.update(data)
                    left -= len(data)
                digests += 1
                if md5.hexdigest() != entry["md5"]:
                    errors.append(f"entry digest mismatch: {name}")
    result.update(path=str(path.resolve()), textures=textures, errors=errors,
                  summary={"entries": len(result["entries"]), "textures": len(textures),
                           "variants": dict(variants), "variant_bytes": dict(variant_bytes),
                           "image_formats": dict(formats), "digests_verified": digests,
                           "encrypted_entries_unverified": sum(e["encrypted"] for e in result["entries"]),
                           "duplicate_paths": [p for p, n in paths.items() if n > 1],
                           "multi_variant_groups": sum(len(v) > 1 for v in groups.values()),
                           "errors": len(errors)})
    result["status"] = "issues_found" if errors else (
        "partial" if result["summary"]["encrypted_entries_unverified"] else "complete")
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pck", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--verify-digests", action="store_true")
    args = parser.parse_args(argv)
    if args.pck.resolve() == args.output.resolve():
        parser.error("output cannot overwrite the input PCK")
    try:
        result = audit(args.pck, args.verify_digests)
    except (OSError, ValueError) as error:
        result = {"path": str(args.pck.resolve()), "status": "partial", "error": str(error)}
    atomic_json(args.output.resolve(), result)
    print(json.dumps({k: v for k, v in result.items() if k not in ("entries", "textures")}, ensure_ascii=False))
    return 0 if result["status"] == "complete" else 1


if __name__ == "__main__":
    raise SystemExit(main())
