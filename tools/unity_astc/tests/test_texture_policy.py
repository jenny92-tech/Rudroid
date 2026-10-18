# INPUT:  native-size Pillow images and shared automatic quality policy
# OUTPUT:  image/alpha quality, sensitive names and ASTC payload regressions
# POS:    protects visible detail and distinguishes file payload from device memory
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
import unittest
from PIL import Image
from tools.unity_astc.texture_policy import astc_bytes, image_passes, sensitive_name_reason


class TexturePolicyTest(unittest.TestCase):
    def test_identical_images_pass_but_alpha_damage_fails(self):
        image = Image.new("RGBA", (128, 128), (80, 30, 190, 120))
        self.assertTrue(image_passes(image, image.copy()))
        damaged = Image.new("RGBA", image.size, (80, 30, 190, 255))
        self.assertFalse(image_passes(image, damaged))

    def test_font_data_and_effect_names_remain_protected(self):
        for name in ("NotoSans SDF Atlas", "Color_Grading_LUT", "Vignette White Mask",
                     "jump_double_glow", "fog-gradient-left", "trackingSilhouetteMask"):
            with self.subTest(name=name):
                self.assertIsNotNone(sensitive_name_reason(name))
        self.assertIsNone(sensitive_name_reason("Deepnest Background"))

    def test_payload_estimate_includes_every_tail_level(self):
        self.assertEqual(astc_bytes(4, 4, (4, 4), 3), 48)
        self.assertEqual(astc_bytes(256, 1, (8, 8), 9), 1056)


if __name__ == "__main__":
    unittest.main()
