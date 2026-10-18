# INPUT:  controlled Unity object snapshots and expected encoded payload hashes
# OUTPUT:  serialized-payload, mip, metadata and untouched-object regressions
# POS:    rejects damage introduced between successful quality checks and file publication
# SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
import hashlib
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from UnityPy.enums import TextureFormat as TF
from UnityPy.streams import EndianBinaryReader
from tools.unity_astc import astc_retier as converter
from tools.unity_astc import validate_output as validator


class SerializedOutputTest(unittest.TestCase):
    def setUp(self):
        self.before = self.texture(TF.RGBA32, b"a" * 4096)
        self.after = self.texture(TF.ASTC_RGB_8x8, b"b" * 256)
        self.expected = {("CAB", 1): (hashlib.sha256(b"b" * 256).hexdigest(), TF.ASTC_RGB_8x8)}
        self.old_mesh, self.new_mesh = b"original mesh", b"original mesh"

    def texture(self, fmt, data):
        texture = SimpleNamespace(m_Name="atlas", m_Width=32, m_Height=32, m_MipCount=1,
                                  m_IsReadable=False, m_TextureFormat=fmt, image_data=data,
                                  m_CompleteImageSize=len(data), m_StreamData=None)
        texture.get_image_data = lambda: texture.image_data
        def tree():
            return {"m_Name": texture.m_Name, "m_Width": texture.m_Width, "m_Height": texture.m_Height,
                    "m_MipCount": texture.m_MipCount, "m_IsReadable": texture.m_IsReadable,
                    "m_TextureFormat": texture.m_TextureFormat, "image data": texture.image_data,
                    "m_CompleteImageSize": texture.m_CompleteImageSize, "m_StreamData": None}
        return SimpleNamespace(assets_file=SimpleNamespace(name="CAB"), path_id=1,
                               type=SimpleNamespace(name="Texture2D"), read=lambda: texture,
                               read_typetree=tree, get_raw_data=lambda: texture.image_data)

    def environment(self, texture, mesh):
        obj = SimpleNamespace(assets_file=SimpleNamespace(name="CAB"), path_id=2,
                              type=SimpleNamespace(name="Mesh"), get_raw_data=lambda: mesh)
        file = SimpleNamespace(version=8, version_engine="6000.0.61f1", version_player="5.x.x",
                               dataflags=579, files={})
        return SimpleNamespace(file=file, objects=[texture, obj])

    def validate(self):
        with (patch.object(validator.UnityPy, "load", side_effect=[
                self.environment(self.before, self.old_mesh), self.environment(self.after, self.new_mesh)]),
              patch.object(converter, "check_streams", return_value=0)):
            return validator.validate(Path("original.bundle"), Path("output.bundle"), self.expected)

    def test_valid_output_matches_the_quality_checked_payload(self):
        self.assertEqual(self.validate(), 1)

    def test_same_length_payload_damage_is_rejected(self):
        self.after.read().image_data = b"c" * 256
        with self.assertRaisesRegex(ValueError, "quality-checked payload"):
            self.validate()

    def test_missing_mip_data_is_rejected(self):
        self.after.read().image_data = b"b" * 240
        with self.assertRaisesRegex(ValueError, "incomplete ASTC"):
            self.validate()

    def test_metadata_and_nontexture_changes_are_rejected(self):
        self.after.read().m_Width = 31
        with self.assertRaisesRegex(ValueError, "invariant"):
            self.validate()
        self.after.read().m_Width = 32
        self.new_mesh = b"corrupted mesh"
        with self.assertRaisesRegex(ValueError, "invariant"):
            self.validate()

    def test_cpu_readability_and_unselected_texture_changes_are_rejected(self):
        self.after.read().m_IsReadable = True
        with self.assertRaisesRegex(ValueError, "CPU-readable"):
            self.validate()
        self.after.read().m_IsReadable = False
        self.expected = {}
        with self.assertRaisesRegex(ValueError, "invariant"):
            self.validate()

    def test_inline_pixels_cannot_hide_damage_in_the_claimed_stream(self):
        self.after.read().m_StreamData = SimpleNamespace(path="pixels.resS", offset=16, size=256)
        original_environment = self.environment

        def environment(texture, mesh):
            env = original_environment(texture, mesh)
            data = b"original-prefix!" + (b"damaged!" * 32 if texture is self.after else b"")
            reader = EndianBinaryReader(data)
            reader.flags = 0
            env.file.files["pixels.resS"] = reader
            return env

        with patch.object(self, "environment", side_effect=environment):
            with self.assertRaisesRegex(ValueError, "inline and streamed"):
                self.validate()


class SerializedLayoutTest(unittest.TestCase):
    def file(self, data, offset=24):
        return SimpleNamespace(name="CAB", reader=SimpleNamespace(bytes=data),
                               header=SimpleNamespace(data_offset=16), objects={
                                   1: SimpleNamespace(path_id=1, byte_start=offset, byte_size=8)})

    def test_selected_object_can_change_without_moving_metadata_or_other_bytes(self):
        original = bytes(range(64))
        changed = bytearray(original)
        changed[24:32] = b"encoded!"
        expected = validator.serialized_layout(self.file(original), {("CAB", 1)})
        self.assertEqual(expected, validator.serialized_layout(self.file(bytes(changed)), {("CAB", 1)}))
        for offset in (0, 16, 23, 32, 63):
            damaged = bytearray(changed)
            damaged[offset] ^= 1
            self.assertNotEqual(expected, validator.serialized_layout(self.file(bytes(damaged)), {("CAB", 1)}))
        self.assertNotEqual(expected, validator.serialized_layout(self.file(bytes(changed), 32), {("CAB", 1)}))
        self.assertNotEqual(expected, validator.serialized_layout(self.file(bytes(changed) + b"extra"), {("CAB", 1)}))

    def stream(self, data):
        reader = EndianBinaryReader(data)
        reader.flags = 0
        return SimpleNamespace(files={"pixels.resS": reader})

    def test_appended_pixels_keep_original_stream_prefix_and_exact_ownership(self):
        original = b"old audio/pixels!!"
        before = validator.stream_snapshot(self.stream(original), None, {})
        aligned = (len(original) + 15) // 16 * 16
        output = original.ljust(aligned, b"\0") + b"encoded"
        ranges = {"pixels.resS": [(aligned, 7)]}
        self.assertEqual(before, validator.stream_snapshot(self.stream(output), before, ranges))
        for damaged in (b"!" + output[1:], output + b"extra", output[:-1]):
            with self.assertRaises(ValueError):
                validator.stream_snapshot(self.stream(damaged), before, ranges)
        for invalid in ({}, {"pixels.resS": [(aligned - 1, 8)]},
                        {"pixels.resS": [(aligned, 7), (aligned, 7)]}):
            with self.assertRaises(ValueError):
                validator.stream_snapshot(self.stream(output), before, invalid)


if __name__ == "__main__":
    unittest.main()
