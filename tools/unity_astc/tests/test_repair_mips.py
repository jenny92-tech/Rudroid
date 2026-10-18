# INPUT:  real UnityPy ASTC codecs, bounded textures and repair_mips
# OUTPUT:  payload preservation, blank effects, orientation and fail-closed regressions
# POS:    proves tail repair cannot re-encode existing art or partially mutate failures
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from PIL import Image
from UnityPy.enums import BuildTarget, TextureFormat as TF
from UnityPy.export.Texture2DConverter import astc, image_to_texture2d

from tools.unity_astc import repair_mips as repair


class MipRepairTest(unittest.TestCase):
    def texture(self, width, height, count=1, image=None):
        image = image or Image.new("RGBA", (width, height), (90, 30, 180, 255))
        fmt = TF.ASTC_RGB_6x6
        data = []
        for w, h, _offset, _size in repair.mip_layout(width, height, count, (6, 6)):
            image = image.resize((w, h), Image.Resampling.BICUBIC)
            encoded, actual = image_to_texture2d(image, fmt, BuildTarget.Android, flip=False)
            self.assertEqual(actual, fmt)
            data.append(encoded)
        data = b"".join(data)
        return SimpleNamespace(m_Width=width, m_Height=height, m_MipCount=count,
                               m_CompleteImageSize=len(data), image_data=data,
                               m_TextureFormat=fmt, m_ImageCount=1, m_TextureDimension=2,
                               m_MipsStripped=0, m_MipMap=count > 1, m_PlatformBlob=None,
                               object_reader=SimpleNamespace(platform=BuildTarget.Android),
                               m_StreamData=SimpleNamespace(path="", offset=0, size=0))

    def test_rectangular_and_one_pixel_chains_preserve_every_existing_byte(self):
        for width, height, old in ((256, 1, 1), (124, 60, 5), (64, 64, 5)):
            texture = self.texture(width, height, old)
            data = texture.image_data
            count = max(width, height).bit_length()
            result = repair.complete_mip_tail(texture, count)
            self.assertEqual(texture.image_data[:len(data)], data)
            self.assertEqual((texture.m_MipCount, texture.m_CompleteImageSize),
                             (count, len(texture.image_data)))
            self.assertEqual(result["added_bytes"], len(texture.image_data) - len(data))
            for w, h, offset, size in repair.mip_layout(width, height, count, (6, 6)):
                image = astc(texture.image_data[offset:offset + size], w, h, (6, 6))
                self.assertGreater(image.getpixel((0, 0))[3], 240)

    def test_intentionally_blank_effect_stays_blank_in_all_new_mips(self):
        texture = self.texture(64, 32, 4, Image.new("RGBA", (64, 32), (0, 0, 0, 0)))
        repair.complete_mip_tail(texture, 7)
        for w, h, offset, size in repair.mip_layout(64, 32, 7, (6, 6))[4:]:
            image = astc(texture.image_data[offset:offset + size], w, h, (6, 6))
            self.assertEqual(image.getchannel("A").getextrema(), (0, 0))

    def test_asymmetric_rows_are_not_flipped_in_new_levels(self):
        image = Image.new("RGBA", (32, 32), (0, 0, 255, 255))
        image.paste((255, 0, 0, 255), (0, 0, 32, 16))
        texture = self.texture(32, 32, image=image)
        repair.complete_mip_tail(texture, 6)
        w, h, offset, size = repair.mip_layout(32, 32, 6, (6, 6))[1]
        lower = astc(texture.image_data[offset:offset + size], w, h, (6, 6))
        self.assertGreater(lower.getpixel((4, 1))[0], 200)
        self.assertGreater(lower.getpixel((4, 14))[2], 200)

    def test_encoder_failure_leaves_all_fields_untouched(self):
        texture = self.texture(64, 32, 4)
        before = vars(texture).copy()
        encoder = repair.image_to_texture2d
        calls = 0
        def fail_second(*args, **kwargs):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise ValueError("injected codec failure")
            return encoder(*args, **kwargs)
        with patch.object(repair, "image_to_texture2d", side_effect=fail_second):
            with self.assertRaisesRegex(ValueError, "codec failure"):
                repair.complete_mip_tail(texture, 7)
        self.assertEqual(vars(texture), before)

    def test_unknown_stream_layout_format_and_mip_counts_are_rejected(self):
        for field, value in (("m_TextureFormat", TF.RGBA32), ("m_ImageCount", 2),
                             ("m_MipsStripped", 1), ("m_PlatformBlob", [1]),
                             ("m_CompleteImageSize", 1), ("m_MipCount", 99)):
            with self.subTest(field=field):
                texture = self.texture(64, 32, 4)
                setattr(texture, field, value)
                with self.assertRaises(ValueError):
                    repair.complete_mip_tail(texture, 7)
        texture = self.texture(64, 32, 4)
        texture.m_StreamData.path = "unknown.resS"
        with self.assertRaisesRegex(ValueError, "stream-owner"):
            repair.complete_mip_tail(texture, 7)

    def test_type_tree_metadata_check_visits_each_node_only_once(self):
        class Node:
            def __init__(self, name, children=()):
                self.name, self.children = name, children
                self.reads = 0
            def traverse(self):
                yield self
                for child in self.children:
                    yield from child.traverse()
            def to_dict(self):
                self.reads += 1
                return {"m_Name": self.name, "m_Children": self.children}
            def __eq__(self, _other):
                raise AssertionError("recursive third-party tree equality must not be called")
        leaf = Node("leaf")
        root = Node("root", [leaf])
        equivalent = Node("root", [Node("leaf")])
        self.assertEqual(repair.metadata_value(root), repair.metadata_value(equivalent))
        self.assertEqual((root.reads, leaf.reads), (1, 1))
        equivalent.children[0].name = "changed"
        self.assertNotEqual(repair.metadata_value(root), repair.metadata_value(equivalent))


if __name__ == "__main__":
    unittest.main()
