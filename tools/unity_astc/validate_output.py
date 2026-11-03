# INPUT:  source/output UnityFS files, changed object identities and UnityPy
# OUTPUT:  snapshot(), validate(); strict bundle, object, texture and stream checks
# POS:    reopens serialized output independently before the complete directory is published
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2026 jenny92-tech and the Rudroid contributors
import gc
import hashlib
import ntpath
from pathlib import Path

import UnityPy
from UnityPy.files import SerializedFile
from UnityPy.streams import EndianBinaryReader


def serialized_layout(file, changed):
    data = file.reader.bytes
    ranges = sorted((obj.byte_start, obj.byte_size, obj.path_id) for obj in file.objects.values()
                    if (file.name, obj.path_id) in changed)
    digest, end = hashlib.sha256(), 0
    for offset, size, _ in ranges:
        if offset < max(end, file.header.data_offset) or size < 0 or offset + size > len(data):
            raise ValueError("invalid serialized object range")
        digest.update(data[end:offset])
        end = offset + size
    digest.update(data[end:])
    return len(data), ranges, digest.hexdigest()


def stream_snapshot(file, prefixes, ranges):
    result = {}
    for name, reader in file.files.items():
        if not isinstance(reader, EndianBinaryReader):
            continue
        data = reader.bytes
        if prefixes is None or name not in prefixes:
            result[name] = (len(data), hashlib.sha256(data).hexdigest(), reader.flags)
            continue
        length, digest, flags = prefixes[name]
        if len(data) < length or reader.flags != flags or hashlib.sha256(data[:length]).hexdigest() != digest:
            raise ValueError("original resource stream was modified: " + name)
        end = length
        for offset, size in sorted(ranges.get(name, [])):
            aligned = (end + 15) // 16 * 16
            if offset != aligned or any(data[end:offset]) or size <= 0 or offset + size > len(data):
                raise ValueError("invalid appended texture range: " + name)
            end = offset + size
        if end != len(data):
            raise ValueError("unowned appended resource bytes: " + name)
        result[name] = prefixes[name]
    return result


def snapshot(path, resource_root, changed=frozenset(), expected=None, prefixes=None):
    from .astc_retier import check_streams
    gc.collect()
    env = UnityPy.load(str(path))
    file = env.file
    result = {"header": (file.version, file.version_engine, file.version_player, file.dataflags),
              "nodes": {name: getattr(reader, "flags", None) for name, reader in file.files.items()},
              "layout": {name: serialized_layout(reader, changed)
                         for name, reader in file.files.items() if isinstance(reader, SerializedFile)},
              "objects": {}, "textures": {}}
    ranges = {}
    # The output keeps the original external resource filenames and byte ranges.
    check_streams(env, resource_root / Path(path).name)
    for obj in env.objects:
        key = (obj.assets_file.name, obj.path_id)
        if key in result["objects"]:
            raise ValueError(f"duplicate object identity: {key}")
        raw = None if key in changed else hashlib.sha256(obj.get_raw_data()).hexdigest()
        result["objects"][key] = (obj.type.name, raw)
        if obj.type.name != "Texture2D":
            continue
        texture = obj.read()
        settings = obj.read_typetree()
        if key in changed:
            for field in ("image data", "m_TextureFormat", "m_CompleteImageSize", "m_StreamData"):
                settings.pop(field, None)
        result["textures"][key] = settings
        if key in changed:
            from .texture_policy import astc_block_of, astc_bytes, cpu_pixels_protected
            block = astc_block_of(texture.m_TextureFormat)
            if cpu_pixels_protected(texture):
                raise ValueError(f"CPU-readable texture was selected: {key}")
            if block is not None and len(texture.get_image_data()) != astc_bytes(
                    texture.m_Width, texture.m_Height, block, texture.m_MipCount or 1):
                raise ValueError(f"incomplete ASTC mip payload: {key}")
            if expected is not None and (hashlib.sha256(texture.get_image_data()).hexdigest(),
                                         texture.m_TextureFormat) != expected[key]:
                raise ValueError(f"serialized texture differs from the quality-checked payload: {key}")
            if expected is not None and getattr(texture, "m_StreamData", None):
                stream = texture.m_StreamData
                if stream.size:
                    if texture.image_data:
                        raise ValueError(f"changed texture has both inline and streamed pixels: {key}")
                    name = ntpath.basename(stream.path)
                    if not isinstance(file.files.get(name), EndianBinaryReader):
                        raise ValueError(f"changed texture is not in its internal resource stream: {key}")
                    ranges.setdefault(name, []).append((stream.offset, stream.size))
    result["streams"] = stream_snapshot(file, prefixes, ranges)
    del env
    gc.collect()
    return result


def validate(source, output, expected):
    source, output = Path(source), Path(output)
    changed = set(expected)
    original = snapshot(source, source.parent, changed)
    converted = snapshot(output, source.parent, changed, expected, original["streams"])
    if original != converted:
        differences = [key for key in original if original[key] != converted[key]]
        raise ValueError("converted bundle changed invariant(s): " + ", ".join(differences))
    if not changed <= original["textures"].keys():
        raise ValueError("selected texture is missing from the source")
    return len(original["textures"])
