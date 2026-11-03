# INPUT:  real UnityPy setter/codecs, small odd-sized Pillow images
# OUTPUT:  complete ASTC mip-tail and metadata preservation regressions
# POS:    catches UnityPy's below-four-pixels mip truncation with actual encoding
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
from types import MethodType, SimpleNamespace
import unittest

from PIL import Image
from UnityPy.classes import Texture2D
from UnityPy.enums import TextureFormat as TF
from UnityPy.export.Texture2DConverter import astc

from tools.unity_astc.image_encode import set_image_preserving_mips
from tools.unity_astc.texture_policy import UNITY_ASTC_BLOCKS


class FullMipChainTest(unittest.TestCase):
    def texture(self, width, height, count):
        texture = SimpleNamespace(m_Width=width, m_Height=height, m_MipCount=count,
                                  m_TextureFormat=TF.RGBA32, m_MipMap=count > 1,
                                  m_PlatformBlob=None, object_reader=None,
                                  m_StreamData=SimpleNamespace(path="old.resS", offset=12, size=24))
        texture.set_image = MethodType(Texture2D.set_image, texture)
        return texture

    def test_odd_rectangular_chain_includes_the_one_pixel_tail(self):
        # The 256x1 ramp is also present in the historical Silksong audit:
        # the old setter retained only the base level of its nine-level chain.
        for width, height, count in ((7, 3, 3), (124, 60, 7), (256, 1, 9), (1, 1, 1)):
            with self.subTest(size=(width, height), count=count):
                texture = self.texture(width, height, count)
                image = Image.new("RGBA", (width, height), (90, 50, 140, 255))
                set_image_preserving_mips(texture, image, TF.ASTC_RGB_8x8)
                self.assertEqual((texture.m_Width, texture.m_Height, texture.m_MipCount),
                                 (width, height, count))
                self.assertEqual(texture.m_StreamData.path, "")
                self.assertEqual(texture.m_CompleteImageSize, len(texture.image_data))
                cursor = 0
                w, h = width, height
                for _ in range(count):
                    size = ((w + 7) // 8) * ((h + 7) // 8) * 16
                    decoded = astc(texture.image_data[cursor:cursor + size], w, h, (8, 8))
                    self.assertEqual(decoded.size, (w, h))
                    self.assertGreater(decoded.convert("RGBA").getpixel((0, 0))[3], 240)
                    cursor += size
                    w, h = max(1, w // 2), max(1, h // 2)
                self.assertEqual(cursor, len(texture.image_data))

    def test_unity_astc_block_set_matches_the_installed_texture_enums(self):
        exposed = {tuple(int(n) for n in fmt.name.removeprefix("ASTC_RGB_").split("x"))
                   for fmt in TF if fmt.name.startswith("ASTC_RGB_")}
        self.assertEqual(UNITY_ASTC_BLOCKS, exposed)

    def test_invalid_mip_count_fails_before_mutating_texture(self):
        texture = self.texture(7, 3, 99)
        with self.assertRaisesRegex(ValueError, "invalid texture dimensions/mip"):
            set_image_preserving_mips(texture, Image.new("RGBA", (7, 3)), TF.ASTC_RGB_8x8)
        self.assertEqual(texture.m_StreamData.path, "old.resS")


if __name__ == "__main__":
    unittest.main()
