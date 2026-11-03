//! Ticket 50 dynamic acceptance: the legacy `[[il2cpp_patch]]` Host Config
//! table reaches IL2CPP hook providers through the real CLI run mode — not
//! just the new `[il2cpp_hooks]` dialect, and not just at unit level.
//!
//! The kit deploys the REAL workspace `il2cpp_patch` capability cdylib plus
//! a probe fixture whose init reads the same table through the Host API v0
//! getters exactly like the C++ reference plugin
//! (`config_array_size("il2cpp_patch")` + `il2cpp_patch.<index>.<field>`).
//! Guest data is synthetic only: the run intentionally ends at the missing
//! `[paths].game_files` error, after the plugin phases already ran.

use std::path::PathBuf;
use std::process::Command;

use rudroid_test_support::cargo::{build_fixture, build_loader_fixture, ensure_workspace_cdylib};

const BIN: &str = env!("CARGO_BIN_EXE_rudroid-unity-loader");

/// Build the android_base-kind probe fixture the kit topology needs.
fn build_android_base_probe() -> PathBuf {
    build_loader_fixture("probe_capability", "rudroid_probe_capability")
}

/// Build the REAL il2cpp_patch capability cdylib from this workspace.
fn build_il2cpp_patch_capability() -> PathBuf {
    ensure_workspace_cdylib("rudroid-unity-il2cpp-hook", "il2cpp_patch")
}

/// Build the patch probe fixture (outside the workspace, own lockfile).
fn build_patch_probe() -> PathBuf {
    build_fixture(
        "capability/unity/il2cpp_hook/tests/fixtures/patch_probe/Cargo.toml",
        "il2cpp_probe",
    )
}

/// The legacy dialect exactly as existing device configs write it, plus a
/// deliberate type error (`argc` as a string) and the new-dialect table so
/// the coexistence rule is exercised in the same run.
const CONFIG: &str = r#"[device]
displayWidth = 640

[paths]

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
argc = "not-a-number"

[il2cpp_hooks]
extra_probe = "Sym_Extra"
"#;

/// What the probe must observe through the Host API v0 getters: every value
/// matches the source TOML above, misses/type errors keep the probe's
/// fallback, and `config_set_string` stays an explicit v0 failure.
const EXPECTED_PROBE_LINES: &[&str] = &[
    "size=2",
    "p0.class=PlayerState ret=1",
    "p0.namespace=HollowKnight ret=1",
    "p0.method=TakeDamage ret=1",
    "p0.mode=scale ret=1",
    "p0.argc=1",
    "p0.arg=0",
    "p0.field=44",
    "p0.mult=0.5",
    "p0.enabled=1",
    "p1.class=CameraController ret=1",
    "p1.mode=trace ret=1",
    "p1.argc=-99",
    "p9.class=MISSING ret=0",
    "oob.field=-99",
    "neg.class=NEG ret=0",
    "garbage.class=GARBAGE ret=0",
    "p00.class=PlayerState ret=1",
    "hook.extra=Sym_Extra ret=1",
    "set.ret=0",
    "set.after=PlayerState ret=1",
];

#[test]
fn legacy_patch_config_reaches_hook_providers_through_cli() {
    let dir = std::env::temp_dir().join(format!(
        "rudroid-t50-il2cpp-patch-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let android_base = build_android_base_probe();
    let il2cpp_patch = build_il2cpp_patch_capability();
    let patch_probe = build_patch_probe();
    std::fs::copy(&android_base, dir.join("libandroid_base.so")).unwrap();
    std::fs::copy(&il2cpp_patch, dir.join("libil2cpp_patch.so")).unwrap();
    std::fs::copy(&patch_probe, dir.join("libil2cpp_probe.so")).unwrap();

    let config_path = dir.join("host.toml");
    std::fs::write(&config_path, CONFIG).unwrap();

    let output = Command::new(BIN)
        .arg(&dir)
        .arg("--config")
        .arg(&config_path)
        .output()
        .expect("spawn rudroid-unity-loader");
    let status = output.status;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    // The run reaches the plugin phases with the installed config, then ends
    // at the explicit missing-gamedata failure — never a fake start.
    assert!(
        !status.success(),
        "missing paths.game_files must exit non-zero, stdout: {stdout}"
    );
    assert!(
        stderr.contains("paths.game_files"),
        "the run must fail naming the missing gamedata key: {stderr}"
    );

    // The REAL capability read the legacy table: two valid patches from the
    // two [[il2cpp_patch]] entries (pre-50 this logged patches=0).
    assert!(
        stderr.contains("[IL2PATCH] patches=2 configured=2"),
        "the il2cpp_patch capability must load both legacy patches: {stderr}"
    );

    // Field-level parity, observed inside the plugin through the same
    // dotted getters the reference plugin uses.
    for expected in EXPECTED_PROBE_LINES {
        assert!(
            stderr.contains(&format!("[PATCHPROBE] {expected}")),
            "probe observation `{expected}` missing from the run: {stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
