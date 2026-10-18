#!/usr/bin/env python3
# INPUT:  Unity resource tree, optional archive entry sizes and original audit JSON
# OUTPUT:  read-only texture/audio/video/stream audit and gated original comparison
# POS:    finds resource conversion defects and candidates without rewriting games
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2026 jenny92-tech and the Rudroid contributors
"""Read-only resource audit. Payload totals describe files, not live game RAM."""
import argparse
from collections import Counter, defaultdict
import gc
import json
import math
import ntpath
from pathlib import Path
import time

import UnityPy
from UnityPy.enums import TextureFormat

if __package__:
    from .unity_assets import _is_bundle, atomic_json, fingerprint
    from .unity_astc.texture_policy import astc_block_of, astc_bytes, sensitive_name_reason
    from .unity_astc.astc_retier import STREAM_FIELDS, _stream_ref, collect_font_textures
else:
    from unity_assets import _is_bundle, atomic_json, fingerprint
    from unity_astc.texture_policy import astc_block_of, astc_bytes, sensitive_name_reason
    from unity_astc.astc_retier import STREAM_FIELDS, _stream_ref, collect_font_textures


def container_key(relative):
    """Keep PAD namespaces apart; strip only the generic FAT32 shard segment."""
    parts = list(Path(relative).parts)
    prefix = "base"
    if "assetpacks" in parts:
        index = parts.index("assetpacks")
        if index + 1 < len(parts):
            prefix = "assetpack/" + parts[index + 1]
    return prefix + "/" + parts[-1].replace("|", "-")


def external_sizes(root, archive_index):
    sizes = {}
    if archive_index:
        data = json.loads(Path(archive_index).read_text())
        for item in data["entries"]:
            if "Size" in item:
                sizes[item["Path"]] = int(item["Size"])
    for path in root.rglob("*"):
        if path.is_file() and path.suffix.lower() in (".ress", ".resource"):
            sizes[path.relative_to(root).as_posix()] = path.stat().st_size
    return sizes


def stream_location(env, container, ref, sizes):
    name, offset, size = ref
    base = ntpath.basename(name)
    # Exact stream names only; an audio .resource must never alias a texture .resS.
    reader = env.get_cab(base)
    length = getattr(reader, "Length", None)
    if length is not None:
        return {"path": name, "offset": offset, "size": size,
                "storage": "internal", "available": length,
                "valid_range": offset >= 0 and size >= 0 and offset + size <= length}
    relative = (Path(container).parent / base).as_posix()
    if relative in sizes:
        return {"path": name, "offset": offset, "size": size,
                "storage": "external", "available": sizes[relative],
                "valid_range": offset >= 0 and size >= 0 and offset + size <= sizes[relative]}
    return {"path": name, "offset": offset, "size": size,
            "storage": "unresolved", "valid_range": None}


def texture_record(texture, obj, fonts):
    width, height = texture.m_Width, texture.m_Height
    count = texture.m_MipCount or 1
    fmt = TextureFormat(texture.m_TextureFormat).name
    block = astc_block_of(texture.m_TextureFormat)
    stream = getattr(texture, "m_StreamData", None)
    data = texture.image_data
    payload = len(data) if data else (stream.size if stream and stream.path else 0)
    font = (texture.m_Name.endswith(" Atlas") or
            (ntpath.basename(obj.assets_file.name), obj.path_id) in fonts)
    protected = font or bool(sensitive_name_reason(texture.m_Name))
    images = getattr(texture, "m_ImageCount", 1)
    expected = (astc_bytes(width, height, block, count) * images
                if block and min(width, height) > 0 and images > 0 else None)
    raw = fmt in ("RGBA32", "RGB24", "ARGB32", "BGRA32")
    candidate = raw and not protected and max(width, height) > 256 and images == 1
    saving = (max(0, payload - astc_bytes(width, height, (8, 8), count))
              if candidate else 0)
    return {"assets_file": obj.assets_file.name, "path_id": obj.path_id,
            "name": texture.m_Name, "width": width, "height": height,
            "mips": count, "images": images, "format": fmt, "payload_bytes": payload,
            "expected_astc_bytes": expected,
            "astc_payload_matches": expected == payload if expected is not None else None,
            "font": font, "protected": protected,
            "raw_astc8_candidate": candidate, "astc8_payload_reduction_estimate": saving,
            "decoded_rgba_base_bytes": max(0, width) * max(0, height) * 4}


def scan_container(path, root, sizes):
    relative = path.relative_to(root).as_posix()
    before = path.stat()
    result = {"container": relative, "key": container_key(relative),
              "fingerprint": fingerprint(path), "textures": [], "audio": [],
              "video": [], "streams": [], "errors": [], "objects": 0}
    try:
        env = UnityPy.load(str(path))
        if not env.objects:
            result["errors"].append("no serialized objects; not a verified Unity container")
        fonts = collect_font_textures(env)
        versions = set()
        for obj in env.objects:
            result["objects"] += 1
            versions.add(str(obj.assets_file.unity_version))
            kind = obj.type.name
            if kind not in STREAM_FIELDS:
                continue
            try:
                data = obj.read()
                if kind == "Texture2D":
                    result["textures"].append(texture_record(data, obj, fonts))
                elif kind == "AudioClip":
                    duration = float(data.m_Length)
                    channels, frequency = int(data.m_Channels), int(data.m_Frequency)
                    if not math.isfinite(duration) or duration < 0:
                        raise ValueError("invalid audio duration")
                    result["audio"].append({"name": data.m_Name, "seconds": duration,
                                            "channels": channels, "frequency": frequency,
                                            "load_type": int(data.m_LoadType),
                                            "preload": bool(data.m_PreloadAudioData),
                                            "compression": int(data.m_CompressionFormat),
                                            "pcm16_bytes_estimate": int(duration * channels * frequency * 2)})
                elif kind == "VideoClip":
                    result["video"].append({"name": data.m_Name,
                                            "metadata_width": getattr(data, "m_Width", None),
                                            "metadata_height": getattr(data, "m_Height", None),
                                            "note": "metadata dimensions do not prove coded stream dimensions"})
                field = STREAM_FIELDS.get(kind)
                ref = _stream_ref(data, field) if field else None
                if ref:
                    item = stream_location(env, relative, ref, sizes)
                    item.update(kind=kind, name=getattr(data, "m_Name", ""), path_id=obj.path_id)
                    result["streams"].append(item)
                    if kind == "AudioClip":
                        result["audio"][-1]["storage"] = item["storage"]
            except Exception as error:
                result["errors"].append(f"{kind}/{obj.path_id}: {type(error).__name__}: {error}")
        result["unity_versions"] = sorted(versions)
        flags = getattr(env.file, "dataflags", None)
        result["unityfs_flags"] = int(flags) if flags is not None else None
    except Exception as error:
        result["errors"].append(f"{type(error).__name__}: {error}")
    after = path.stat()
    if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (
            after.st_size, after.st_mtime_ns, after.st_ctime_ns):
        result["errors"].append("source changed during scan")
    return result


def summarize(containers):
    textures = [t for c in containers for t in c["textures"]]
    audio = [t for c in containers for t in c["audio"]]
    streams = [t for c in containers for t in c["streams"]]
    formats = Counter()
    payloads = Counter()
    for texture in textures:
        formats[texture["format"]] += 1
        payloads[texture["format"]] += texture["payload_bytes"]
    duplicates = defaultdict(list)
    for container in containers:
        duplicates[container["fingerprint"]["sha256"]].append(container["container"])
    return {"containers": len(containers), "objects": sum(c["objects"] for c in containers),
            "textures": len(textures), "formats": dict(formats),
            "texture_payload_bytes_by_format": dict(payloads),
            "raw_astc8_candidates": sum(t["raw_astc8_candidate"] for t in textures),
            "astc8_payload_reduction_estimate": sum(t["astc8_payload_reduction_estimate"] for t in textures),
            "astc_payload_mismatches": sum(t["astc_payload_matches"] is False for t in textures),
            "multi_mip_textures": sum(t["mips"] > 1 for t in textures),
            "large_textures_over_2048": sum(max(t["width"], t["height"]) > 2048 for t in textures),
            "audio_clips": len(audio),
            "long_nonstream_audio": sum(t["seconds"] >= 30 and t["load_type"] in (0, 1) for t in audio),
            "long_external_nonstream_audio": sum(t["seconds"] >= 30 and t["load_type"] in (0, 1)
                                                  and t.get("storage") == "external" for t in audio),
            "video_clips": sum(len(c["video"]) for c in containers),
            "invalid_stream_ranges": sum(t["valid_range"] is False for t in streams),
            "unresolved_streams": sum(t["valid_range"] is None for t in streams),
            "parse_errors": sum(len(c["errors"]) for c in containers),
            "identical_container_groups": [group for group in duplicates.values() if len(group) > 1]}


def compare(original, processed):
    originals, results = defaultdict(list), defaultdict(list)
    for container in original["containers"]:
        originals[container["key"]].append(container)
    for container in processed["containers"]:
        results[container["key"]].append(container)
    changes, incompatible, ambiguous = [], [], []
    paired = 0
    for key in sorted(originals.keys() & results.keys()):
        if len(originals[key]) != 1 or len(results[key]) != 1:
            ambiguous.append(key)
            continue
        before, after = originals[key][0], results[key][0]
        if before.get("unity_versions") != after.get("unity_versions"):
            incompatible.append(key)
            continue
        index = {(t["assets_file"], t["path_id"]): t for t in before["textures"]}
        for texture in after["textures"]:
            previous = index.get((texture["assets_file"], texture["path_id"]))
            if previous is None or any(previous[field] != texture[field]
                                       for field in ("name", "width", "height")):
                continue
            paired += 1
            if previous["mips"] != texture["mips"]:
                changes.append({"container": key, "name": texture["name"],
                                "assets_file": texture["assets_file"], "path_id": texture["path_id"],
                                "width": texture["width"], "height": texture["height"],
                                "source_mips": previous["mips"], "processed_mips": texture["mips"]})
    return {"paired_textures": paired, "mip_changes": changes,
            "reduced_mip_textures": sum(t["source_mips"] > t["processed_mips"] for t in changes),
            "incompatible_unity_builds": incompatible, "ambiguous_containers": ambiguous}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--archive-index", type=Path)
    parser.add_argument("--compare", type=Path, help="original report; only matching objects/builds are paired")
    args = parser.parse_args(argv)
    root = args.root.resolve(strict=True)
    output = args.output.resolve()
    if output == root or root in output.parents:
        parser.error("audit output must be outside the resource input tree")
    start = time.monotonic()
    paths = sorted(p for p in root.rglob("*") if p.is_file() and
                   (p.suffix in (".bundle", ".unity3d", ".assets") or p.name == "globalgamemanagers"
                    or _is_bundle(p)))
    if not paths:
        parser.error("no Unity containers found; do not run this adapter on other engines")
    sizes = external_sizes(root, args.archive_index)
    containers = []
    for index, path in enumerate(paths, 1):
        if path.is_symlink():
            parser.error(f"input symlink is not supported: {path}")
        containers.append(scan_container(path, root, sizes))
        if index % 32 == 0:
            gc.collect()
        if index % 100 == 0 or index == len(paths):
            print(f"SCAN {index}/{len(paths)} ({time.monotonic() - start:.1f}s)", flush=True)
    report = {"root": str(root), "seconds": round(time.monotonic() - start, 3),
              "scope": "all discovered Unity containers; metadata/ranges, no image decode or runtime residency proof",
              "summary": summarize(containers), "containers": containers}
    if args.compare:
        report["comparison"] = compare(json.loads(args.compare.read_text()), report)
    summary = report["summary"]
    incomplete = summary["parse_errors"] or summary["unresolved_streams"]
    invalid = summary["invalid_stream_ranges"] or summary["astc_payload_mismatches"]
    report["status"] = "partial" if incomplete else ("issues_found" if invalid else "complete")
    atomic_json(output, report)
    print(json.dumps({"output": str(output), "status": report["status"],
                      "summary": report["summary"]}, ensure_ascii=False), flush=True)
    return 1 if incomplete or invalid else 0


if __name__ == "__main__":
    raise SystemExit(main())
