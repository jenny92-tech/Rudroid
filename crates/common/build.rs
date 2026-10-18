//! INPUT:  workspace sources/assets, Cargo manifests/lockfile and codegen/feature environment
//! OUTPUT:  kit-wide runtime_build_id and Cargo rebuild watch paths
//! POS:    stamps every capability and loader with one content-derived kit identity
//! Inject the kit-wide `runtime_build_id` at compile time (ticket 57).
//!
//! The id is a content fingerprint of the whole kit, not of this crate: every
//! workspace member's Rust/C/header/assembly sources, embedded RGBA assets and Cargo manifests, the workspace
//! `Cargo.lock` (dependency versions), the target triple, the codegen-affecting
//! build parameters cargo exposes here (`PROFILE`, `OPT_LEVEL`, `DEBUG`,
//! `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_PROFILE_RELEASE_LTO`) and the enabled feature surface
//! (`CARGO_FEATURE_*` plus `RUDROID_KIT_FEATURES` from `build-kit.sh`).
//! Every capability cdylib links `rudroid-common`, so every descriptor in a
//! build carries the same id — kits are same-build only, and the loader
//! refuses members whose id differs.
//!
//! `RUDROID_RUNTIME_BUILD_ID` stays supported as an explicit CI/release kit
//! name: it is kept as a `-ci-<name>` suffix on the content hash, so "same id"
//! implies "same content" by construction. `crates/scripts/kit-manifest.py`
//! records and verifies the per-member ids of a distribution. No timestamps,
//! no git metadata, no absolute paths feed the hash: identical inputs build
//! identical ids anywhere, so a rebuilt kit stays internally consistent.

#[path = "kit_hash.rs"]
mod kit_hash;

use std::env;
use std::path::PathBuf;

fn main() {
    rudroid_native_build::build(&rudroid_native_build::NativeSources {
        archive: Some("rudroid_managed_gateway"),
        sources: &["src/managed_gateway.S"],
        headers: &[],
        extra_flags: &[],
        link: &[],
    });
    println!("cargo:rerun-if-env-changed=RUDROID_RUNTIME_BUILD_ID");
    println!("cargo:rerun-if-env-changed=RUDROID_KIT_FEATURES");
    println!("cargo:rerun-if-env-changed=CARGO_PROFILE_RELEASE_LTO");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = manifest_dir
        .parent()
        .expect("rudroid-common sits directly under the workspace root")
        .to_path_buf();

    let walk = kit_hash::walk_workspace(&workspace_root).expect("walk workspace sources");
    // Rerun coverage: once any rerun-if-changed is emitted, cargo tracks only
    // the listed paths, so every hashed file and every walked directory (new
    // and removed sources change directory mtimes) must be listed here.
    for dir in &walk.dirs {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    for file in &walk.files {
        println!("cargo:rerun-if-changed={}", file.absolute.display());
    }

    let ci_name = env::var("RUDROID_RUNTIME_BUILD_ID")
        .ok()
        .filter(|name| !name.is_empty());
    if let Some(name) = &ci_name {
        if let Err(reason) = kit_hash::validate_ci_name(name) {
            panic!("RUDROID_RUNTIME_BUILD_ID is not a valid kit name: {reason}");
        }
    }

    let target = env::var("TARGET").unwrap_or_default();
    let profile = env::var("PROFILE").unwrap_or_default();
    let opt_level = env::var("OPT_LEVEL").unwrap_or_default();
    let debug = env::var("DEBUG").unwrap_or_default();
    let rustflags = env::var("RUSTFLAGS").unwrap_or_default();
    let cargo_encoded_rustflags = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    println!("cargo:rustc-check-cfg=cfg(rudroid_pgo_training)");
    if cargo_encoded_rustflags.contains("profile-generate=") {
        // Internal to common, whose selected export macro reaches every DSO.
        // Keep the Cargo feature graph identical in training and feedback builds.
        println!("cargo:rustc-cfg=rudroid_pgo_training");
    }
    let release_lto = env::var("CARGO_PROFILE_RELEASE_LTO").unwrap_or_default();
    let kit_features = env::var("RUDROID_KIT_FEATURES").unwrap_or_default();
    let mut features: Vec<String> = env::vars()
        .filter(|(name, _)| name.starts_with("CARGO_FEATURE_"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    features.sort();

    let params = kit_hash::KitParams {
        target: &target,
        profile: &profile,
        opt_level: &opt_level,
        debug: &debug,
        rustflags: &rustflags,
        cargo_encoded_rustflags: &cargo_encoded_rustflags,
        release_lto: &release_lto,
        features: &features,
        kit_features: &kit_features,
        ci_name: ci_name.as_deref(),
    };
    let id = kit_hash::kit_build_id(&params, &walk.files);

    println!("cargo:rustc-env=RUDROID_RUNTIME_BUILD_ID={id}");
}
