# INPUT:  texture metadata, Pillow image pairs and verified ASTC target profiles
# OUTPUT:  target block candidates, sensitive-texture protection and image quality checks
# POS:    keeps all automatic resource policy inside the one-shot converter
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
# Copyright (c) 2025-2026 jenny92-tech <jennyliu90223@gmail.com>
import math
import re

from PIL import Image, ImageChops, ImageFilter, ImageOps, ImageStat
from UnityPy.enums import TextureFormat as TF

UNITY_ASTC_BLOCKS = {(n, n) for n in (4, 5, 6, 8, 10, 12)}
QUALITY_LIMITS = (32.0, 0.995, 3.0, 1.0)
DESKTOP_COLOR_FORMATS = {TF.DXT1, TF.DXT5, TF.DXT1Crunched, TF.DXT5Crunched, TF.BC7}
COLOR_FORMATS = {TF.RGBA32, TF.RGB24, TF.BGRA32, TF.ARGB32,
                 TF.DXT1, TF.DXT5, TF.DXT1Crunched, TF.DXT5Crunched, TF.BC7,
                 TF.ETC_RGB4, TF.ETC_RGB4Crunched, TF.ETC2_RGB, TF.ETC2_RGBA1,
                 TF.ETC2_RGBA8, TF.ETC2_RGBA8Crunched,
                 TF.PVRTC_RGB2, TF.PVRTC_RGB4, TF.PVRTC_RGBA2, TF.PVRTC_RGBA4}


def cpu_pixels_protected(texture):
    return getattr(texture, "m_IsReadable", None) is not False


def astc_bytes(w, h, block, mips):
    bx, by = block
    total = 0
    for _ in range(max(1, mips)):
        total += math.ceil(w / bx) * math.ceil(h / by) * 16
        w, h = max(1, w // 2), max(1, h // 2)
    return total


def astc_block_of(fmt):
    try:
        name = TF(fmt).name
    except ValueError:
        # A newer Unity format is opaque to this codec, not permission to
        # decode or rewrite it. Let the caller retain its original payload.
        return None
    if name.startswith(("ASTC_RGB_", "ASTC_RGBA_")):
        bx, by = name.rsplit("_", 1)[1].split("x")
        return int(bx), int(by)
    return None


def psnr(a_rgba, b_rgba):
    rms = ImageStat.Stat(ImageChops.difference(a_rgba, b_rgba)).rms
    mse = sum(r * r for r in rms) / len(rms)
    return 99.0 if mse <= 1e-9 else 10 * math.log10(255 * 255 / mse)


def _global_ssim(a, b):
    """Dependency-free luminance SSIM on a small representative image."""
    if max(a.size) > 64:
        scale = 64 / max(a.size)
        size = (max(1, round(a.size[0] * scale)),
                max(1, round(a.size[1] * scale)))
        a = a.resize(size, Image.Resampling.LANCZOS)
        b = b.resize(size, Image.Resampling.LANCZOS)
    a_stat = ImageStat.Stat(a)
    b_stat = ImageStat.Stat(b)
    mean_a, mean_b = a_stat.mean[0], b_stat.mean[0]
    var_a, var_b = a_stat.var[0], b_stat.var[0]
    av = a.tobytes()
    bv = b.tobytes()
    ab = sum(x * y for x, y in zip(av, bv)) / max(1, len(av))
    covariance = ab - mean_a * mean_b
    c1 = (0.01 * 255) ** 2
    c2 = (0.03 * 255) ** 2
    numerator = (2 * mean_a * mean_b + c1) * (2 * covariance + c2)
    denominator = (mean_a * mean_a + mean_b * mean_b + c1) * (var_a + var_b + c2)
    if denominator <= 1e-9:
        return 1.0
    return max(0.0, min(1.0, numerator / denominator))


def quality_metrics(reference, candidate):
    """Metrics used by auto-tier. Lower MAE and higher PSNR/SSIM are better."""
    ref = reference.convert("RGBA")
    dst = candidate.convert("RGBA")
    if ref.size != dst.size:
        raise ValueError(f"image size mismatch: {ref.size} != {dst.size}")
    rgba_diff = ImageChops.difference(ref, dst)
    alpha_mae = ImageStat.Stat(rgba_diff.getchannel("A")).mean[0]
    ref_luma = ImageOps.grayscale(ref)
    dst_luma = ImageOps.grayscale(dst)
    ref_edge = ref_luma.filter(ImageFilter.FIND_EDGES)
    dst_edge = dst_luma.filter(ImageFilter.FIND_EDGES)
    edge_mae = ImageStat.Stat(ImageChops.difference(ref_edge, dst_edge)).mean[0]
    return {
        "psnr": round(psnr(ref, dst), 4),
        "ssim": round(_global_ssim(ref_luma, dst_luma), 6),
        "edge_mae": round(edge_mae, 4),
        "alpha_mae": round(alpha_mae, 4),
    }


def quality_passes(metrics, min_psnr, min_ssim, max_edge_mae, max_alpha_mae):
    return (metrics["psnr"] >= min_psnr
            and metrics["ssim"] >= min_ssim
            and metrics["edge_mae"] <= max_edge_mae
            and metrics["alpha_mae"] <= max_alpha_mae)


def sensitive_name_reason(name):
    """Conservative name fallback for metadata-light sensitive textures."""
    normalized = re.sub(r"[^a-z0-9]+", " ", name.casefold()).strip()
    tokens = set(normalized.split())
    compact = normalized.replace(" ", "")
    if name.endswith(" Atlas") or tokens.intersection(("font", "sdf")):
        return "font-name"
    if tokens.intersection(("lut", "lookup")) or "normalmap" in compact:
        return "data-texture-name"
    if "color grading" in normalized or "colour grading" in normalized:
        return "data-texture-name"
    # These textures are sampled as alpha/lighting data rather than ordinary
    # colour artwork.  Global image metrics can rate a mostly-empty mask as
    # safe even when a small alpha error becomes a large on-screen halo.
    if any(word in compact for word in
           ("mask", "vignette", "halo", "glow", "gradient", "blur", "fog")):
        return "effect-texture-name"
    return None


def image_passes(reference, candidate):
    """Gate the actual native-size result, including visible alpha edges."""
    if not quality_passes(quality_metrics(reference, candidate), *QUALITY_LIMITS):
        return False
    for color in ((0, 0, 0, 255), (255, 255, 255, 255)):
        background = Image.new("RGBA", reference.size, color)
        if not quality_passes(quality_metrics(Image.alpha_composite(background, reference),
                                              Image.alpha_composite(background, candidate)), *QUALITY_LIMITS):
            return False
    return True


def keep_reason(texture, key, fonts):
    if cpu_pixels_protected(texture):
        return "cpu-readable-or-unknown"
    if key in fonts or sensitive_name_reason(texture.m_Name):
        return "font-or-data-texture"
    if (getattr(texture, "m_ImageCount", 1) != 1 or
            getattr(texture, "m_TextureDimension", 2) != 2 or
            getattr(texture, "m_PlatformBlob", None) or getattr(texture, "m_MipsStripped", 0)):
        return "special-texture-layout"
    if texture.m_TextureFormat not in COLOR_FORMATS and astc_block_of(texture.m_TextureFormat) is None:
        return "non-color-format"
    return None
