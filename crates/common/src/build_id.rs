//! Same-build identity shared by the loader and every capability cdylib.
//!
//! Build scripts inject `RUDROID_RUNTIME_BUILD_ID` (see ticket 06); when it
//! is absent — e.g. plain `cargo test` on a dev machine — every member falls
//! back to the same default so the kit still loads coherently. Kits are
//! same-build only: the loader refuses capabilities whose id differs.
#![forbid(unsafe_code)]

use std::sync::OnceLock;

/// Compile-time build id injected by each crate's build script via
/// `cargo:rustc-env=RUDROID_RUNTIME_BUILD_ID=...`.
pub const BUILD_ID_ENV: &str = "RUDROID_RUNTIME_BUILD_ID";

/// Fallback used when the build did not inject an id.
pub const DEFAULT_BUILD_ID: &str = "unconfigured-rust-kit";

/// Build id baked into this compilation unit. `option_env!` needs the literal
/// name; `BUILD_ID_ENV` stays the documented single source for build scripts.
pub const fn compiled_build_id() -> &'static str {
    match option_env!("RUDROID_RUNTIME_BUILD_ID") {
        Some(id) => id,
        None => DEFAULT_BUILD_ID,
    }
}

/// Runtime override installed by the loader after it reads the deploy kit;
/// later capability checks compare against this value.
static ACTIVE_BUILD_ID: OnceLock<&'static str> = OnceLock::new();

/// Install the kit-wide build id for the process lifetime. Returns the id that
/// was active before (`None` on first install). A second install with a
/// different id is rejected to keep same-build kits honest.
pub fn install_active_build_id(id: &'static str) -> Result<Option<&'static str>, &'static str> {
    let previous = ACTIVE_BUILD_ID.get().copied();
    match previous {
        Some(current) if current != id => Err(current),
        _ => {
            let _ = ACTIVE_BUILD_ID.set(id);
            Ok(previous)
        }
    }
}

/// Build id all capability checks must match after the loader installed one;
/// falls back to this unit's compiled id before installation.
pub fn active_build_id() -> &'static str {
    ACTIVE_BUILD_ID.get().copied().unwrap_or_else(compiled_build_id)
}

/// Kits are same-build only: a capability whose descriptor reports a
/// different `runtime_build_id` must be refused before its code runs.
#[cfg(test)]
#[path = "../tests/unit/build_id_mismatch_kit_tests.rs"]
mod mismatch_kit_tests;

/// Returns `Err` with the mismatching ids when `candidate` differs from the
/// active kit build id.
pub fn check_same_build(candidate: &str) -> Result<(), (&'static str, &str)> {
    let active = active_build_id();
    if active == candidate {
        Ok(())
    } else {
        Err((active, candidate))
    }
}

#[cfg(test)]
#[path = "../tests/unit/build_id.rs"]
mod tests;
