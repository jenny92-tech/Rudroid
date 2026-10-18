#!/usr/bin/env python3
# INPUT:  current/reference UnityFS pair and UnityPy ASTC codecs
# OUTPUT:  complete_mip_tail(), repair_bundle(), validate_repair()
# POS:    appends missing ASTC mip levels while preserving every existing payload byte
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2026 jenny92-tech and the Rudroid contributors
"""Restore missing mip tails; the reference supplies counts, never replacement art."""
import gc
import hashlib

import UnityPy
from PIL import Image
from UnityPy.enums import BuildTarget, TextureFormat
from UnityPy.export.Texture2DConverter import astc, image_to_texture2d


def mip_layout(width, height, count, block):
    if (min(width, height) < 1 or not 1 <= count <= max(width, height).bit_length()):
        raise ValueError("invalid texture dimensions/mip count")
    result, cursor = [], 0
    for _ in range(count):
        size = ((width + block[0] - 1) // block[0]) * ((height + block[1] - 1) // block[1]) * 16
        result.append((width, height, cursor, size))
        cursor += size
        width, height = max(1, width // 2), max(1, height // 2)
    return result


def inline_astc(texture):
    fmt = TextureFormat(texture.m_TextureFormat)
    if not fmt.name.startswith(("ASTC_RGB_", "ASTC_RGBA_")):
        raise ValueError("mip repair supports only LDR ASTC textures")
    reader = texture.object_reader
    if (reader.platform != BuildTarget.Android or getattr(texture, "m_PlatformBlob", None)
            or getattr(texture, "m_ImageCount", 1) != 1
            or getattr(texture, "m_TextureDimension", 2) != 2
            or getattr(texture, "m_MipsStripped", 0) != 0):
        raise ValueError("mip repair requires an unswizzled Android 2D texture without stripped mips")
    stream = getattr(texture, "m_StreamData", None)
    if stream and (stream.path or stream.offset or stream.size):
        raise ValueError("streamed texture mip repair requires a separate stream-owner path")
    block = tuple(int(x) for x in fmt.name.rsplit("_", 1)[1].split("x"))
    layout = mip_layout(texture.m_Width, texture.m_Height, texture.m_MipCount, block)
    data = bytes(texture.image_data)
    if (len(data) != sum(row[3] for row in layout)
            or texture.m_CompleteImageSize != len(data)):
        raise ValueError("ASTC payload/complete-size mismatch before repair")
    return fmt, block, data, layout


def complete_mip_tail(texture, count):
    """Encode only absent levels from the last existing level, in stored orientation.

    Existing artistic/alpha edits stay authoritative. Even a deliberately blank
    effect keeps blank lower levels; original artwork is never restored over it.
    No mutation takes place until all new levels encode and decode successfully.
    """
    fmt, block, data, old = inline_astc(texture)
    full = mip_layout(texture.m_Width, texture.m_Height, count, block)
    if count <= len(old):
        raise ValueError("reference must have more mip levels than the current texture")
    width, height, offset, size = old[-1]
    if width * height > 1024 * 1024:
        raise ValueError("last existing mip exceeds the bounded tail-repair decode size")
    image = astc(data[offset:offset + size], width, height, block)
    levels = [data]
    for width, height, _offset, size in full[len(old):]:
        image = image.resize((width, height), Image.Resampling.BICUBIC)
        encoded, actual = image_to_texture2d(image, fmt, texture.object_reader.platform,
                                            flip=False)
        if actual != fmt or len(encoded) != size:
            raise ValueError("new mip format/size mismatch")
        if astc(encoded, width, height, block).size != (width, height):
            raise ValueError("new mip failed to decode")
        levels.append(encoded)
    payload = b"".join(levels)
    texture.image_data = payload
    texture.m_CompleteImageSize = len(payload)
    texture.m_MipCount = count
    if getattr(texture, "m_MipMap", None) is not None:
        texture.m_MipMap = count > 1
    return {"old_mips": len(old), "new_mips": count, "old_bytes": len(data),
            "added_bytes": len(payload) - len(data),
            "preserved_sha256": hashlib.sha256(data).hexdigest()}


def object_map(env):
    result = {}
    for obj in env.objects:
        key = (obj.assets_file.name, obj.path_id)
        if key in result:
            raise ValueError(f"ambiguous serialized object: {key}")
        result[key] = obj
    return result


def bundle_identity(env):
    file = env.file
    if getattr(file, "signature", None) != "UnityFS":
        raise ValueError("only UnityFS bundles support mip repair")
    return (file.version, file.version_player, file.version_engine,
            tuple(sorted({str(o.assets_file.unity_version) for o in env.objects})))


def repair_targets(current, reference):
    if bundle_identity(current) != bundle_identity(reference):
        raise ValueError("current/reference Unity build mismatch")
    current_objects, reference_objects = object_map(current), object_map(reference)
    targets = {}
    for key, obj in current_objects.items():
        if obj.type.name != "Texture2D":
            continue
        other = reference_objects.get(key)
        if other is None or other.type.name != "Texture2D":
            raise ValueError(f"missing matching reference texture: {key}")
        texture, original = obj.read(), other.read()
        fields = ("m_Name", "m_Width", "m_Height", "m_ImageCount", "m_TextureDimension")
        if any(getattr(texture, f, None) != getattr(original, f, None) for f in fields):
            raise ValueError(f"reference texture identity/dimensions mismatch: {key}")
        before, after = texture.m_MipCount, original.m_MipCount
        if after < before:
            raise ValueError(f"reference mip count is smaller than current: {key}")
        if after > before:
            if obj.serialized_type is None or obj.serialized_type.node is None:
                raise ValueError(f"writing requires an embedded texture type tree: {key}")
            mip_layout(original.m_Width, original.m_Height, after, (4, 4))
            inline_astc(texture)  # Fail before any texture is modified.
            targets[key] = (texture, after)
    return targets


def metadata_value(value):
    """Flatten type trees once; UnityPy's node equality recurses twice per level."""
    if value is None or isinstance(value, (str, int, float, bool, bytes)):
        return value
    if isinstance(value, (list, tuple)):
        return tuple(metadata_value(item) for item in value)
    if hasattr(value, "traverse") and hasattr(value, "to_dict"):
        flattened = []
        for node in value.traverse():
            fields = node.to_dict()
            fields.pop("m_Children", None)
            flattened.append(tuple(sorted(fields.items())))
        return tuple(flattened)
    fields = getattr(type(value), "__attrs_attrs__", None)
    if fields is not None:
        return tuple((field.name, metadata_value(getattr(value, field.name))) for field in fields)
    raise ValueError(f"unknown serialized metadata layout: {type(value).__name__}")


def validate_repair(current_path, reference_path, output_path):
    """Independently reopen and allow only the explicit mip-tail metadata delta."""
    current, reference, output = (UnityPy.load(str(path)) for path in
                                  (current_path, reference_path, output_path))
    targets = repair_targets(current, reference)
    if not targets:
        raise ValueError("reference contains no missing mip tails")
    if bundle_identity(current) != bundle_identity(output):
        raise ValueError("output Unity build changed")
    if (current.file.dataflags != output.file.dataflags or
            current.file._block_info_flags != output.file._block_info_flags):
        raise ValueError("output UnityFS flags changed")
    if [(k, v.flags) for k, v in current.file.files.items()] != [
            (k, v.flags) for k, v in output.file.files.items()]:
        raise ValueError("output bundle entries/flags changed")
    old_objects, new_objects = object_map(current), object_map(output)
    if old_objects.keys() != new_objects.keys():
        raise ValueError("output serialized object identities changed")
    changed_files = {key[0] for key in targets}
    for name, file in current.file.files.items():
        other_file = output.file.files[name]
        if name in changed_files:
            if str(file.unity_version) != str(other_file.unity_version):
                raise ValueError(f"serialized Unity build changed: {name}")
            for field in ("target_platform", "_enable_type_tree", "types",
                          "ref_types", "script_types", "externals", "userInformation",
                          "unknown", "big_id_enabled"):
                if metadata_value(getattr(file, field, None)) != metadata_value(
                        getattr(other_file, field, None)):
                    raise ValueError(f"serialized metadata changed: {name}/{field}")
            for field in ("version", "endian", "reserved"):
                if getattr(file.header, field) != getattr(other_file.header, field):
                    raise ValueError(f"serialized header changed: {name}/{field}")
        before = getattr(file, "reader", file)
        after = getattr(other_file, "reader", other_file)
        if name not in changed_files and before.bytes != after.bytes:
            raise ValueError(f"unmodified serialized/stream file changed: {name}")
    report = []
    for key, obj in old_objects.items():
        other = new_objects[key]
        if obj.type != other.type or obj.type_id != other.type_id:
            raise ValueError(f"object type changed: {key}")
        if key not in targets:
            if obj.get_raw_data() != other.get_raw_data():
                raise ValueError(f"unselected object bytes changed: {key}")
            continue
        texture, count = targets[key]
        repaired = other.read()
        fmt, block, data, old_layout = inline_astc(texture)
        new_fmt, _block, payload, layout = inline_astc(repaired)
        if new_fmt != fmt or repaired.m_MipCount != count or not payload.startswith(data):
            raise ValueError(f"existing mip payload/count changed: {key}")
        if getattr(repaired, "m_MipMap", True) is False:
            raise ValueError(f"repaired mip-map flag is false: {key}")
        old_tree, new_tree = obj.read_typetree(), other.read_typetree()
        # These are the only permitted serialized field changes.
        for field in ("image data", "m_CompleteImageSize", "m_MipCount", "m_MipMap"):
            old_tree.pop(field, None)
            new_tree.pop(field, None)
        if old_tree != new_tree:
            raise ValueError(f"texture settings/references changed: {key}")
        for width, height, offset, size in layout[len(old_layout):]:
            astc(payload[offset:offset + size], width, height, block)
        report.append({"assets_file": key[0], "path_id": key[1], "name": texture.m_Name,
                       "old_mips": len(old_layout), "new_mips": count,
                       "old_bytes": len(data), "added_bytes": len(payload) - len(data),
                       "preserved_sha256": hashlib.sha256(data).hexdigest()})
    return {"textures": report, "objects": len(old_objects),
            "added_payload_bytes": sum(row["added_bytes"] for row in report)}


def repair_bundle(current_path, reference_path, output_path):
    if output_path.exists() or output_path.is_symlink():
        raise ValueError("worker output must be a new file")
    current, reference = (UnityPy.load(str(p)) for p in (current_path, reference_path))
    targets = repair_targets(current, reference)
    if not targets:
        raise ValueError("reference contains no missing mip tails")
    for texture, count in targets.values():
        complete_mip_tail(texture, count)
        texture.save()
    payload = current.file.save(packer="original")
    del current, reference, targets
    gc.collect()
    with output_path.open("xb") as handle:
        handle.write(payload)

