# INPUT:  unittest, synthetic texture/stream metadata and audited object identities
# OUTPUT:  strict range/ASTC validation and original comparison regression tests
# POS:    prevents false attribution across engines, object versions and namespaces
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
from types import SimpleNamespace
import unittest

from UnityPy.enums import TextureFormat

from tools.audit_game_assets import compare, container_key, stream_location, texture_record


class ResourceAuditTest(unittest.TestCase):
    def container(self, key="base/a.bundle", mips=7, version="6000.0.61f1", **updates):
        texture = dict(assets_file="CAB-id", path_id=7, name="image", width=64, height=64, mips=mips)
        texture.update(updates)
        return {"key": key, "unity_versions": [version], "textures": [texture]}

    def test_reduced_mips_are_detected_only_for_the_same_build_and_object(self):
        original = {"containers": [self.container()]}
        result = compare(original, {"containers": [self.container(mips=5)]})
        self.assertEqual(result["reduced_mip_textures"], 1)
        for changes in ({"path_id": 8}, {"name": "another-image"}, {"width": 128}):
            result = compare(original, {"containers": [self.container(mips=5, **changes)]})
            self.assertEqual(result["paired_textures"], 0)
        result = compare(original, {"containers": [self.container(mips=5, version="6000.0.62f1")]})
        self.assertEqual(result["reduced_mip_textures"], 0)
        self.assertEqual(len(result["incompatible_unity_builds"]), 1)

    def test_duplicate_basenames_are_ambiguous_not_silently_paired(self):
        result = compare({"containers": [self.container(), self.container()]},
                         {"containers": [self.container(mips=5)]})
        self.assertEqual(result["paired_textures"], 0)
        self.assertEqual(result["ambiguous_containers"], ["base/a.bundle"])

    def test_pad_namespaces_stay_separate_and_fat32_names_can_be_paired(self):
        base = container_key("game/gamedata/assets/bin/Data/data.unity3d")
        pack = container_key("game/gamedata/assetpacks/UnityDataAssetPack/assets/bin/Data/data.unity3d")
        self.assertNotEqual(base, pack)
        self.assertEqual(container_key("assets/aa/Android/a|b.bundle"),
                         container_key("game/assets/aa/Android/0/a-b.bundle"))

    def test_stream_bounds_and_missing_files_have_different_results(self):
        env = SimpleNamespace(get_cab=lambda _name: None)
        sizes = {"a/texture.resS": 100, "b/texture.resS": 200}
        ref = ("archive:/CAB/texture.resS", 90, 20)
        self.assertFalse(stream_location(env, "a/data.unity3d", ref, sizes)["valid_range"])
        self.assertTrue(stream_location(env, "b/data.unity3d", ref, sizes)["valid_range"])
        self.assertIsNone(stream_location(env, "c/data.unity3d", ref, sizes)["valid_range"])

    def test_internal_stream_wins_over_an_external_file_with_same_name(self):
        env = SimpleNamespace(get_cab=lambda name: SimpleNamespace(Length=50) if name == "pixels.resS" else None)
        result = stream_location(env, "a.bundle", ("pixels.resS", 40, 20), {"pixels.resS": 100})
        self.assertEqual(result["storage"], "internal")
        self.assertFalse(result["valid_range"])

    def test_external_audio_resource_does_not_alias_internal_texture_stream(self):
        env = SimpleNamespace(get_cab=lambda name: SimpleNamespace(Length=100)
                              if name == "sharedassets0.assets.resS" else None)
        sizes = {"sharedassets0.resource": 1000}
        result = stream_location(env, "data.unity3d",
                                 ("sharedassets0.resource", 900, 100), sizes)
        self.assertEqual(result["storage"], "external")
        self.assertTrue(result["valid_range"])

    def test_astc_payload_must_include_tail_blocks_and_empty_placeholders_are_allowed(self):
        texture = SimpleNamespace(m_Name="image", m_Width=7, m_Height=3,
                                  m_MipCount=3, m_ImageCount=1,
                                  m_TextureFormat=TextureFormat.ASTC_RGB_8x8,
                                  m_StreamData=None, image_data=bytes(48))
        obj = SimpleNamespace(assets_file=SimpleNamespace(name="CAB"), path_id=1)
        self.assertTrue(texture_record(texture, obj, set())["astc_payload_matches"])
        texture.image_data = bytes(32)
        self.assertFalse(texture_record(texture, obj, set())["astc_payload_matches"])
        texture.m_Width = texture.m_Height = 0
        texture.image_data = b""
        self.assertIsNone(texture_record(texture, obj, set())["astc_payload_matches"])


if __name__ == "__main__":
    unittest.main()
