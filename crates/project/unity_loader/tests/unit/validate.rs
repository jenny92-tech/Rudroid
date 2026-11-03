//! INPUT:  private items of `src/validate.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/validate.rs`; declared there with `#[path]`
use super::*;

fn descriptor(id: &str, kind: Kind, required: &[&str]) -> DescriptorInfo {
    DescriptorInfo {
        id: id.to_owned(),
        kind,
        build_id: "test".to_owned(),
        target_package: None,
        required_plugins: required.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn sample_kit() -> Vec<DescriptorInfo> {
    vec![
        descriptor("unity_6", Kind::UnityProfile, &["android_base"]),
        descriptor("sdk_fmod", Kind::AndroidAddon, &["android_base"]),
        descriptor("platform_log_stderr", Kind::Logging, &[]),
        descriptor("android_base", Kind::AndroidBase, &[]),
    ]
}

#[test]
fn android_base_loads_first_then_dependency_order() {
    let ordered = validate_and_sort(&sample_kit(), None).unwrap();
    assert_eq!(ordered[0].id, "android_base");
    // Independent peers (log_stderr, fmod, unity_6) sort by (kind, id):
    // logging(1) < unity_profile(3) < android_addon(5).
    let ids: Vec<&str> = ordered[1..].iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["platform_log_stderr", "unity_6", "sdk_fmod"]);
}

#[test]
fn duplicate_ids_are_fatal() {
    let mut kit = sample_kit();
    kit.push(descriptor("android_base", Kind::AndroidBase, &[]));
    let err = validate_and_sort(&kit, None).unwrap_err();
    assert_eq!(err.code, "duplicate_plugin_id");
    assert!(err.to_json().contains("rudroid.plugin-diagnostic.v1"));
}

#[test]
fn android_base_count_is_enforced() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("unity_6", Kind::UnityProfile, &[]),
        descriptor("log", Kind::Logging, &[]),
    ];
    let err = validate_and_sort(&kit, None).unwrap_err();
    assert_eq!(err.code, "invalid_android_base_count");

    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("android_base2", Kind::AndroidBase, &[]),
        descriptor("log", Kind::Logging, &[]),
    ];
    assert_eq!(validate_and_sort(&kit, None).unwrap_err().code, "invalid_android_base_count");
}

#[test]
fn two_profiles_are_rejected() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("unity_6", Kind::UnityProfile, &[]),
        descriptor("unity_2021_3", Kind::UnityProfile, &[]),
    ];
    assert_eq!(validate_and_sort(&kit, None).unwrap_err().code, "invalid_unity_profile_count");
}

#[test]
fn missing_dependency_is_actionable() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("sdk_xlua", Kind::AndroidAddon, &["android_elf_load"]),
    ];
    let err = validate_and_sort(&kit, None).unwrap_err();
    assert_eq!(err.code, "missing_dependency");
    assert_eq!(err.required_plugin.as_deref(), Some("android_elf_load"));
}

/// Reference `multiple_logging_plugins`: at most one logging plugin in
/// the deploy dir; one stays legal.
#[test]
fn two_logging_plugins_are_rejected() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("log_a", Kind::Logging, &[]),
        descriptor("log_b", Kind::Logging, &[]),
    ];
    let err = validate_and_sort(&kit, None).unwrap_err();
    assert_eq!(err.code, "multiple_logging_plugins");
    assert!(err.message.contains("found 2"), "{err:?}");
    // One logging plugin beside the base stays fine.
    let ok: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("log_a", Kind::Logging, &[]),
    ];
    assert!(validate_and_sort(&ok, None).is_ok());
}

/// Reference `invalid_target_package`: only game patches may declare a
/// target package — a matching package name does not rescue another kind.
#[test]
fn target_package_outside_gamepatch_is_rejected() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        {
            let mut runtime = descriptor("some_runtime", Kind::Runtime, &[]);
            runtime.target_package = Some("com.game.silksong".to_owned());
            runtime
        },
    ];
    let err = validate_and_sort(&kit, Some("com.game.silksong")).unwrap_err();
    assert_eq!(err.code, "invalid_target_package");
    assert!(err.message.contains("only game patches"), "{err:?}");
}

/// Reference `duplicate_dependency`: the same capability listed twice in
/// one descriptor's manifest is a manifest bug, not a topology.
#[test]
fn duplicate_dependency_is_rejected() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("sdk_xlua", Kind::AndroidAddon, &["android_base", "android_base"]),
    ];
    let err = validate_and_sort(&kit, None).unwrap_err();
    assert_eq!(err.code, "duplicate_dependency");
    assert_eq!(err.required_plugin.as_deref(), Some("android_base"));
}

/// Ticket 83: every string field survives a JSON round-trip — quotes,
/// backslashes, newlines, tabs, control bytes and Unicode. The test
/// unescapes with a minimal decoder over exactly the escapes the writer
/// emits.
#[test]
fn diagnostic_json_escapes_control_and_structural_bytes() {
    let hostile = descriptor(
        "we\"id\\path\n\t\u{0001}中文",
        Kind::AndroidAddon,
        &["android_elf_load"],
    );
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        hostile,
    ];
    let json = validate_and_sort(&kit, None).unwrap_err().to_json();

    // Structural integrity: exactly one JSON object, the schema intact,
    // and no raw control byte inside a string value.
    assert!(json.starts_with('{') && json.ends_with('}'));
    assert!(json.contains("rudroid.plugin-diagnostic.v1"));
    assert!(!json
        .chars()
        .filter(|c| c.is_control() && *c != '\n')
        .any(|c| !matches!(c, '\n')));

    // Round-trip: decode every escape form the writer can emit.
    fn unescape(input: &str) -> String {
        let mut out = String::new();
        let mut chars = input.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('b') => out.push('\u{0008}'),
                Some('f') => out.push('\u{000C}'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16).expect("hex escape");
                    out.push(char::from_u32(code).expect("valid escape"));
                }
                other => panic!("unexpected escape {other:?}"),
            }
        }
        out
    }

    // Extract the "plugin" value and unescape it.
    let key = "\"plugin\":\"";
    let start = json.find(key).expect("plugin field") + key.len();
    let end = json[start..]
        .find("\",\"")
        .expect("plugin value terminator");
    let decoded = unescape(&json[start..start + end]);
    assert_eq!(decoded, "we\"id\\path\n\t\u{0001}中文");
}

/// Reference `plugin_catalog` rejects empty dependency items
/// (`invalid_dependency`) before the existence checks; a dropped-in
/// filter used to swallow them silently instead.
#[test]
fn empty_dependency_is_fatal() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("sdk_xlua", Kind::AndroidAddon, &["", "android_base"]),
    ];
    let err = validate_and_sort(&kit, None).unwrap_err();
    assert_eq!(err.code, "invalid_dependency");
    assert_eq!(err.plugin, "sdk_xlua");
    let json = err.to_json();
    assert!(json.contains("declares an empty dependency"), "{json}");
    assert!(json.contains("rudroid.plugin-diagnostic.v1"));
}

#[test]
fn self_dependency_is_fatal() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("loop", Kind::Runtime, &["loop"]),
    ];
    assert_eq!(validate_and_sort(&kit, None).unwrap_err().code, "self_dependency");
}

#[test]
fn dependency_cycle_is_detected() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("a", Kind::Runtime, &["b"]),
        descriptor("b", Kind::Runtime, &["a"]),
    ];
    assert_eq!(validate_and_sort(&kit, None).unwrap_err().code, "dependency_cycle");
}

#[test]
fn package_gate_mismatch_is_fatal_for_gated_patches() {
    let mut kit: Vec<DescriptorInfo> = sample_kit();
    let mut silksong = descriptor("silksong_unity6", Kind::GamePatch, &[]);
    silksong.target_package = Some("com.game.silksong".to_owned());
    kit.push(silksong);
    // Matching package loads.
    assert_eq!(validate_and_sort(&kit, Some("com.game.silksong")).unwrap().len(), 5);
    // Mismatching package is fatal with the reference diagnostic code.
    assert_eq!(
        validate_and_sort(&kit, Some("com.other")).unwrap_err().code,
        "target_package_mismatch"
    );
}

#[test]
fn diagnostic_json_matches_reference_schema_fields() {
    let kit: Vec<DescriptorInfo> = vec![
        descriptor("android_base", Kind::AndroidBase, &[]),
        descriptor("sdk_xlua", Kind::AndroidAddon, &["android_elf_load"]),
    ];
    let json = validate_and_sort(&kit, None).unwrap_err().to_json();
    for field in [
        "\"schema\":\"rudroid.plugin-diagnostic.v1\"",
        "\"severity\":\"fatal\"",
        "\"code\":\"missing_dependency\"",
        "\"plugin\":\"sdk_xlua\"",
        "\"required_plugin\":\"android_elf_load\"",
        "\"installed_plugins\":[\"android_base\",\"sdk_xlua\"]",
    ] {
        assert!(json.contains(field), "json missing {field}: {json}");
    }
}
