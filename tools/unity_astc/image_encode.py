# INPUT:  Pillow image, UnityPy Texture2D instance and target format
# OUTPUT:  set_image_preserving_mips(), append_mips() with complete original mip count
# POS:    preserves every mip level while reusing an already quality-checked base
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2026 jenny92-tech and the Rudroid contributors
from PIL import Image
from UnityPy.export.Texture2DConverter import image_to_texture2d


def set_image_preserving_mips(texture, image, target_format):
    """UnityPy's setter stops below 4 pixels; ASTC also needs the 2/1px tail.

    Use its ordinary single-level setter for format/platform/stream handling,
    then append each remaining level with dimensions clamped to at least one.
    """
    count = texture.m_MipCount or 1
    _check_chain(texture, image, count)
    texture.set_image(image, target_format=target_format, mipmap_count=1)
    append_mips(texture, image, count)


def _check_chain(texture, image, count):
    width, height = image.size
    if min(width, height) < 1 or not 1 <= count <= max(width, height).bit_length():
        raise ValueError(f"invalid texture dimensions/mip count: {width}x{height}, {count}")
    if getattr(texture, "m_ImageCount", 1) != 1:
        raise ValueError("multi-image Texture2D requires a separate conversion path")


def append_mips(texture, image, count):
    """Keep the quality-checked base bytes; encode each remaining level once."""
    _check_chain(texture, image, count)
    width, height = image.size
    target_format = texture.m_TextureFormat
    reader = getattr(texture, "object_reader", None)
    platform = reader.platform if reader is not None else 0
    platform_blob = getattr(texture, "m_PlatformBlob", None)
    levels = [texture.image_data]
    current = image
    for _ in range(1, count):
        width, height = max(1, width // 2), max(1, height // 2)
        current = current.resize((width, height), Image.Resampling.BICUBIC)
        encoded, actual = image_to_texture2d(current, target_format, platform, platform_blob)
        if actual != texture.m_TextureFormat:
            raise ValueError("mip format differs from base texture format")
        levels.append(encoded)
    texture.image_data = b"".join(levels)
    texture.m_CompleteImageSize = len(texture.image_data)
    texture.m_MipCount = count
    if getattr(texture, "m_MipMap", None) is not None:
        texture.m_MipMap = count > 1
