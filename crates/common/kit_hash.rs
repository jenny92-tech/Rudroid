//! INPUT:  workspace source/asset tree and explicit KitParams
//! OUTPUT:  deterministic workspace walk and kit build id
//! POS:    shares content fingerprint rules between build.rs and identity regressions
//! Kit-wide build identity (ticket 57).
//!
//! Shared by `build.rs` (via `#[path]`) and the unit tests (via `#[path]`), so
//! the hash that bakes `RUDROID_RUNTIME_BUILD_ID` into every descriptor is
//! exactly the hash the tests exercise. The identity covers every input that
//! can change what a kit does at runtime:
//!
//! - every workspace member's Rust/C/header/assembly sources, embedded RGBA assets and Cargo manifests plus
//!   the workspace `Cargo.lock` (dependency versions),
//! - the target triple (`TARGET`) and the codegen-affecting build parameters
//!   cargo exposes to build scripts (`PROFILE`, `OPT_LEVEL`, `DEBUG`,
//!   `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_PROFILE_RELEASE_LTO`),
//! - the enabled feature surface: `CARGO_FEATURE_*` of this build plus the
//!   kit build entry's `RUDROID_KIT_FEATURES` (see `build-kit.sh`).
//!
//! No timestamps, no git metadata, no absolute paths: identical inputs hash
//! to the identical id in any directory, so members of one kit always agree
//! and a rebuilt kit stays coherent. An explicit `RUDROID_RUNTIME_BUILD_ID`
//! (CI/release name) is kept as a `-ci-<name>` suffix on the content hash, so
//! "same id" implies "same content" by construction.

use std::fs;
use std::path::{Path, PathBuf};

/// Salt for the kit identity hash. Different from the ticket-06 source-hash
/// salt so ids issued by the incomplete scheme can never collide with
/// kit-wide ones.
const KIT_SALT: &[u8] = b"rudroid-kit-v2";

/// One hashed workspace file: path relative to the workspace root (forward
/// slashes, stable across machines) and full content bytes.
pub struct SourceFile {
    pub rel_path: String,
    pub absolute: PathBuf,
    pub bytes: Vec<u8>,
}

/// Result of walking the workspace tree for identity inputs.
pub struct WorkspaceWalk {
    /// Hashed files, sorted by `rel_path`.
    pub files: Vec<SourceFile>,
    /// Every visited directory (absolute), for `cargo:rerun-if-changed`
    /// coverage: a new file inside a watched directory must trigger a rerun.
    pub dirs: Vec<PathBuf>,
}

/// Build-parameter inputs, kept as plain strings so both build.rs and tests
/// can synthesize them.
pub struct KitParams<'a> {
    pub target: &'a str,
    pub profile: &'a str,
    pub opt_level: &'a str,
    pub debug: &'a str,
    pub rustflags: &'a str,
    pub cargo_encoded_rustflags: &'a str,
    /// Release LTO environment override, including the fixed entry's opt-in.
    pub release_lto: &'a str,
    /// Sorted `CARGO_FEATURE_*` entries (`NAME=value`) of this build.
    pub features: &'a [String],
    /// Raw `RUDROID_KIT_FEATURES` from the kit build entry; unset and empty
    /// are the same input.
    pub kit_features: &'a str,
    /// Explicit CI/release kit name, if any.
    pub ci_name: Option<&'a str>,
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn fold(hash: u64, bytes: &[u8]) -> u64 {
    hash.rotate_left(13) ^ fnv1a64(bytes)
}

/// Walk the workspace root collecting the identity inputs: every Rust/C/header/assembly source, RGBA asset and
/// `Cargo.toml` below `root`, plus `root/Cargo.lock`. `target` and `.git`
/// directories are skipped entirely (build output must never feed the
/// identity, and fixture `Cargo.lock`s outside the root are test setup, not
/// kit content).
pub fn walk_workspace(root: &Path) -> std::io::Result<WorkspaceWalk> {
    let mut walk = WorkspaceWalk {
        files: Vec::new(),
        dirs: Vec::new(),
    };
    walk_dir(root, root, &mut walk)?;
    // Dependency versions are kit identity: the root lockfile joins the file
    // set even though the walk only takes `Cargo.toml` manifests.
    let lockfile = root.join("Cargo.lock");
    if lockfile.is_file() {
        walk.files.push(SourceFile {
            rel_path: "Cargo.lock".to_owned(),
            absolute: lockfile.clone(),
            bytes: fs::read(&lockfile)?,
        });
    }
    walk.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    walk.dirs.sort();
    Ok(walk)
}

fn walk_dir(root: &Path, dir: &Path, walk: &mut WorkspaceWalk) -> std::io::Result<()> {
    walk.dirs.push(dir.to_path_buf());
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            // Never descend into build output or repository internals.
            if name == "target" || name == ".git" {
                continue;
            }
            walk_dir(root, &dir.join(&*name), walk)?;
        } else if file_type.is_file()
            && ([".rs", ".c", ".h", ".S", ".rgba"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
                || name == "Cargo.toml")
        {
            let absolute = dir.join(&*name);
            let bytes = fs::read(&absolute)?;
            let rel_path = absolute
                .strip_prefix(root)
                .expect("walked path is under the workspace root")
                .to_string_lossy()
                .replace('\\', "/");
            walk.files.push(SourceFile {
                rel_path,
                absolute,
                bytes,
            });
        }
    }
    Ok(())
}

/// Explicit CI/release kit names must stay parseable inside the final id
/// (`rust-kit-<hash>-ci-<name>`) and safe to embed in descriptors.
pub fn validate_ci_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("must not be empty".to_owned());
    }
    if name.len() > 64 {
        return Err(format!("longer than 64 bytes ({} bytes)", name.len()));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err("only [0-9A-Za-z._-] characters are allowed".to_owned());
    }
    Ok(())
}

/// Compute the kit-wide build id from the parameter and source inputs.
/// Deterministic: the same inputs always produce the same id, so two builds
/// of one kit agree and mixed kits are detectable by construction.
pub fn kit_build_id(params: &KitParams, sources: &[SourceFile]) -> String {
    let mut hash = fnv1a64(KIT_SALT);
    for (label, value) in [
        ("target", params.target),
        ("profile", params.profile),
        ("opt-level", params.opt_level),
        ("debug", params.debug),
        ("rustflags", params.rustflags),
        ("cargo-encoded-rustflags", params.cargo_encoded_rustflags),
        ("release-lto", params.release_lto),
        ("kit-features", params.kit_features),
    ] {
        hash = fold(hash, label.as_bytes());
        hash = fold(hash, value.as_bytes());
    }
    for feature in params.features {
        hash = fold(hash, b"cargo-feature");
        hash = fold(hash, feature.as_bytes());
    }
    // Sources are sorted (walk) so traversal order is content, not filesystem
    // order; each file folds its path, length and a content digest.
    for file in sources {
        hash = fold(hash, b"file");
        hash = fold(hash, file.rel_path.as_bytes());
        hash = fold(hash, &file.bytes.len().to_le_bytes());
        hash = fold(hash, &fnv1a64(&file.bytes).to_le_bytes());
    }
    let id = format!("rust-kit-{hash:016x}");
    match params.ci_name {
        Some(name) => format!("{id}-ci-{name}"),
        None => id,
    }
}

#[cfg(test)]
#[path = "tests/unit/kit_hash.rs"]
mod tests;
