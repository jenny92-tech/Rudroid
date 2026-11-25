# INPUT:  UnityFS bundle, target profile, UnityPy, Pillow and shared texture policy
# OUTPUT:  scan_font_references(), prepare_bundle(); metadata keys and validated conversion
# POS:    automatically chooses smaller quality-passing texture payloads without runtime work
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2025-2026 jenny92-tech <jennyliu90223@gmail.com>
from collections import Counter
from copy import copy
import gc
import hashlib
import ntpath
from pathlib import Path
import shutil

import UnityPy
from UnityPy.enums import TextureFormat as TF
from UnityPy.files import SerializedFile
from UnityPy.streams import EndianBinaryReader

from .image_encode import append_mips
from .profiles import TARGETS
from .resource_refs import internal_resource_name, resource_basename
from .texture_policy import DESKTOP_COLOR_FORMATS, astc_block_of, astc_bytes, image_passes, keep_reason

STREAM_FIELDS = {
    "Texture2D": "m_StreamData", "Texture2DArray": "m_StreamData",
    "Texture3D": "m_StreamData", "Cubemap": "m_StreamData",
    "CubemapArray": "m_StreamData", "Mesh": "m_StreamData",
    "AudioClip": "m_Resource", "VideoClip": "m_ExternalResources",
}


def _stream_ref(data, field):
    info = getattr(data, field, None)
    if info is None:
        return None
    if field == "m_StreamData":
        path, offset, size = info.path, info.offset, info.size
    else:
        path, offset, size = info.m_Source, info.m_Offset, info.m_Size
    return (path, offset, size) if size else None


def _field(data, name, default=None):
    return data.get(name, default) if isinstance(data, dict) else getattr(data, name, default)


def _font_metadata(env):
    """Extract stable font roots/material edges without loading external files."""
    objects = list(env.objects)
    index, checked_owners = {}, set()
    for obj in objects:
        owner = resource_basename(obj.assets_file.name).lower()
        if owner not in checked_owners:
            internal_resource_name(getattr(env, "file", None), obj.assets_file.name)
            checked_owners.add(owner)
        key = (owner, obj.path_id)
        if key in index:
            raise ValueError("ambiguous font object owner")
        index[key] = obj

    def pointer_key(pointer, owner):
        pid = _field(pointer, "m_PathID", _field(pointer, "path_id", 0))
        if not pid:
            return None
        fid = _field(pointer, "m_FileID", _field(pointer, "file_id", 0))
        externals = getattr(owner, "externals", [])
        if fid < 0 or fid > len(externals):
            raise ValueError("invalid font texture owner")
        name = owner.name if fid == 0 else externals[fid - 1].path
        # Resolve only already-loaded objects: do not follow arbitrary external
        # filesystem paths or guess a same-PathID object in another assets file.
        return resource_basename(name).lower(), pid

    def deref(pointer, owner):
        return index.get(pointer_key(pointer, owner))

    roots, materials = set(), {}

    def add_root(pointer, owner):
        key = pointer_key(pointer, owner)
        if key is not None:
            roots.add(key)

    for obj in objects:
        if obj.type.name == "Font":
            data = obj.read()
            add_root(_field(data, "m_Texture"), obj.assets_file)
            add_root(_field(data, "m_DefaultMaterial"), obj.assets_file)
        elif obj.type.name == "Material":
            properties = _field(obj.read(), "m_SavedProperties")
            slots = _field(properties, "m_TexEnvs", [])
            edges = set()
            for _, slot in slots.items() if isinstance(slots, dict) else slots:
                key = pointer_key(_field(slot, "m_Texture"), obj.assets_file)
                if key is not None:
                    edges.add(key)
            materials[(resource_basename(obj.assets_file.name).lower(), obj.path_id)] = edges
        elif obj.type.name == "MonoBehaviour":
            try:
                data = obj.read_typetree()
            except (ValueError, TypeError, KeyError, AttributeError, IndexError):
                # An opaque unrelated script must not disable conversion for a
                # whole bundle. No custom-font coverage is inferred without its
                # serialized fields; sensitive texture names remain a fallback.
                continue
            modern = "m_AtlasTextures" in data and any(
                name in data for name in ("m_GlyphTable", "m_CharacterTable", "m_FaceInfo"))
            legacy = "m_glyphInfoList" in data and any(name in data for name in ("atlas", "m_atlas"))
            script = deref(data.get("m_Script"), obj.assets_file)
            font_script = (script is not None and script.type.name == "MonoScript"
                           and "font" in str(_field(script.read(), "m_ClassName", "")).casefold())
            if not (modern or legacy or font_script):
                continue
            for field in ("m_AtlasTextures", "atlasTextures"):
                for pointer in data.get(field, []) or []:
                    add_root(pointer, obj.assets_file)
            for field in ("atlas", "m_atlas", "m_AtlasTexture", "atlasTexture", "m_Texture",
                          "m_Material", "m_DefaultMaterial", "material"):
                add_root(data.get(field), obj.assets_file)
    return roots, materials, index


def collect_font_textures(env, protected_fonts=()):
    """Follow only font roots; an unrelated material/atlas never becomes a root."""
    roots, materials, index = _font_metadata(env)
    pending = list(roots | {(resource_basename(owner).lower(), pid) for owner, pid in protected_fonts})
    keys, visited = set(), set()
    while pending:
        key = pending.pop()
        if key in visited:
            continue
        visited.add(key)
        if key in materials:
            pending.extend(materials[key])
            continue
        target = index.get(key)
        if target is None:
            keys.add(key)
        elif target.type.name == "Texture2D":
            keys.add((resource_basename(target.assets_file.name), target.path_id))
    return keys


def scan_font_references(path):
    """Return only JSON metadata for the parent's complete-tree font closure.

    Keys are lowercase owner basename + PathID, matching UnityPy pointer lookup.
    Roots retain both font atlas and material pointers; material edges alone do
    not confer protection. The parent closes the graph across bundles before
    starting any conversion worker. No UnityPy objects escape this process.
    """
    env = UnityPy.load(str(path))
    if getattr(env.file, "signature", None) != "UnityFS":
        raise ValueError(f"not a UnityFS bundle: {path}")
    roots, materials, _ = _font_metadata(env)
    return {"roots": [list(key) for key in sorted(roots)],
            "materials": [{"owner": owner, "path_id": pid,
                           "textures": [list(key) for key in sorted(edges)]}
                          for (owner, pid), edges in sorted(materials.items())]}


def check_streams(env, bundle):
    """Validate exact stream owners; never redirect to a similarly named file."""
    refs = 0
    for obj in env.objects:
        field = STREAM_FIELDS.get(obj.type.name)
        if field is None:
            continue
        ref = _stream_ref(obj.read(), field)
        if ref is None:
            continue
        path, offset, size = ref
        name = resource_basename(path)
        if offset < 0 or size < 0:
            raise ValueError(f"invalid stream reference in {bundle.name}: {path}")
        internal = internal_resource_name(getattr(env, "file", None), path)
        reader = env.get_cab(name)
        if internal is not None and reader is not env.file.files[internal]:
            raise ValueError(f"ambiguous resource registry owner: {path!r}")
        if reader is not None:
            if not isinstance(reader, EndianBinaryReader):
                raise ValueError(f"stream does not reference raw resource bytes: {path!r}")
            length = reader.Length
        else:
            resource = bundle.parent / name
            if resource.is_symlink() or not resource.is_file():
                raise ValueError(f"missing or linked stream: {resource}")
            length = resource.stat().st_size
            if obj.type.name == "AudioClip":
                from .audio_resource_validate import AudioRef, validate_ref
                problems = validate_ref(bundle.parent, AudioRef(str(obj.path_id), path, offset, size))
                if problems:
                    raise ValueError(problems[0])
        if offset + size > length:
            raise ValueError(f"stream range exceeds {name}: {offset}+{size} > {length}")
        refs += 1
    return refs


def payload_size(texture):
    if min(texture.m_Width, texture.m_Height) <= 0:
        return 0
    block = astc_block_of(texture.m_TextureFormat)
    if block is not None:
        return astc_bytes(texture.m_Width, texture.m_Height, block, texture.m_MipCount or 1)
    stream = getattr(texture, "m_StreamData", None)
    return (getattr(texture, "m_CompleteImageSize", 0) or getattr(stream, "size", 0)
            or len(texture.image_data))


def convert_texture(texture, key, fonts, target):
    """Try coarse to fine from the original pixels, then retain the original."""
    reason = keep_reason(texture, key, fonts)
    if reason:
        return None, reason
    width, height = texture.m_Width, texture.m_Height
    count = texture.m_MipCount or 1
    if min(width, height) < 1 or not 1 <= count <= max(width, height).bit_length():
        return None, "invalid-mip-layout"
    before = payload_size(texture)
    if texture.m_TextureFormat in DESKTOP_COLOR_FORMATS:
        # The target promises ASTC, not desktop BC/DXT support. Price these
        # against decoded RGBA, avoiding an unnecessary runtime decode fallback.
        before = sum(max(1, width >> n) * max(1, height >> n) * 4 for n in range(count))
    blocks = [block for block in TARGETS[target]
              if astc_bytes(width, height, (block, block), count) + 4096 <= before]
    if not blocks:
        return None, "already-small"
    reference = texture.image.convert("RGBA")
    for block in blocks:
        trial = copy(texture)
        trial.m_StreamData = copy(texture.m_StreamData)
        trial.set_image(reference, target_format=getattr(TF, f"ASTC_RGB_{block}x{block}"), mipmap_count=1)
        if not image_passes(reference, trial.image.convert("RGBA")):
            continue
        # Keep the tested base bytes. Generate the full tail only for the winner.
        append_mips(trial, reference, count)
        return trial, f"astc-{block}x{block}"
    return None, "quality-preserved"


def _stream_target(bundle, texture):
    ref = _stream_ref(texture, "m_StreamData")
    if ref is None or texture.image_data:
        return None
    name = internal_resource_name(bundle, ref[0])
    return name if isinstance(bundle.files.get(name), EndianBinaryReader) else None


def _save_preserving_layout(bundle, patches, streams):
    """Retain every serialized header, object offset and untouched byte."""
    for name, entry in list(bundle.files.items()):
        if isinstance(entry, SerializedFile):
            data = entry.reader.bytes
            edits = sorted(patches.get(name, []))
            if edits:
                data = bytearray(data)
                end = entry.header.data_offset
                for offset, size, replacement in edits:
                    if len(replacement) != size or offset < end or offset + size > len(data):
                        raise ValueError("texture edit would change the serialized object layout")
                    data[offset:offset + size] = replacement
                    end = offset + size
            reader = EndianBinaryReader(bytes(data))
        elif name in streams:
            reader = EndianBinaryReader(bytes(streams[name]))
        else:
            continue
        reader.flags = entry.flags
        bundle.files[name] = reader
    return bundle.save(packer="original")


def prepare_bundle(source, output, target, protected_fonts=()):
    source, output = Path(source), Path(output)
    protected_fonts = tuple(protected_fonts)
    if target not in TARGETS:
        raise ValueError(f"unknown target: {target}")
    env = UnityPy.load(str(source))
    if getattr(env.file, "signature", None) != "UnityFS":
        raise ValueError(f"not a UnityFS bundle: {source}")
    textures = []
    try:
        fonts = {(resource_basename(owner).lower(), pid)
                 for owner, pid in collect_font_textures(env, protected_fonts)}
        for obj in env.objects:
            if obj.type.name == "Texture2D":
                texture = obj.read()
                obj.read_typetree()  # Reject incomplete/unknown serialized layouts before writing anything.
                textures.append((obj, texture))
    except (ValueError, TypeError, KeyError, AttributeError, IndexError) as error:
        # An unknown layout remains byte-identical. Never rewrite a partially parsed bundle.
        shutil.copyfile(source, output)
        return {"textures": 0, "changed": 0, "decisions": {"unsupported-layout": 1},
                "payload_before": 0, "payload_after": 0, "unoptimized_bundle": str(error)[:180]}
    refs = check_streams(env, source)
    changed, reasons = {}, Counter()
    patches, streams = {}, {}
    before = after = 0
    for obj, texture in textures:
        key = (obj.assets_file.name, obj.path_id)
        size = payload_size(texture)
        before += size
        stream = _stream_target(env.file, texture)
        if stream is None:
            converted, reason = None, "stream-layout-preserved"
        else:
            converted, reason = convert_texture(texture, (ntpath.basename(key[0]), key[1]), fonts, target)
        reasons[reason] += 1
        if converted is not None:
            payload = converted.image_data
            if stream not in streams:
                streams[stream] = bytearray(env.file.files[stream].bytes)
            data = streams[stream]
            data.extend(b"\0" * (-len(data) % 16))
            converted.m_StreamData = copy(texture.m_StreamData)
            converted.m_StreamData.offset, converted.m_StreamData.size = len(data), len(payload)
            converted.image_data = b""
            replacement = obj.save_typetree(converted)
            if len(replacement) != obj.byte_size:
                raise ValueError("texture serialization changed its object size")
            patches.setdefault(key[0], []).append((obj.byte_start, obj.byte_size, replacement))
            data.extend(payload)
            changed[key] = (hashlib.sha256(payload).hexdigest(), converted.m_TextureFormat)
            after += payload_size(converted)
        else:
            after += size
    if changed:
        output.write_bytes(_save_preserving_layout(env.file, patches, streams))
        del env, textures, obj, texture, converted, patches, streams, data, payload, replacement
        gc.collect()
        from .validate_output import validate
        validate(source, output, changed, protected_fonts)
    else:
        shutil.copyfile(source, output)
    return {"textures": sum(reasons.values()), "changed": len(changed), "decisions": dict(reasons),
            "payload_before": before, "payload_after": after, "stream_references": refs}
