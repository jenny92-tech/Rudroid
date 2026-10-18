//! INPUT:  cargo build-script environment (TARGET/HOST/PROFILE/OUT_DIR) and a crate's fixed C/assembly sources
//! OUTPUT:  private PIC static archives, compile-only ABI gates and the matching cargo link directives
//! POS:    the single build-time native toolchain contract for every capability with a C boundary
//!
//! Every capability that keeps a C or assembly ABI boundary used to spell
//! out its own gcc/ar invocation; the copies drifted (one crate compiled
//! debug builds at `-O2`, others at `-O0`). This crate is the one place the
//! compiler, the shared flag set and the optimization rule live.
//!
//! The flags are fixed source code, so they are part of the kit fingerprint
//! (`common/kit_hash.rs` hashes every workspace `.rs`). `CC`/`CFLAGS`
//! overrides are intentionally not consumed: the supported Docker image pins
//! GCC, and an environment override would change the code without changing
//! the kit id. Build-script-only: never a normal dependency of a capability.
#![forbid(unsafe_code)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod vulkan_wsi;
pub mod portable_wayland;

/// The only target with native boundaries; other targets compile nothing.
pub const NATIVE_TARGET: &str = "aarch64-unknown-linux-gnu";

/// Flags every native boundary source is compiled with.
pub const COMMON_FLAGS: [&str; 5] = ["-fPIC", "-fvisibility=hidden", "-Wall", "-Wextra", "-Werror"];

/// One crate's native boundary.
pub struct NativeSources<'a> {
    /// Static archive stem (`lib<stem>.a`, linked statically into this crate
    /// only). `None` compiles the sources as a compile-time ABI gate and
    /// links nothing from them.
    pub archive: Option<&'a str>,
    /// `.c` / `.S` paths relative to the crate root, compiled in order.
    pub sources: &'a [&'a str],
    /// Included headers relative to the crate root; tracked for rebuilds only.
    pub headers: &'a [&'a str],
    /// Crate-specific flags appended after [`COMMON_FLAGS`] for every source.
    pub extra_flags: &'a [&'a str],
    /// Host libraries the boundary needs (`cargo:rustc-link-lib=<name>`).
    pub link: &'a [&'a str],
}

/// Optimization level for the cargo profile: release boundaries are `-O2`,
/// every other profile `-O0` (the same rule for every crate).
pub fn optimization(profile: &str) -> &'static str {
    if profile == "release" {
        "-O2"
    } else {
        "-O0"
    }
}

/// Full compiler argument list for one source; `.c` files add `-std=c11`,
/// assembly does not.
pub fn compile_args(source: &str, profile: &str, extra_flags: &[&str], object: &Path) -> Vec<String> {
    let mut args = Vec::new();
    if source.ends_with(".c") {
        args.push("-std=c11".to_owned());
    }
    args.extend(COMMON_FLAGS.iter().map(|flag| (*flag).to_owned()));
    args.extend(extra_flags.iter().map(|flag| (*flag).to_owned()));
    args.push(optimization(profile).to_owned());
    args.push("-c".to_owned());
    args.push(source.to_owned());
    args.push("-o".to_owned());
    args.push(object.display().to_string());
    args
}

/// Object file name for a source: the full file name keeps `jump.c` and
/// `jump.S` apart.
pub fn object_name(source: &str) -> String {
    let file = Path::new(source)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.replace('/', "_"));
    format!("{file}.o")
}

/// GNU compiler and archiver for this build: native names inside the arm64
/// container, the `aarch64-linux-gnu-` prefix when cross compiling.
pub fn toolchain(host: &str) -> (&'static str, &'static str) {
    if host == NATIVE_TARGET {
        ("gcc", "ar")
    } else {
        ("aarch64-linux-gnu-gcc", "aarch64-linux-gnu-ar")
    }
}

/// Compile `native` for the current build script. Rebuild tracking is
/// always emitted; returns `false` without compiling anything when the
/// target is not [`NATIVE_TARGET`]. Panics with the failing source named,
/// which fails the build script.
pub fn build(native: &NativeSources) -> bool {
    for path in native.sources.iter().chain(native.headers) {
        println!("cargo:rerun-if-changed={path}");
    }
    let target = env::var("TARGET").expect("TARGET");
    if target != NATIVE_TARGET {
        return false;
    }
    let host = env::var("HOST").expect("HOST");
    let profile = env::var("PROFILE").unwrap_or_default();
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let (compiler, archiver) = toolchain(&host);
    let mut objects = Vec::new();
    for source in native.sources {
        let object = out.join(object_name(source));
        let status = Command::new(compiler)
            .args(compile_args(source, &profile, native.extra_flags, &object))
            .status()
            .unwrap_or_else(|error| panic!("GNU compiler {compiler} required for {source}: {error}"));
        assert!(status.success(), "native boundary compilation failed: {source}");
        objects.push(object);
    }
    if let Some(archive) = native.archive {
        let status = Command::new(archiver)
            .arg("crs")
            .arg(out.join(format!("lib{archive}.a")))
            .args(&objects)
            .status()
            .unwrap_or_else(|error| panic!("GNU archiver {archiver} required for {archive}: {error}"));
        assert!(status.success(), "native boundary archive failed: {archive}");
        println!("cargo:rustc-link-search=native={}", out.display());
        println!("cargo:rustc-link-lib=static={archive}");
    }
    for library in native.link {
        println!("cargo:rustc-link-lib={library}");
    }
    true
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
