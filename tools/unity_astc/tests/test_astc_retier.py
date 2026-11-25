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
from UnityPy import Environment
from UnityPy.enums import TextureFormat as TF
from UnityPy.export.Texture2DConverter import get_image_from_texture2d
from UnityPy.helpers.ResourceReader import get_resource_data
from UnityPy.streams import EndianBinaryReader

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
    def test_resource_aliases_cannot_hide_different_case_equivalent_owners_or_nul(self):
        bundle=SimpleNamespace(files={
            'left/PIXELS.resS': EndianBinaryReader(b'first'),
            'right/pixels.ress': EndianBinaryReader(b'second')})
        from tools.unity_astc.resource_refs import internal_resource_name
        with self.assertRaisesRegex(ValueError,'ambiguous'):
            internal_resource_name(bundle,'archive:/CAB/pixels.resS')
        with self.assertRaisesRegex(ValueError,'invalid resource path'):
            internal_resource_name(bundle,'archive:/bad\0CAB/pixels.resS')

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

    def test_duplicate_internal_basenames_cannot_validate_the_wrong_stream(self):
        environment = Environment()
        left, right = EndianBinaryReader(b"a"), EndianBinaryReader(b"wrong-file")
        environment.register_cab("left/pixels.resS", left)
        environment.register_cab("right/pixels.resS", right)
        stream = SimpleNamespace(path="archive:/left/pixels.resS", offset=0, size=4)
        obj = SimpleNamespace(type=SimpleNamespace(name="Texture2D"),
                              read=lambda: SimpleNamespace(m_StreamData=stream))
        env = SimpleNamespace(objects=[obj], get_cab=environment.get_cab,
                              file=SimpleNamespace(files={"left/pixels.resS": left, "right/pixels.resS": right}))
        # UnityPy's real registry/decoder silently picks the last basename.
        self.assertEqual(get_resource_data(stream.path, SimpleNamespace(environment=environment), 0, 4), b"wron")
        with self.assertRaisesRegex(ValueError, "ambiguous"):
            converter.check_streams(env, Path("game.bundle"))
        with self.assertRaisesRegex(ValueError, "ambiguous"):
            converter._stream_target(env.file, SimpleNamespace(m_StreamData=stream, image_data=b""))

    def test_windows_and_archive_prefixes_keep_unique_internal_node_ownership(self):
        environment = Environment()
        reader = EndianBinaryReader(b"pixels")
        environment.register_cab("folder/PIXELS.resS", reader)
        bundle = SimpleNamespace(files={"folder/PIXELS.resS": reader})
        for path in (r"C:\build\pixels.resS", "archive:/CAB/pixels.resS", "pixels.resS"):
            with self.subTest(path=path):
                stream = SimpleNamespace(path=path, offset=0, size=6)
                texture = SimpleNamespace(m_StreamData=stream, image_data=b"")
                obj = SimpleNamespace(type=SimpleNamespace(name="Texture2D"), read=lambda: texture)
                env = SimpleNamespace(file=bundle, objects=[obj], get_cab=environment.get_cab)
                self.assertEqual(converter.check_streams(env, Path("game.bundle")), 1)
                self.assertEqual(get_resource_data(path, SimpleNamespace(environment=environment), 0, 6), b"pixels")
                self.assertEqual(converter._stream_target(bundle, texture), "folder/PIXELS.resS")
                self.assertEqual(stream.path, path)


class FontReferencesTest(unittest.TestCase):
    def pointer(self, pid, fid=0):
        return {"m_FileID": fid, "m_PathID": pid}

    def obj(self, kind, pid, tree, owner=None):
        owner = owner or SimpleNamespace(name="CAB", externals=[])
        return SimpleNamespace(type=SimpleNamespace(name=kind), path_id=pid, assets_file=owner,
                               read_typetree=lambda: tree, read=lambda: SimpleNamespace(**tree))

    def test_tmp_glyph_atlases_with_ordinary_names_are_protected_without_global_material_protection(self):
        atlas = self.obj("Texture2D", 1, {})
        other = self.obj("Texture2D", 2, {})
        tmp = self.obj("MonoBehaviour", 3, {"m_AtlasTextures": [self.pointer(1)], "m_GlyphTable": []})
        unrelated = self.obj("MonoBehaviour", 4, {"m_AtlasTextures": [self.pointer(2)]})
        fonts = converter.collect_font_textures(SimpleNamespace(objects=[atlas, other, tmp, unrelated]))
        self.assertEqual(fonts, {("CAB", 1)})
        texture = Texture()
        texture.m_Name = "ordinary pixels"
        with patch.object(Texture, "image", property(lambda _: self.fail("font pixels must not decode"))):
            converted, reason = converter.convert_texture(texture, ("CAB", 1), fonts, "astc-1g")
        self.assertIsNone(converted)
        self.assertEqual(reason, "font-or-data-texture")

    def test_native_font_texture_pointer_and_invalid_external_owner(self):
        font = self.obj("Font", 1, {"m_Texture": self.pointer(2)})
        atlas = self.obj("Texture2D", 2, {})
        self.assertEqual(converter.collect_font_textures(SimpleNamespace(objects=[font, atlas])), {("CAB", 2)})
        font = self.obj("Font", 1, {"m_Texture": self.pointer(2, 1)})
        with self.assertRaisesRegex(ValueError, "invalid font texture owner"):
            converter.collect_font_textures(SimpleNamespace(objects=[font, atlas]))

    def test_opaque_unrelated_script_does_not_disable_color_conversion(self):
        script = self.obj("MonoBehaviour", 1, {})
        script.read_typetree = lambda: (_ for _ in ()).throw(ValueError("no script typetree"))
        self.assertEqual(converter.collect_font_textures(SimpleNamespace(objects=[script])), set())

    def test_ambiguous_serialized_owner_cannot_supply_font_reference_metadata(self):
        font = self.obj("Font", 1, {"m_Texture": self.pointer(2)})
        env = SimpleNamespace(objects=[font], file=SimpleNamespace(
            signature="UnityFS", files={"left/CAB": object(), "right/cab": object()}))
        with patch.object(converter.UnityPy, "load", return_value=env):
            with self.assertRaisesRegex(ValueError, "ambiguous"):
                converter.scan_font_references("font.bundle")

    def test_custom_font_default_material_and_cross_file_atlas_keep_exact_owner(self):
        source = SimpleNamespace(name="CAB-font", externals=[SimpleNamespace(path="archive:/CAB/cab-pixels")])
        target = SimpleNamespace(name="CAB-pixels", externals=[])
        font = self.obj("Font", 1, {"m_Texture": self.pointer(0), "m_DefaultMaterial": self.pointer(2)}, source)
        material = self.obj("Material", 2, {"m_SavedProperties": {
            "m_TexEnvs": [("_MainTex", {"m_Texture": self.pointer(3, 1)})]}}, source)
        atlas = self.obj("Texture2D", 3, {}, target)
        same_id_other_owner = self.obj("Texture2D", 3, {}, source)
        objects = [font, material, atlas, same_id_other_owner]
        self.assertEqual(converter.collect_font_textures(SimpleNamespace(objects=objects)), {("CAB-pixels", 3)})

    def test_legacy_tmp_atlas_and_font_named_script_cover_custom_font_schema(self):
        atlas = self.obj("Texture2D", 1, {})
        legacy = self.obj("MonoBehaviour", 2, {"atlas": self.pointer(1), "m_glyphInfoList": []})
        script = self.obj("MonoScript", 3, {"m_ClassName": "BitmapFontAsset"})
        custom = self.obj("MonoBehaviour", 4, {"m_Script": self.pointer(3), "m_Texture": self.pointer(1)})
        for font in (legacy, custom):
            self.assertEqual(converter.collect_font_textures(SimpleNamespace(objects=[atlas, script, font])), {("CAB", 1)})

    def test_cross_bundle_metadata_scan_protects_isolated_texture_worker(self):
        a = SimpleNamespace(name="CAB-font", externals=[SimpleNamespace(path=r"archive:/CAB/CAB-pixels")])
        b = SimpleNamespace(name="CAB-pixels", externals=[])
        font = self.obj("Font", 1, {"m_Texture": self.pointer(7, 1)}, a)
        tmp = self.obj("MonoBehaviour", 2, {"m_AtlasTextures": [self.pointer(7, 1)], "m_GlyphTable": []}, a)
        for root in (font, tmp):
            with self.subTest(kind=root.type.name), tempfile.TemporaryDirectory() as directory:
                env_a = SimpleNamespace(objects=[root], file=SimpleNamespace(signature="UnityFS"))
                with patch.object(converter.UnityPy, "load", return_value=env_a):
                    metadata = converter.scan_font_references("A.bundle")
                self.assertEqual(metadata, {"roots": [["cab-pixels", 7]], "materials": []})
                protected = metadata["roots"]
                texture = Texture()
                texture.m_Name, texture.image_data = "ordinary pixels", b""
                reader = EndianBinaryReader(b"x" * (texture.m_StreamData.offset + texture.m_StreamData.size))
                obj = self.obj("Texture2D", 7, {}, b)
                obj.read = lambda: texture
                env_b = SimpleNamespace(objects=[obj], get_cab=lambda _: reader,
                                        file=SimpleNamespace(signature="UnityFS", files={"old.resS": reader}))
                source, output = Path(directory) / "B.bundle", Path(directory) / "out.bundle"
                source.write_bytes(b"unchanged synthetic bundle")
                with (patch.object(converter.UnityPy, "load", return_value=env_b),
                      patch.object(Texture, "image", property(lambda _: self.fail("unprotected encoder reached")))):
                    # Control: B alone has no font metadata and enters pixel decoding.
                    with self.assertRaisesRegex(AssertionError, "unprotected encoder reached"):
                        converter.prepare_bundle(source, output, "astc-1g")
                    result = converter.prepare_bundle(source, output, "astc-1g", protected)
                self.assertEqual(result["changed"], 0)
                self.assertEqual(result["decisions"], {"font-or-data-texture": 1})
                self.assertEqual(output.read_bytes(), source.read_bytes())

    def test_three_bundle_font_material_texture_graph_excludes_unrelated_materials(self):
        a = SimpleNamespace(name="CAB-font", externals=[SimpleNamespace(path="archive:/CAB/CAB-material")])
        b = SimpleNamespace(name="CAB-material", externals=[SimpleNamespace(path=r"C:\build\CAB-pixels")])
        c = SimpleNamespace(name="CAB-pixels", externals=[])
        font = self.obj("Font", 1, {"m_DefaultMaterial": self.pointer(2, 1)}, a)
        material = self.obj("Material", 2, {"m_SavedProperties": {
            "m_TexEnvs": [("_MainTex", {"m_Texture": self.pointer(7, 1)})]}}, b)
        unrelated = self.obj("Material", 4, {"m_SavedProperties": {
            "m_TexEnvs": [("_MainTex", {"m_Texture": self.pointer(8, 1)})]}}, b)
        ordinary_script = self.obj("MonoBehaviour", 5, {"m_AtlasTextures": [self.pointer(9, 1)]}, b)
        atlas, color, other = (self.obj("Texture2D", pid, {}, c) for pid in (7, 8, 9))
        envs = [SimpleNamespace(objects=objects, file=SimpleNamespace(signature="UnityFS"))
                for objects in ([font], [material, unrelated, ordinary_script], [atlas, color, other])]
        with patch.object(converter.UnityPy, "load", side_effect=envs):
            metadata = [converter.scan_font_references(name) for name in ("A", "B", "C")]
        self.assertEqual(metadata, [
            {"roots": [["cab-material", 2]], "materials": []},
            {"roots": [], "materials": [
                {"owner": "cab-material", "path_id": 2, "textures": [["cab-pixels", 7]]},
                {"owner": "cab-material", "path_id": 4, "textures": [["cab-pixels", 8]]}]},
            {"roots": [], "materials": []}])
        # Simulate the parent transporting only the root-reachable graph keys.
        fonts = converter.collect_font_textures(envs[2], [("cab-material", 2), ("cab-pixels", 7)])
        self.assertEqual(fonts, {("cab-material", 2), ("CAB-pixels", 7)})
        self.assertNotIn(("CAB-pixels", 8), fonts)
        self.assertNotIn(("CAB-pixels", 9), fonts)


if __name__ == "__main__":
    unittest.main()
