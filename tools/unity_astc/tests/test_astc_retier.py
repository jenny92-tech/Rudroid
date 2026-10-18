# INPUT:  synthetic Texture2D data, actual UnityPy codecs and automatic conversion
# OUTPUT:  coarse-to-fine fallback, CPU/data protection and source immutability regressions
# POS:    proves memory profiles select actual encoded results without modifying failed candidates
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
from types import SimpleNamespace
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from PIL import Image
from UnityPy.classes import Texture2D
from UnityPy.enums import TextureFormat as TF
from UnityPy.export.Texture2DConverter import get_image_from_texture2d

from tools.unity_astc import astc_retier as converter


class Texture:
    set_image = Texture2D.set_image

    def __init__(self):
        self.m_Name = "ordinary atlas"
        self.m_Width = self.m_Height = 128
        self.m_MipCount, self.m_MipMap = 8, True
        self.m_TextureFormat = TF.RGBA32
        self.m_IsReadable = False
        self.m_ImageCount, self.m_TextureDimension, self.m_MipsStripped = 1, 2, 0
        self.m_PlatformBlob = self.object_reader = None
        self.m_StreamData = SimpleNamespace(path="old.resS", offset=12, size=65536)
        self.image_data = Image.new("RGBA", (128, 128), (50, 70, 90, 255)).tobytes()
        self.m_CompleteImageSize = len(self.image_data)

    def get_image_data(self):
        return self.image_data

    @property
    def image(self):
        return get_image_from_texture2d(self)


class AutomaticConversionTest(unittest.TestCase):
    def test_empty_font_placeholder_does_not_try_to_open_a_pixel_stream(self):
        texture = Texture()
        texture.m_Width = texture.m_Height = texture.m_CompleteImageSize = 0
        texture.m_Name = "Font Texture"
        texture.get_image_data = lambda: (_ for _ in ()).throw(AssertionError("must not read pixels"))
        self.assertEqual(converter.payload_size(texture), 0)
        converted, reason = converter.convert_texture(texture, ("CAB", 1), set(), "astc-1g")
        self.assertIsNone(converted)
        self.assertEqual(reason, "font-or-data-texture")

    def test_each_memory_profile_chooses_a_passing_encoded_result_with_all_mips(self):
        for target, block in (("astc-1g", 12), ("astc-2g", 8), ("astc-4g", 4)):
            with self.subTest(target=target):
                original = Texture()
                data = original.image_data
                converted, reason = converter.convert_texture(original, ("CAB", 1), set(), target)
                self.assertEqual(converted.m_TextureFormat, getattr(TF, f"ASTC_RGB_{block}x{block}"))
                self.assertEqual(converted.m_MipCount, 8)
                self.assertEqual(reason, f"astc-{block}x{block}")
                self.assertEqual(original.image_data, data)
                self.assertEqual(original.m_StreamData.path, "old.resS")
                self.assertEqual(original.m_MipCount, 8)
                self.assertEqual(converted.m_StreamData.path, "")
                self.assertLess(len(converted.image_data), len(data))

    def test_quality_failure_retries_finer_blocks_from_original_pixels(self):
        original = Texture()
        with patch.object(converter, "image_passes", side_effect=[False, True]) as gate:
            converted, reason = converter.convert_texture(original, ("CAB", 1), set(), "astc-1g")
        self.assertEqual(reason, "astc-8x8")
        self.assertEqual(gate.call_count, 2)
        self.assertEqual(converted.m_MipCount, 8)
        self.assertEqual(original.m_StreamData.path, "old.resS")

    def test_all_quality_failures_keep_original_bytes_and_stream_reference(self):
        original = Texture()
        data = original.image_data
        with patch.object(converter, "image_passes", return_value=False):
            result, reason = converter.convert_texture(original, ("CAB", 1), set(), "astc-1g")
        self.assertIsNone(result)
        self.assertEqual(reason, "quality-preserved")
        self.assertEqual(original.image_data, data)
        self.assertEqual(original.m_StreamData.path, "old.resS")

    def test_cpu_unknown_font_and_noncolor_formats_never_enter_encoder(self):
        for field, value in (("m_IsReadable", True), ("m_IsReadable", None),
                             ("m_Name", "Chinese SDF Atlas"), ("m_Name", "Color LUT"),
                             ("m_TextureFormat", TF.BC4), ("m_TextureFormat", TF.BC5),
                             ("m_TextureFormat", TF.BC6H), ("m_TextureFormat", TF.Alpha8)):
            with self.subTest(field=field, value=value):
                texture = Texture()
                setattr(texture, field, value)
                with patch.object(converter, "image_passes", side_effect=AssertionError("must not encode")):
                    converted, _ = converter.convert_texture(texture, ("CAB", 1), set(), "astc-1g")
                self.assertIsNone(converted)

    def test_already_coarser_astc_is_not_reencoded_or_inflated(self):
        texture = Texture()
        texture.m_TextureFormat = TF.ASTC_RGB_8x8
        with patch.object(converter, "image_passes", side_effect=AssertionError("must not encode")):
            converted, reason = converter.convert_texture(texture, ("CAB", 1), set(), "astc-4g")
        self.assertIsNone(converted)
        self.assertEqual(reason, "already-small")

    def test_unknown_texture_format_keeps_payload_and_stream_without_decoding(self):
        texture = Texture()
        texture.m_TextureFormat = 0x7fffffff
        before = (texture.image_data, vars(texture.m_StreamData).copy())
        with patch.object(Texture, "image", property(lambda _: self.fail("unknown pixels must not decode"))):
            converted, reason = converter.convert_texture(texture, ("CAB", 1), set(), "astc-1g")
            self.assertEqual(converter.payload_size(texture), texture.m_CompleteImageSize)
        self.assertIsNone(converted)
        self.assertEqual(reason, "non-color-format")
        self.assertEqual((texture.image_data, vars(texture.m_StreamData)), before)

    def test_desktop_color_format_is_converted_even_without_disk_payload_saving(self):
        texture = Texture()
        texture.set_image(texture.image, target_format=TF.BC7, mipmap_count=1)
        data = texture.image_data
        converted, reason = converter.convert_texture(texture, ("CAB", 1), set(), "astc-4g")
        self.assertEqual(reason, "astc-4x4")
        self.assertEqual(converted.m_TextureFormat, TF.ASTC_RGB_4x4)
        self.assertEqual(len(converted.image_data), len(data))
        self.assertEqual(texture.m_TextureFormat, TF.BC7)
        self.assertEqual(texture.image_data, data)

    def test_unreadable_audio_reference_is_not_silently_skipped(self):
        obj = SimpleNamespace(type=SimpleNamespace(name="AudioClip"),
                              read=lambda: (_ for _ in ()).throw(ValueError("unknown audio layout")))
        with self.assertRaisesRegex(ValueError, "unknown audio layout"):
            converter.check_streams(SimpleNamespace(objects=[obj]), Path("game.bundle"))

    def test_stream_cannot_resolve_to_a_different_suffix_or_outside_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "pixels.resource").write_bytes(b"1234")
            stream = SimpleNamespace(path="pixels.resource", offset=0, size=4)
            obj = SimpleNamespace(type=SimpleNamespace(name="Texture2D"),
                                  read=lambda: SimpleNamespace(m_StreamData=stream))
            env = SimpleNamespace(objects=[obj], get_cab=lambda name: None)
            self.assertEqual(converter.check_streams(env, root / "game.bundle"), 1)
            stream.size = 5
            with self.assertRaisesRegex(ValueError, "exceeds"):
                converter.check_streams(env, root / "game.bundle")
            stream.path = "pixels.assets.resS"
            with self.assertRaisesRegex(ValueError, "missing"):
                converter.check_streams(env, root / "game.bundle")
            stream.path = ""
            with self.assertRaisesRegex(ValueError, "invalid"):
                converter.check_streams(env, root / "game.bundle")


if __name__ == "__main__":
    unittest.main()
