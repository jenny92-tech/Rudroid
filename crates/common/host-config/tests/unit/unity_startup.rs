//! INPUT:  real mode/config resolution and synthetic Unity boot contracts
//! OUTPUT:  opt-in mode matrix, conflict rejection and byte-preserving boot projection tests
//! POS:    startup policy regression; does not infer game performance from configuration
use super::*;
use crate::HostConfig;

#[test]
fn only_verified_native_player_identity_can_enable_overrides() {
    let known = rudroid_common::elf_identity::ElfIdentity {
        file_len: 23_298_160,
        build_id: Some(vec![0x30, 0x9e, 0x28, 0x4d, 0x38, 0xa6, 0x8f, 0x71]),
    };
    let p = UnityStartupPolicy::new(UnityRenderThread::Multi, "").unwrap();
    assert!(p.supports_native_player(&known));
    for unknown in [
        // The complete native ELF length is required, never a section length.
        rudroid_common::elf_identity::ElfIdentity {
            file_len: 13_154_416,
            build_id: known.build_id.clone(),
        },
        rudroid_common::elf_identity::ElfIdentity {
            file_len: 23_298_159,
            build_id: known.build_id.clone(),
        },
        rudroid_common::elf_identity::ElfIdentity {
            file_len: known.file_len,
            build_id: Some(vec![1; 8]),
        },
        rudroid_common::elf_identity::ElfIdentity {
            file_len: known.file_len,
            build_id: None,
        },
    ] {
        assert!(!p.supports_native_player(&unknown));
        assert!(UnityStartupPolicy::default().supports_native_player(&unknown));
    }
}

fn policy(mode: &str, threads: Option<&str>) -> UnityStartupPolicy {
    let text = format!(
        "[runtime]\nmemory_mode={mode:?}\n[unity]\ntarget_fps=60\n{}",
        threads
            .map(|s| format!("render_thread_mode={s:?}\n"))
            .unwrap_or_default()
    );
    let config = HostConfig::parse(&text).unwrap();
    assert_eq!(config.get_i64("unity.target_fps", 0), 60);
    config.unity_startup_policy().unwrap()
}

#[test]
fn memory_mode_only_controls_rendering_after_opt_in_and_explicit_override_wins() {
    for (mode, expected) in [
        ("low_memory", UnityRenderThread::Single),
        ("balanced", UnityRenderThread::Engine),
        ("performance", UnityRenderThread::Multi),
    ] {
        assert_eq!(policy(mode, None), UnityStartupPolicy::default());
        assert_eq!(policy(mode, Some("memory_mode")).render_thread(), expected);
        for (override_, value) in [
            ("engine", UnityRenderThread::Engine),
            ("single", UnityRenderThread::Single),
            ("multi", UnityRenderThread::Multi),
        ] {
            assert_eq!(policy(mode, Some(override_)).render_thread(), value);
        }
    }
}

#[test]
fn player_arguments_are_preserved_and_conflicts_or_truncation_reject() {
    let args = "-force-gles30 -name \"keep spaces\"";
    let p = UnityStartupPolicy::new(UnityRenderThread::Multi, args).unwrap();
    assert_eq!(
        p.command_line(),
        Some("-force-gles30 -name \"keep spaces\" -force-gfx-mt")
    );
    assert_eq!(
        UnityStartupPolicy::new(UnityRenderThread::Multi, "-force-gfx-mt")
            .unwrap()
            .command_line(),
        Some("-force-gfx-mt")
    );
    assert_eq!(
        policy("low_memory", Some("memory_mode")).command_line(),
        Some("-force-gfx-direct")
    );
    for argument in [
        "-force-gfx-direct",
        "'-force-gfx-st'",
        "-force-gfx-jobs off",
        "-force-gfx-mt=0",
        "-force-gfx-mt -force-gfx-st",
    ] {
        assert!(
            UnityStartupPolicy::new(UnityRenderThread::Multi, argument).is_err(),
            "{argument}"
        );
    }
    assert!(UnityStartupPolicy::new(UnityRenderThread::Single, "-force-gfx-mt").is_err());
    assert!(UnityStartupPolicy::new(UnityRenderThread::Multi, &"x".repeat(4090)).is_err());
    assert!(UnityStartupPolicy::new(UnityRenderThread::Multi, "before\0after").is_err());
}

#[test]
fn malformed_explicit_configuration_is_not_an_engine_default() {
    for line in [
        "render_thread_mode=1",
        "render_thread_mode='fast'",
        "render_thread_mode=false",
    ] {
        let config = HostConfig::parse(&format!("[unity]\n{line}\n")).unwrap();
        assert!(config.unity_startup_policy().is_err());
    }
    let config = HostConfig::parse(
        "[unity]\nrender_thread_mode='multi'\n[package.mainIntentBundle]\nunity=1\n",
    )
    .unwrap();
    assert!(config.unity_startup_policy().is_err());
}

const BOOT: &str = "force-gfx-direct=1\ngfx-threading-mode=0\ngfx-disable-mt-rendering=1\ngfx-enable-gfx-jobs=0\njob-worker-count=1\nbuild-guid=keep-this\n";

#[test]
fn only_three_keys_change_and_original_text_is_untouched() {
    let multi = policy("performance", Some("memory_mode"));
    let result = multi.boot_config(BOOT).unwrap().unwrap();
    assert_eq!(result, "force-gfx-direct=0\ngfx-threading-mode=1\ngfx-disable-mt-rendering=0\ngfx-enable-gfx-jobs=0\njob-worker-count=1\nbuild-guid=keep-this\n");
    assert_eq!(multi.boot_config(&result).unwrap().unwrap(), result);
    assert_eq!(
        policy("low_memory", Some("memory_mode"))
            .boot_config(&result)
            .unwrap()
            .unwrap(),
        BOOT
    );
    assert_eq!(
        policy("balanced", Some("memory_mode"))
            .boot_config("arbitrary original content")
            .unwrap(),
        None
    );
    assert!(BOOT.starts_with("force-gfx-direct=1\n"));
}

#[test]
fn line_endings_unicode_and_unterminated_last_line_survive() {
    let text = BOOT.trim_end_matches('\n').replace('\n', "\r\n") + "\r\nname=艾希";
    let result = policy("performance", Some("memory_mode"))
        .boot_config(&text)
        .unwrap()
        .unwrap();
    assert!(result.starts_with("force-gfx-direct=0\r\n"));
    assert!(result.ends_with("build-guid=keep-this\r\nname=艾希"));
    assert!(!result.ends_with('\n'));
}

#[test]
fn unknown_boot_contracts_and_excessive_text_reject() {
    let p = policy("performance", Some("memory_mode"));
    for text in [
        String::new(),
        BOOT.replacen("gfx-threading-mode=0\n", "", 1),
        format!("{BOOT}force-gfx-direct=1\n"),
        BOOT.replace("gfx-threading-mode=0", "gfx-threading-mode=2"),
        BOOT.replace("force-gfx-direct=1", "force-gfx-direct"),
        format!("{BOOT}\0"),
        format!("{BOOT}{}", "x".repeat(MAX_BOOT_CONFIG_BYTES)),
    ] {
        assert!(p.boot_config(&text).is_err());
    }
}
