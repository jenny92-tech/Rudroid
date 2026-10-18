//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

#[test]
fn vulkan_linear_rendering_preserves_the_separate_gles_filter_and_measured_output() {
    for percent in [100,75,50] {
        let mut config=HostConfig::parse(&format!("[gpu]\ngraphicsApi='vulkan'\nrenderScalePercent={percent}\nrenderScaleSharp=true\nvulkanRenderFilter='linear'\n")).unwrap();
        let dimensions=config.render_dimensions_for_output(960,720).unwrap();
        assert_eq!((dimensions.width,dimensions.height),(960*percent/100,720*percent/100));
        assert_eq!((dimensions.output_width,dimensions.output_height),(960,720));
        assert!(!dimensions.sharp);
        assert!(config.get_bool("gpu.renderScaleSharp",false));
        assert!(config.set_string("gpu.graphicsApi","gles"));
        assert!(config.render_dimensions_for_output(960,720).unwrap().sharp);
    }
    for filter in ["'sharp'","true","12"] {
        let config=HostConfig::parse(&format!("[gpu]\ngraphicsApi='vulkan'\nvulkanRenderFilter={filter}")).unwrap();
        assert!(config.render_dimensions().unwrap_err().contains("vulkanRenderFilter"));
    }
    let exact=HostConfig::parse("[gpu]\ngraphicsApi='vulkan'\nrenderWidth=640\nrenderHeight=480\nrenderScalePercent=75").unwrap();
    let dimensions=exact.render_dimensions_for_output(960,720).unwrap();
    assert_eq!((dimensions.width,dimensions.height),(640,480));
}

#[test]
fn memory_modes_defaults_overrides_and_invalid_values() {
    assert_eq!(HostConfig::parse("").unwrap().memory_policy().unwrap(), MemoryPolicy::default());
    for mode in [MemoryMode::LowMemory, MemoryMode::Balanced, MemoryMode::Performance] {
        let config = HostConfig::parse(&format!("[runtime]\nmemory_mode = {:?}\n", mode.as_str())).unwrap();
        assert_eq!(config.memory_policy().unwrap(), MemoryPolicy::for_mode(mode));
    }
    let config = HostConfig::parse("[runtime]\nmemory_mode = 'performance'\nstream_buffer_bytes = 16384\nstream_buffer_budget_mib = 2\npreload_audio = false\n").unwrap();
    let policy = config.memory_policy().unwrap();
    assert_eq!((policy.stream_buffer_bytes, policy.stream_buffer_budget_mib, policy.preload_audio), (16384, 2, false));
    for text in ["memory_mode = 'fast'", "memory_mode = true", "stream_buffer_bytes = -1", "stream_buffer_bytes = 1048577", "stream_buffer_budget_mib = 1025", "stream_buffer_budget_mib = 2.0", "preload_audio = 'false'"] {
        assert!(HostConfig::parse(&format!("[runtime]\n{text}\n")).unwrap().memory_policy().is_err(), "{text}");
    }
}

fn sample_config_path() -> String {
    format!("{}/../../../configs/unity.toml", env!("CARGO_MANIFEST_DIR"))
}

fn silksong_config_path() -> String {
    format!(
        "{}/../../../work/config.silksong.device.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn parses_in_repo_sample_config() {
    let config = HostConfig::from_path(sample_config_path()).unwrap();
    let paths = config.paths();
    assert_eq!(
        paths.game_files.as_deref(),
        Some("../gamefiles/unitytest/")
    );
    assert_eq!(
        paths.support_files.as_deref(),
        Some("../gamefiles/support_files/")
    );
    assert_eq!(paths.android_obb_dirs, ["/fakepath_obb"]);
    assert_eq!(
        paths.android_source_dirs,
        ["/fakepath_source"]
    );
    assert_eq!(
        config.package().package_name.as_deref(),
        Some("com.example.package")
    );
    assert_eq!(config.device().display_width, 640);
    assert_eq!(config.device().display_height, 480);
    // No [plugins] section in the sample: no allowlist.
    assert_eq!(config.enabled_plugins(), None);
}

#[test]
fn parses_silksong_device_config_with_unknown_sections() {
    let config = HostConfig::from_path(silksong_config_path()).unwrap();
    assert_eq!(
        config.package().package_name.as_deref(),
        Some("com.game.silksong")
    );
    assert_eq!(config.device().display_width, 960);
    assert_eq!(config.device().display_height, 720);
    // Unknown/extra sections must not break parsing and stay reachable.
    assert!(config.get_bool("game_patches.silksong_unity6.disable_touch_camera", false));
    assert_eq!(
        config.get_string("gpu.glVersionOverride", ""),
        Some("OpenGL ES 3.2 Rudroid".to_owned())
    );
    assert_eq!(config.get_i64("unity.frame_sync_interval_us", 0), 16667);
    assert!(!config.get_bool("sdk_fmod.clocked_audio_pump", true));
    assert!(config.get_bool("input.controller", false));
    assert_eq!(
        config.get_string("input.remap.a", ""),
        Some("BUTTON_A".to_owned())
    );
}

#[test]
fn unknown_keys_and_sections_are_forward_compatible() {
    let config = HostConfig::parse(
        r#"
[paths]
game_files = "x/"
brand_new_section = "ignored but kept"

[future]
anything = { nested = 7 }
"#,
    )
    .unwrap();
    assert_eq!(config.paths().game_files.as_deref(), Some("x/"));
    assert_eq!(
        config.get_string("paths.brand_new_section", ""),
        Some("ignored but kept".to_owned())
    );
    assert_eq!(config.get_i64("future.anything.nested", 0), 7);
    assert_eq!(config.get_bool("paths.brand_new_section", true), true);
}

#[test]
fn enabled_plugins_allowlist_is_exposed() {
    let config = HostConfig::parse(
        r#"
[plugins]
enabled = ["log_stderr", "android_base", "unity_6"]
"#,
    )
    .unwrap();
    assert_eq!(
        config.enabled_plugins(),
        Some(vec![
            "log_stderr".to_owned(),
            "android_base".to_owned(),
            "unity_6".to_owned()
        ])
    );
}

#[test]
fn dotted_getters_use_reference_defaults() {
    let config = HostConfig::parse("[device]\ndisplayWidth = 100\n").unwrap();
    assert_eq!(config.get_i64("device.displayWidth", 640), 100);
    assert_eq!(config.get_i64("device.displayHeight", 480), 480);
    assert_eq!(config.get_i64("device.missing", 42), 42);
    // Reference parity (ticket 50): toml++ `value<bool>()` coerces
    // integers (static_cast<bool>), so an int leaf reads as
    // true regardless of the fallback.
    assert!(config.get_bool("device.displayWidth", true));
    assert!(config.get_bool("device.displayWidth", false));
    assert_eq!(config.array_size("paths.android_obb_dirs"), 0);
    assert_eq!(
        config.get_string("nope.nope", "fallback"),
        None
    );
}

#[test]
fn device_defaults_match_reference() {
    let config = HostConfig::parse("").unwrap();
    assert_eq!(config.device(), Device::default());
}

#[test]
fn set_string_overwrites_existing_leaf_only() {
    let mut config =
        HostConfig::parse("[device]\ndisplayWidth = 1\n[paths]\ngame_files = \"a/\"\n")
            .unwrap();
    assert!(config.set_string("device.displayWidth", "not-an-int"));
    // Overwrote the leaf; reparse shows a string now.
    assert_eq!(
        config.get_string("device.displayWidth", "?"),
        Some("not-an-int".to_owned())
    );
    assert!(config.set_string("paths.new_key", "v"));
    assert_eq!(config.get_string("paths.new_key", ""), Some("v".to_owned()));
    // Missing intermediate tables are not created (reference parity).
    assert!(!config.set_string("no.such.table.key", "v"));
}

#[test]
fn inline_table_and_bundle_keys_parse() {
    let config = HostConfig::from_path(sample_config_path()).unwrap();
    assert_eq!(
        config.get_string("package.mainIntentBundle.unity", ""),
        Some("-force-gles20".to_owned())
    );
}

#[test]
fn invalid_toml_is_an_error() {
    assert!(matches!(
        HostConfig::parse("[paths\nbroken"),
        Err(ConfigError::Parse(_))
    ));
    assert!(matches!(
        HostConfig::from_path("/definitely/missing/file.toml"),
        Err(ConfigError::Io(_))
    ));
}

// ---- ticket 50: numeric array segments (legacy `[[il2cpp_patch]]`) ----

/// The legacy dialect exactly as existing device configs write it: an
/// array of tables whose fields the reference plugin reads through
/// `il2cpp_patch.<index>.<field>` dotted keys. No renaming, no migration.
fn patch_fixture() -> HostConfig {
    HostConfig::parse(
        r#"
[[il2cpp_patch]]
assembly = "Assembly-CSharp"
namespace = "HollowKnight"
class = "PlayerState"
method = "TakeDamage"
argc = 1
arg = 0
field = 44
mult = 0.5
mode = "scale"
src_field = -1
setval = 0.0
type = "f32"
enabled = true

[[il2cpp_patch]]
class = "CameraController"
method = "onEnable"
mode = "trace"
"#,
    )
    .unwrap()
}

#[test]
fn legacy_patch_array_size_is_visible() {
    let config = patch_fixture();
    // RED pre-50: the dotted walk only followed tables, so the array of
    // tables was invisible and the plugin saw zero patches.
    assert_eq!(config.array_size("il2cpp_patch"), 2);
}

#[test]
fn numeric_array_segments_reach_fields() {
    let config = patch_fixture();
    assert_eq!(
        config.get_string("il2cpp_patch.0.class", ""),
        Some("PlayerState".to_owned())
    );
    assert_eq!(
        config.get_string("il2cpp_patch.0.namespace", ""),
        Some("HollowKnight".to_owned())
    );
    assert_eq!(
        config.get_string("il2cpp_patch.0.method", ""),
        Some("TakeDamage".to_owned())
    );
    assert_eq!(
        config.get_string("il2cpp_patch.0.mode", ""),
        Some("scale".to_owned())
    );
    assert_eq!(config.get_i64("il2cpp_patch.0.field", -1), 44);
    assert_eq!(config.get_i64("il2cpp_patch.0.argc", -1), 1);
    assert_eq!(config.get_i64("il2cpp_patch.0.arg", -1), 0);
    assert_eq!(config.get_f64("il2cpp_patch.0.mult", 1.0), 0.5);
    assert!(config.get_bool("il2cpp_patch.0.enabled", false));
    // Second entry: keys it does not set are per-entry misses.
    assert_eq!(
        config.get_string("il2cpp_patch.1.class", ""),
        Some("CameraController".to_owned())
    );
    assert_eq!(config.get_string("il2cpp_patch.1.assembly", ""), None);
    assert_eq!(
        config.get_string("il2cpp_patch.1.mode", ""),
        Some("trace".to_owned())
    );
}

#[test]
fn array_segment_indexing_matches_reference_lexing() {
    let config = patch_fixture();
    // Leading zeros parse (from_chars accepts them)...
    assert_eq!(
        config.get_string("il2cpp_patch.00.class", ""),
        Some("PlayerState".to_owned())
    );
    // ...and everything the reference rejects stays a miss: out of
    // bounds, negative, explicitly signed, non-numeric, trailing garbage
    // and a numeric segment aimed at a non-array node.
    assert_eq!(config.get_string("il2cpp_patch.2.class", ""), None);
    assert_eq!(config.get_string("il2cpp_patch.-1.class", ""), None);
    assert_eq!(config.get_string("il2cpp_patch.+1.class", ""), None);
    assert_eq!(config.get_string("il2cpp_patch.x.class", ""), None);
    assert_eq!(config.get_string("il2cpp_patch.0x.class", ""), None);
    assert_eq!(config.get_string("il2cpp_patch.1x.class", ""), None);
    assert_eq!(config.get_string("il2cpp_patch.0.0", ""), None);
}

#[test]
fn type_errors_fall_back_like_tomlpp() {
    let config = HostConfig::parse(
        r#"
[[il2cpp_patch]]
class = "PlayerState"
method = "TakeDamage"
mode = 7
argc = "oops"
huge = 9223372036854775807
"#,
    )
    .unwrap();
    // A present leaf of the wrong type keeps the caller's fallback
    // (toml++ `value<T>()` returns an empty optional, the C++ getter
    // applies `value_or`); the string getter reports it as a miss (the
    // reference returns 0 and copies the fallback).
    assert_eq!(config.get_string("il2cpp_patch.0.mode", "scale"), None);
    assert_eq!(config.get_i64("il2cpp_patch.0.argc", -9), -9);
    assert_eq!(config.get_string("il2cpp_patch.0.huge", "fb"), None);
    assert_eq!(config.get_bool("il2cpp_patch.0.mode", true), true);
}

#[test]
fn permissive_conversions_match_the_reference_matrix() {
    let config = HostConfig::parse(
        r#"
[probe]
i = -10
zero = 0
big = 9223372036854775807
f = 25.0
pi = 3.14159
b = true
f_b = false
s = "42"
"#,
    )
    .unwrap();
    // int -> bool: static_cast<bool>(int64) (nonzero = true).
    assert!(config.get_bool("probe.i", false));
    assert!(!config.get_bool("probe.zero", true));
    assert!(config.get_bool("probe.big", false));
    // bool -> bool stays; float/string -> bool rejected (the caller's
    // fallback passes through either way).
    assert!(config.get_bool("probe.b", false));
    assert!(config.get_bool("probe.f", true));
    assert!(!config.get_bool("probe.f", false));
    assert!(!config.get_bool("probe.s", false));
    assert!(config.get_bool("probe.s", true));

    // int -> i64 direct; float -> i64 only when the float IS an exact
    // in-range integer; bool -> i64 (0/1); string rejected.
    assert_eq!(config.get_i64("probe.i", 0), -10);
    assert_eq!(config.get_i64("probe.f", -1), 25);
    assert_eq!(config.get_i64("probe.pi", -1), -1);
    assert_eq!(config.get_i64("probe.b", -1), 1);
    assert_eq!(config.get_i64("probe.f_b", -1), 0);
    assert_eq!(config.get_i64("probe.s", -1), -1);
    assert_eq!(config.get_i64("probe.big", -1), 9223372036854775807);

    // float -> f64 direct; int -> f64 while losslessly representable
    // (|v| <= 2^53, the toml++ largest_whole_float bound); bool and
    // string rejected.
    assert_eq!(config.get_f64("probe.f", 0.0), 25.0);
    assert_eq!(config.get_f64("probe.i", 0.0), -10.0);
    assert_eq!(config.get_f64("probe.big", -1.0), -1.0);
    assert_eq!(config.get_f64("probe.b", -1.0), -1.0);
    assert_eq!(config.get_f64("probe.s", -1.0), -1.0);
    // 2^53 exactly is still lossless, 2^53+1 is not:
    let edge = HostConfig::parse("v = 9007199254740992\n").unwrap();
    assert_eq!(edge.get_f64("v", 0.0), 9007199254740992.0);
    let over = HostConfig::parse("v = 9007199254740993\n").unwrap();
    assert_eq!(over.get_f64("v", -1.0), -1.0);

    // String retrieval has no fuzzy conversions at all.
    assert_eq!(config.get_string("probe.s", ""), Some("42".to_owned()));
    assert_eq!(config.get_string("probe.i", "fb"), None);
}

/// Reference `config_node`: empty segments never index an array, an
/// empty key misses, and a trailing dot reads the node before it.
#[test]
fn dotted_key_edges_match_the_reference_walk() {
    let config = patch_fixture();
    // RED pre-fix: the empty segment parsed as index 0.
    assert_eq!(config.get_string("il2cpp_patch..class", ""), None);
    assert_eq!(config.array_size(""), 0);
    assert_eq!(config.get_string("", "fb"), None);
    // Trailing dot: `il2cpp_patch.` is the array itself.
    assert_eq!(config.array_size("il2cpp_patch."), 2);
    assert_eq!(
        config.get_string("il2cpp_patch.0.class.", ""),
        Some("PlayerState".to_owned())
    );
    let sample = HostConfig::parse("[unity]
render_sleep_us = 8000
").unwrap();
    assert_eq!(sample.get_i64("unity.render_sleep_us.", 0), 8000);
    // Only one trailing dot ends the walk; `a..` still asks for "".
    assert_eq!(sample.get_i64("unity.render_sleep_us..", 0), 0);
    // set_string stays strict: no writable leaf after a trailing dot.
    let mut writable = sample.clone();
    assert!(!writable.set_string("unity.", "v"));
}

/// Reference `init_config`: `value_or<int32_t>` accepts lossless floats
/// (and bools) and rejects out-of-range integers.
#[test]
fn device_reads_use_tomlpp_int32_conversions() {
    let config = HostConfig::parse(
        "[device]\ndisplayWidth = 1280.0\ndisplayHeight = 720\ndisplayRotation = true\n",
    )
    .unwrap();
    assert_eq!(
        config.device(),
        Device { display_width: 1280, display_height: 720, display_rotation: 1 }
    );
    let rejected = HostConfig::parse(
        "[device]\ndisplayWidth = 1280.5\ndisplayHeight = 4294967296\ndisplayRotation = \"90\"\n",
    )
    .unwrap();
    assert_eq!(rejected.device(), Device::default());
    let edge = HostConfig::parse("[device]\ndisplayWidth = 2147483647\ndisplayHeight = -2147483649\n").unwrap();
    assert_eq!(edge.device().display_width, 2147483647);
    assert_eq!(edge.device().display_height, 480);
}

#[test]
fn set_string_rejects_array_segments_and_leaves_no_side_effects() {
    let mut config = patch_fixture();
    // The reference config_set_string only walks tables; array segments
    // are an explicit failure, never a silent write.
    assert!(!config.set_string("il2cpp_patch.0.class", "Hacked"));
    assert_eq!(
        config.get_string("il2cpp_patch.0.class", ""),
        Some("PlayerState".to_owned())
    );
    assert_eq!(config.array_size("il2cpp_patch"), 2);
    // Failed walks must not litter the document (the reference
    // config_parent_table is read-only on the way down).
    assert!(!config.set_string("brand.new.leaf", "v"));
    assert!(config.table().get("brand").is_none());
}

#[test]
fn render_policy_rejects_malformed_types_and_resolves_independent_sizes() {
    let config=HostConfig::parse("[device]\ndisplayWidth=960\ndisplayHeight=720\n[gpu]\nrenderScalePercent=75\nrenderScaleSharp=true\n").unwrap();
    let size=config.render_dimensions().unwrap();
    assert_eq!((size.output_width,size.output_height,size.width,size.height),(960,720,720,540));
    for policy in ["renderScalePercent='75'","renderWidth=512","renderScalePercent=80","renderScaleSharp='yes'"] {
        let config=HostConfig::parse(&format!("[gpu]\n{policy}\n")).unwrap();
        assert!(config.render_dimensions().is_err(),"{policy}");
    }
}

#[test]
fn measured_fullscreen_output_preserves_explicit_low_resolution_rendering() {
    let config = HostConfig::parse("[device]\ndisplayWidth=640\ndisplayHeight=480\n[gpu]\nrenderWidth=640\nrenderHeight=480\nrenderScalePercent=100\n").unwrap();
    let size = config.render_dimensions_for_output(960, 720).unwrap();
    assert_eq!((size.output_width, size.output_height, size.width, size.height), (960, 720, 640, 480));
    assert!(size.scaled());
    assert_eq!(size.point(480, 360), (320, 240));
    assert_eq!(config.device().display_width, 640);
    assert!(config.render_dimensions_for_output(0, 720).is_err());
}

#[test]
fn measured_fullscreen_output_rebases_percentage_and_native_rendering() {
    for (percent, expected) in [(50, (480, 360)), (75, (720, 540)), (100, (960, 720))] {
        let config = HostConfig::parse(&format!("[device]\ndisplayWidth=640\ndisplayHeight=480\n[gpu]\nrenderScalePercent={percent}\n")).unwrap();
        let size = config.render_dimensions_for_output(960, 720).unwrap();
        assert_eq!((size.width, size.height), expected);
    }
    let malformed = HostConfig::parse("[gpu]\nrenderWidth=640\n").unwrap();
    assert!(malformed.render_dimensions_for_output(960, 720).is_err());
}

#[test]
fn native_presentation_keeps_exact_guest_size_and_rejects_implicit_visual_changes() {
    use crate::PresentationBackend;
    assert_eq!(HostConfig::parse("").unwrap().presentation_backend().unwrap(), PresentationBackend::Internal);
    let base = "[device]\ndisplayWidth=960\ndisplayHeight=720\n[gpu]\npresentationBackend='wayland_native'\nrenderWidth=640\nrenderHeight=480\nrenderScaleSharp=false\n";
    let config = HostConfig::parse(base).unwrap();
    assert_eq!(config.presentation_backend().unwrap(), PresentationBackend::WaylandNative);
    let before = config.render_dimensions_for_output(960, 720).unwrap();
    let native = config.render_dimensions_for_output(640, 480).unwrap();
    assert_eq!((before.width, before.height), (native.width, native.height));
    assert!(before.scaled());
    assert!(!native.scaled());
    assert_eq!(native.point(320, 240), (320, 240));
    assert_eq!(before.point(480, 360), (320, 240));
    assert_eq!(config.device().display_width, 960, "configured physical request is preserved");
    for invalid in [
        base.replace("renderScaleSharp=false", "renderScaleSharp=true"),
        base.replace("renderScaleSharp=false\n", ""),
        base.replace("renderWidth=640\nrenderHeight=480\n", "renderScalePercent=75\n"),
        base.replace("'wayland_native'", "'other'"),
        base.replace("'wayland_native'", "true"),
    ] {
        assert!(HostConfig::parse(&invalid).unwrap().render_dimensions().is_err(), "{invalid}");
    }
}
