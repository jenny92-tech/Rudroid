//! Dynamic-fixture assembly rules shared by the integration tests
//! (tickets 73/75 conventions, extracted by the ticket 81 rework for the
//! 84 assembly gate): the target dir is resolved the way cargo itself
//! resolves it for this run, the built artifact profile must match the
//! test run's profile, and workspace cdylibs are always rebuilt — an
//! existence check would silently load a stale same-named artifact.
//!
//! Every nested cargo call goes through [`cargo_build`], so `--locked`, the
//! profile and the kit feature selection are decided in one place.
//!
//! Kit features (review 2026-09-22 S2): `RUDROID_KIT_FEATURES` is the
//! single source of truth for nested builds — the same variable
//! `common/build.rs` folds into the kit id and the fixed Docker entry
//! exports. Nested builds forward its `--features <pkg>/<feat>` entries to
//! the matching package (or all of them for a workspace build). A run whose
//! outer feature set disagrees with the variable (e.g. a plain
//! `cargo test --features rudroid-linux-sdl-runtime/sdl`) fails loudly in
//! [`assert_kit_features`], called by the loader's SDL test binary with the
//! capability's `SDL_ENABLED`.
//!
//! Variants (no-SDL, test-fixture builds) never land in the kit's
//! `<target>/<profile>`: [`build_variant`] gives each its own target subdir.
//!
//! Guest ARM64 execution happens in Linux ARM64 containers; on other hosts
//! the consumers stay cfg-gated. The helpers only assemble paths and spawn
//! the same cargo the outer build used (`CARGO`), so a custom
//! `CARGO_TARGET_DIR` — absolute or relative — works unchanged.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// The SDL backend feature as cargo spells it on the workspace command line
/// (`docker-rust.sh --sdl`, `cross-build-linux.sh`).
pub const SDL_FEATURE: &str = "rudroid-linux-sdl-runtime/sdl";

/// The cargo binary of the outer invocation.
pub fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// The workspace root (`crates/`).
pub fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("crates root")
}

/// The target dir cargo itself uses for this run: `CARGO_TARGET_DIR` when
/// the invocation set one (relative paths resolve against the crates root,
/// the workspace cargo cwd), else the workspace `target`.
pub fn workspace_target() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                dir
            } else {
                crates_root().join(dir)
            }
        }
        None => crates_root().join("target"),
    }
}

/// The profile directory of THIS test run (`debug` under a debug test
/// binary, `release` under `cargo test --release`) — a release runner must
/// never accept a debug-built member (same-build kit rule, ticket 57).
pub fn profile_dir() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Cargo adds the explicit target's name below --target-dir. PGO must use
/// an explicit target so compiler instrumentation does not affect build scripts.
pub fn profile_path_in(target: &Path) -> PathBuf {
    profile_path_for(target, std::env::var_os("CARGO_BUILD_TARGET").as_deref())
}

fn profile_path_for(target: &Path, selected: Option<&std::ffi::OsStr>) -> PathBuf {
    let selected = selected.filter(|name| !name.is_empty());
    let root = match selected {
        Some(name) => {
            let path = Path::new(name);
            let leaf = if path.extension().is_some_and(|ext| ext == "json") {
                path.file_stem().expect("custom target name")
            } else {
                assert_eq!(path.components().count(), 1, "expected a target triple or target JSON");
                name
            };
            target.join(leaf)
        }
        None => target.to_path_buf(),
    };
    root.join(profile_dir())
}

/// The cargo arguments selecting [`profile_dir`].
pub fn profile_args() -> &'static [&'static str] {
    if cfg!(debug_assertions) {
        &[]
    } else {
        &["--release"]
    }
}

/// Platform file name of a cdylib artifact stem (Linux `.so`, macOS
/// `.dylib` for host-only assembly checks).
pub fn lib_name(artifact: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{artifact}.dylib")
    } else {
        format!("lib{artifact}.so")
    }
}

/// `<pkg>/<feat>` entries of `RUDROID_KIT_FEATURES` (build-kit.sh /
/// docker-rust.sh syntax: `--features a/x,b/y` or `--features=a/x`). Other
/// selections cannot be replayed per package and fail loudly.
pub fn kit_features() -> &'static [String] {
    static FEATURES: OnceLock<Vec<String>> = OnceLock::new();
    FEATURES.get_or_init(|| {
        parse_kit_features(&std::env::var("RUDROID_KIT_FEATURES").unwrap_or_default())
            .unwrap_or_else(|reason| panic!("RUDROID_KIT_FEATURES: {reason}"))
    })
}

fn parse_kit_features(value: &str) -> Result<Vec<String>, String> {
    let mut features = Vec::new();
    let mut tokens = value.split_whitespace();
    while let Some(token) = tokens.next() {
        let list = match token.strip_prefix("--features=") {
            Some(list) => list,
            None if token == "--features" => tokens
                .next()
                .ok_or_else(|| "`--features` without a value".to_owned())?,
            None => {
                return Err(format!(
                    "`{token}` cannot be replayed by nested test builds; \
                     use `--features <pkg>/<feat>`"
                ))
            }
        };
        for feature in list.split(',').filter(|f| !f.is_empty()) {
            if !feature.contains('/') {
                return Err(format!(
                    "`{feature}` names no package; use `<pkg>/<feat>`"
                ));
            }
            features.push(feature.to_owned());
        }
    }
    Ok(features)
}

/// Whether the kit selection enables the SDL backend.
pub fn kit_sdl() -> bool {
    kit_features().iter().any(|f| f == SDL_FEATURE)
}

/// Fail loudly when the outer build and `RUDROID_KIT_FEATURES` disagree:
/// nested builds would otherwise replace artifacts with a different feature
/// set under the same kit id. `outer_sdl` is
/// `platform_sdl_runtime::SDL_ENABLED` as seen by a test binary that links
/// the capability (test-support itself must not: every capability rlib
/// exports `rudroid_plugin_descriptor`, so two in one binary collide).
pub fn assert_kit_features(outer_sdl: bool) {
    let declared = kit_sdl();
    assert_eq!(
        outer_sdl, declared,
        "outer build sdl={outer_sdl} but RUDROID_KIT_FEATURES declares sdl={declared}; \
         run `bash crates/scripts/docker-rust.sh test [--sdl]` or export \
         RUDROID_KIT_FEATURES='--features {SDL_FEATURE}' together with the feature"
    );
}

/// Feature arguments replaying the kit selection for a nested build of
/// `package` (`None` = the whole workspace). Capabilities never link each
/// other (ticket 43), so a package only needs its own entries.
pub fn feature_args(package: Option<&str>) -> Vec<String> {
    let selected = select_features(kit_features(), package);
    if selected.is_empty() {
        Vec::new()
    } else {
        vec!["--features".to_owned(), selected.join(",")]
    }
}

/// A single selected package takes its plain feature names: `pkg/feat`
/// would address a dependency named `pkg` (elf_load dev-depends on itself,
/// so the qualified form silently enables nothing there).
fn select_features<'a>(features: &'a [String], package: Option<&str>) -> Vec<&'a str> {
    match package {
        None => features.iter().map(String::as_str).collect(),
        Some(p) => features
            .iter()
            .filter_map(|f| f.split_once('/').filter(|(pkg, _)| *pkg == p).map(|(_, feat)| feat))
            .collect(),
    }
}

/// `cargo build --locked` with this run's profile, run from the crates
/// root. Callers add the package/manifest selection.
pub fn cargo_build() -> Command {
    let mut command = Command::new(cargo());
    command
        .args(["build", "--locked"])
        .args(profile_args())
        .current_dir(crates_root());
    command
}

fn run(mut command: Command, what: &str) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("spawn cargo for {what}: {err}"));
    assert!(status.success(), "building {what} failed");
}

fn existing(artifact: PathBuf) -> PathBuf {
    assert!(
        artifact.exists(),
        "cargo build succeeded but no cdylib at {}; \
         the test resolved a different CARGO_TARGET_DIR than the build",
        artifact.display()
    );
    artifact
}

/// Build the whole kit (`cargo build --workspace`) with the kit features.
pub fn build_workspace() {
    let mut command = cargo_build();
    command.arg("--workspace").args(feature_args(None));
    run(command, "the workspace kit");
}

/// Path of a workspace kit artifact in this run's profile dir.
pub fn kit_artifact(artifact: &str) -> PathBuf {
    profile_path_in(&workspace_target()).join(lib_name(artifact))
}

/// Build a workspace cdylib with this run's profile, target dir and kit
/// features and return the artifact path. Always spawns cargo: incremental
/// rebuilds are cheap, and the same-build id changes whenever shared sources
/// change, so a stale cdylib from a previous run must never be picked up.
pub fn ensure_workspace_cdylib(package: &str, artifact: &str) -> PathBuf {
    let mut command = cargo_build();
    command.args(["-p", package]).args(feature_args(Some(package)));
    run(command, package);
    existing(kit_artifact(artifact))
}

/// Build a fixture capability that lives outside the workspace (own
/// `[workspace]` root and lock file; `manifest` is relative to the crates
/// root) into the workspace target dir, so it shares the kit id.
pub fn build_fixture(manifest: &str, artifact: &str) -> PathBuf {
    build_fixture_features(manifest, &[], artifact)
}

/// A loader fixture under `project/unity_loader/tests/fixtures/<name>`.
pub fn build_loader_fixture(name: &str, artifact: &str) -> PathBuf {
    build_fixture(&format!("project/unity_loader/tests/fixtures/{name}/Cargo.toml"), artifact)
}

/// [`build_fixture`] with the fixture's own features. Variants share one
/// artifact path, so a caller building several copies each before the next.
pub fn build_fixture_features(manifest: &str, features: &[&str], artifact: &str) -> PathBuf {
    let mut command = cargo_build();
    command
        .args(["--manifest-path", manifest, "--target-dir"])
        .arg(workspace_target());
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
    run(command, manifest);
    existing(kit_artifact(artifact))
}

/// Build a workspace package variant (different features than the kit;
/// `features` are the package's own plain names) into
/// `<target>/variants/<name>` and return the artifact path. The kit's
/// own `<target>/<profile>` is never touched, so a deploy from it cannot
/// pick the variant up. The kit id is inherited unchanged: the variant is
/// loadable by this run's loader on purpose.
pub fn build_variant(name: &str, package: &str, features: &[&str], artifact: &str) -> PathBuf {
    let target = workspace_target().join("variants").join(name);
    let mut command = cargo_build();
    command.args(["-p", package, "--target-dir"]).arg(&target);
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
    run(command, &format!("{package} variant {name}"));
    existing(profile_path_in(&target).join(lib_name(artifact)))
}

/// Compile a freestanding guest C fixture (`-shared -fPIC -nostdlib`: the
/// ELF provider binds no undefined imports) with the container's C compiler
/// into a per-process scratch dir, once per `name`. `source` is relative to
/// the crates root. A missing compiler fails the test — never a silent skip.
pub fn compile_c_fixture(source: &str, flags: &[&str], name: &str) -> PathBuf {
    static BUILT: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let mut built = BUILT
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(path) = built.get(name) {
        return path.clone();
    }
    let out_dir = std::env::temp_dir().join(format!("rudroid-c-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).expect("C fixture dir");
    let output = out_dir.join(name);
    let compiler = ["gcc", "cc"]
        .into_iter()
        .find(|cc| Command::new(cc).arg("--version").output().is_ok_and(|o| o.status.success()))
        .expect("no gcc/cc: guest C fixtures need the container's C compiler");
    let status = Command::new(compiler)
        .args(["-shared", "-fPIC", "-nostdlib"])
        .args(flags)
        .arg("-o")
        .arg(&output)
        .arg(crates_root().join(source))
        .status()
        .expect("spawn C compiler");
    assert!(status.success(), "compiling {source} {flags:?} failed");
    built.insert(name.to_owned(), output.clone());
    output
}

#[cfg(test)]
#[path = "../tests/unit/cargo.rs"]
mod tests;
