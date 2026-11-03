//! Kit identity against the real loader path (ticket 57).
//!
//! Two properties, both exercised through `load_deploy_dir` — the same
//! descriptor pipeline the CLI uses:
//!
//! 1. every deployed kit member of one controlled build carries exactly the
//!    loader's own `runtime_build_id`;
//! 2. a member rebuilt under a different explicit CI kit name (a real
//!    `cargo build`, not a handwritten id) is refused with the structured
//!    `BuildIdMismatch` error — the mixing experiment in miniature.
//!
//! Review 2026-09-22 adds the descriptor guards in front of the id check
//! (foreign `abi_version` read first, empty id refused) and the test-build
//! refusal: a member built with dev-dependency test features carries the
//! kit id unchanged, so its test-only export is what the loader refuses.

use std::path::{Path, PathBuf};

use rudroid_common::compiled_build_id;
use rudroid_test_support::cargo::{
    build_fixture_features, build_workspace, cargo_build, kit_artifact, profile_dir, profile_path_in,
};
use rudroid_unity_loader::{load_deploy_dir, LoaderError, Selection};

/// Every workspace cdylib, by artifact stem (capability id); the loader
/// binary itself is covered by `compiled_build_id()` in the assertions.
const KIT_MEMBER_ARTIFACTS: &[&str] = &[
    "platform_log_stderr",
    "android_base",
    "android_elf_load",
    "android_bionic",
    "android_jni_vm",
    "rudroid_android_reply_catalog",
    "unity_2021_3",
    "unity_6",
    "il2cpp_patch",
    "platform_sdl_runtime",
    "sdk_google_gms",
    "sdk_google_play_core",
    "sdk_fmod",
    "sdk_unity_burst",
    "sdk_unity_helpers",
    "sdk_xlua",
    "silksong_unity6",
    "hollow_knight_viewport",
];

fn artifact(stem: &str) -> PathBuf {
    kit_artifact(stem)
}

fn deploy_full_kit(dest: &Path) {
    std::fs::create_dir_all(dest).expect("create deploy dir");
    for stem in KIT_MEMBER_ARTIFACTS {
        let source = artifact(stem);
        assert!(
            source.is_file(),
            "kit member artifact missing: {} (build the workspace first)",
            source.display()
        );
        std::fs::copy(&source, dest.join(source.file_name().unwrap())).expect("copy member");
    }
}

#[test]
fn deployed_kit_members_share_the_loader_build_id() {
    build_workspace();
    let deploy = std::env::temp_dir().join(format!("rudroid57-consistent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    deploy_full_kit(&deploy);

    let kit_id = compiled_build_id();
    assert!(kit_id.starts_with("rust-kit-"), "kit id shape: {kit_id}");

    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect("load full kit");
    assert_eq!(
        capabilities.len(),
        KIT_MEMBER_ARTIFACTS.len(),
        "every kit member must load"
    );
    let mut seen_ids = Vec::new();
    for capability in &capabilities {
        assert_eq!(
            capability.descriptor.build_id,
            kit_id,
            "member {} must carry the kit id",
            capability.descriptor.id
        );
        seen_ids.push(capability.descriptor.build_id.clone());
    }
    seen_ids.dedup();
    assert_eq!(seen_ids.len(), 1, "one kit, one id: {seen_ids:?}");
    // Handles stay open for process lifetime (pinned by design).
    std::mem::forget(capabilities);
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn foreign_named_rebuild_of_one_member_is_rejected_by_the_loader() {
    build_workspace();

    // Build a real variant of one capability under an explicit CI kit name
    // in an isolated target dir: this is build B, genuinely produced by
    // cargo with a different identity.
    const FOREIGN_KIT: &str = "rudroid57-foreign-kit";
    const MEMBER: &str = "rudroid-linux-log-stderr";
    const MEMBER_STEM: &str = "platform_log_stderr";
    let foreign_target =
        std::env::temp_dir().join(format!("rudroid57-foreign-target-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&foreign_target);
    let status = cargo_build()
        .args(["-p", MEMBER])
        .arg("--target-dir")
        .arg(&foreign_target)
        .env("RUDROID_RUNTIME_BUILD_ID", FOREIGN_KIT)
        .status()
        .expect("spawn cargo for foreign kit member");
    assert!(status.success(), "foreign kit member build failed");
    let foreign_artifact = profile_path_in(&foreign_target)
        .join(format!("lib{MEMBER_STEM}.so"));
    assert!(foreign_artifact.is_file(), "foreign artifact missing");

    // Mixed deploy: full kit A + the one member from build B.
    let deploy = std::env::temp_dir().join(format!("rudroid57-mixed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    deploy_full_kit(&deploy);
    std::fs::copy(
        &foreign_artifact,
        deploy.join(format!("lib{MEMBER_STEM}.so")),
    )
    .expect("swap in foreign member");

    let kit_id = compiled_build_id();
    let error = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect_err("mixed kit must be refused");
    match &error {
        LoaderError::BuildIdMismatch { path, kit, found } => {
            assert_eq!(
                path.file_name().and_then(|n| n.to_str()),
                Some(format!("lib{MEMBER_STEM}.so").as_str()),
                "the foreign member is named"
            );
            assert_eq!(kit, kit_id, "kit side reports the loader id");
            assert!(
                found.ends_with(&format!("-ci-{FOREIGN_KIT}")),
                "foreign side reports its CI-named id, found: {found}"
            );
            assert_ne!(kit, found, "ids must differ");
        }
        other => panic!("expected BuildIdMismatch, got: {other:?}"),
    }
    let message = error.to_string();
    assert!(
        message.contains("same-build only"),
        "actionable mixing diagnostic: {message}"
    );

    // The unmixed kit still loads: the rejection is about this member only.
    let clean = std::env::temp_dir().join(format!("rudroid57-clean-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&clean);
    deploy_full_kit(&clean);
    let capabilities = unsafe { load_deploy_dir(&clean, &Selection::All) }.expect("clean kit loads");
    std::mem::forget(capabilities);

    let _ = std::fs::remove_dir_all(&deploy);
    let _ = std::fs::remove_dir_all(&clean);
    let _ = std::fs::remove_dir_all(&foreign_target);
}

const DESCRIPTOR_FIXTURE: &str = "project/unity_loader/tests/fixtures/descriptor_capability/Cargo.toml";

fn single_member_dir(name: &str, artifact: &Path, deployed: &str) -> PathBuf {
    let deploy = std::env::temp_dir().join(format!("rudroid-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&deploy);
    std::fs::create_dir_all(&deploy).unwrap();
    std::fs::copy(artifact, deploy.join(deployed)).expect("deploy member");
    deploy
}

/// A descriptor from another kit ABI is refused on its version alone: the
/// fixture's pointer fields are unmapped addresses, so reading any later
/// field would crash this test instead of failing it.
#[test]
fn foreign_abi_descriptor_is_rejected_before_any_field_read() {
    let foreign =
        build_fixture_features(DESCRIPTOR_FIXTURE, &[], "rudroid_descriptor_capability");
    let deploy = single_member_dir("abi", &foreign, "libforeign_abi.so");
    match unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect_err("foreign ABI must be refused") {
        LoaderError::AbiVersionMismatch { path, expected, found } => {
            assert!(path.ends_with("libforeign_abi.so"), "names the member: {path:?}");
            assert_eq!(expected, rudroid_common::descriptor::PLUGIN_ABI_VERSION);
            assert_eq!(found, expected + 1);
        }
        other => panic!("expected AbiVersionMismatch, got: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&deploy);
}

/// Reference parity: `!*descriptor->id` rejects the plugin.
#[test]
fn empty_descriptor_id_is_rejected() {
    let empty = build_fixture_features(
        DESCRIPTOR_FIXTURE,
        &["empty-id"],
        "rudroid_descriptor_capability",
    );
    let deploy = single_member_dir("empty-id", &empty, "libempty_id.so");
    let error = unsafe { load_deploy_dir(&deploy, &Selection::All) }.expect_err("empty id must be refused");
    assert!(
        matches!(error, LoaderError::EmptyId { ref path } if path.ends_with("libempty_id.so")),
        "expected EmptyId, got: {error:?}"
    );
    let _ = std::fs::remove_dir_all(&deploy);
}

/// `cargo test` unifies `android_elf_load/test-fixtures` into the shared
/// target's cdylib without changing the kit id; such a member must never be
/// accepted into a kit. The variant builds into its own target subdir and is
/// checked by the CLI's `--check` mode in a child process.
#[cfg(target_os = "linux")]
#[test]
fn test_feature_member_is_refused_by_the_loader() {
    let variant = rudroid_test_support::cargo::build_variant(
        "elf-load-test-fixtures",
        "rudroid-android-elf-load",
        &["test-fixtures"],
        "android_elf_load",
    );
    assert!(
        !variant.starts_with(kit_artifact("android_elf_load").parent().unwrap()),
        "the variant must stay out of the kit's {} dir",
        profile_dir()
    );
    let deploy = single_member_dir("test-build", &variant, "libandroid_elf_load.so");
    // A fresh process: glibc dedups dlopen by SONAME, so in this binary a
    // `libandroid_elf_load.so` another test already loaded would be handed
    // back instead of the variant (the check would then inspect the wrong
    // image and the outcome would depend on test order).
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rudroid-unity-loader"))
        .arg(&deploy)
        .arg("--check")
        .output()
        .expect("spawn rudroid-unity-loader");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "a test build must be refused: {stderr}");
    assert!(
        stderr.contains("exports test-only symbol `rudroid_test_build_marker`")
            && stderr.contains("cargo build --workspace"),
        "the TestBuildMember diagnostic names the marker and the fix: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&deploy);
}
