//! INPUT:  real workspace tree and common's kit_hash implementation
//! OUTPUT:  identity coverage and determinism regressions
//! POS:    verifies production kit inputs are included in the shared build fingerprint
//! Compile the shared kit identity hash into the common test suite
//! (ticket 57). The module's own `#[cfg(test)]` tests cover determinism,
//! input sensitivity, walk shape and CI-name rules; the tests here pin the
//! same contract against the real workspace tree.

#[path = "../kit_hash.rs"]
mod kit_hash;

use std::path::Path;

#[test]
fn real_workspace_walk_sees_every_member_and_the_lockfile() {
    // crates/, two levels up from this test binary's crate dir.
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let walk = kit_hash::walk_workspace(&workspace_root).expect("walk crates/");
    let rels: Vec<&str> = walk
        .files
        .iter()
        .map(|f| f.rel_path.as_str())
        .collect::<Vec<_>>();
    for must_see in [
        "Cargo.lock",
        "Cargo.toml",
        "common/build.rs",
        "common/src/build_id.rs",
        "capability/linux/log_stderr/src/lib.rs",
        "capability/linux/sdl_runtime/assets/mascot.rgba",
        "capability/android/elf_load/src/lib.rs",
        "capability/unity/gamepatch/silksong/src/lib.rs",
        "project/unity_loader/src/main.rs",
    ] {
        assert!(
            rels.contains(&must_see),
            "kit identity must hash {must_see} (walked {} files)",
            walk.files.len()
        );
    }
    assert!(
        !rels.iter().any(|r| r.contains("target/")),
        "build output must never feed the kit identity"
    );
    // The absolute mirror of each rel path is what build.rs emits as the
    // cargo:rerun-if-changed watch target.
    for file in walk.files.iter().take(8) {
        assert!(
            file.absolute.ends_with(&file.rel_path),
            "absolute path must mirror rel_path: {} vs {}",
            file.absolute.display(),
            file.rel_path
        );
    }
}

#[test]
fn real_workspace_walk_is_deterministic() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let first = kit_hash::walk_workspace(&workspace_root).expect("walk first");
    let second = kit_hash::walk_workspace(&workspace_root).expect("walk second");
    let rels = |w: &kit_hash::WorkspaceWalk| -> Vec<String> {
        w.files.iter().map(|f| f.rel_path.clone()).collect()
    };
    assert_eq!(rels(&first), rels(&second));
    assert!(first.files.len() > 50, "unexpectedly small source set");
}
