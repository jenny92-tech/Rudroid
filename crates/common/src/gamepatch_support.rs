//! INPUT:  typed detour slots, hook entries and config readers
//! OUTPUT:  shared detour installation, InstallEvent and config override helpers
//! POS:    common installation boundary and reporting vocabulary for title patches
//! Ticket 87 Phase B: the shared gamepatch reporting vocabulary. The two
//! Title gamepatches (hollow_knight, silksong) carry identical
//! InstallEvent/report semantics — only their hook sets differ (which stays
//! per-Title per card 87's "extract only the consistent parts" rule).
#![deny(unsafe_op_in_unsafe_fn)]

use crate::arm64_detour::{HookFn, InstalledDetour, Original};

/// Install and publish a typed original, converting failure into a report.
///
/// # Safety
/// The entry is mapped with type `F`; both modules remain mapped for every
/// call. No thread executes the target through installation and publication.
pub unsafe fn install_detour<F: HookFn>(
    behavior: &'static str,
    target: &str,
    entry: usize,
    original: &Original<F>,
    destination: F,
) -> Result<InstalledDetour, InstallEvent> {
    // SAFETY: caller's mapped-code, matching ABI and publication contract.
    unsafe { original.install(entry, destination) }.map_err(|error| InstallEvent::DetourFailed {
        behavior,
        target: target.to_owned(),
        reason: error.to_string(),
    })
}

/// Outcome of one hook install, reported by both Title gamepatches
/// (reference `post_init` per-hook logging, structured).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallEvent {
    /// Detour installed.
    Installed {
        behavior: &'static str,
        target: String,
        shape: super::arm64_detour::HookShape,
    },
    /// Target named by config does not resolve in the guest metadata.
    TargetMissing {
        behavior: &'static str,
        target: String,
    },
    /// Config switch off — nothing attempted.
    Skipped { behavior: &'static str, reason: &'static str },
    /// Detour installation failed (target left intact).
    DetourFailed {
        behavior: &'static str,
        target: String,
        reason: String,
    },
}

impl InstallEvent {
    /// One human-readable line, matching both gamepatches' log format.
    pub fn log_line(&self) -> String {
        match self {
            InstallEvent::Installed { behavior, target, shape } => {
                format!("hook installed: {behavior} target={target} shape={shape:?}")
            }
            InstallEvent::TargetMissing { behavior, target } => {
                format!("target missing: {behavior} target={target} (native behavior kept)")
            }
            InstallEvent::Skipped { behavior, reason } => {
                format!("hook skipped: {behavior} ({reason}) — no detour installed")
            }
            InstallEvent::DetourFailed { behavior, target, reason } => {
                format!("hook failed: {behavior} target={target}: {reason}")
            }
        }
    }
}

/// Config override primitive (ticket 87 Phase C): `reader(new_key)` wins;
/// if the new key yields nothing, `reader(legacy_key)` is tried; if that
/// yields nothing too, `default`. Generic over the read result so typed
/// consumers keep their exact fallback semantics — `None` means "this key
/// carries no usable value" (missing, or present with a non-convertible
/// value), which is precisely the chain-through rule `HostConfig::get_bool`
/// applies to a present-but-wrong-type value. Implemented as a closure over
/// the config so common keeps no host-config dependency (host-config
/// depends on common).
pub fn override_setting<T>(
    mut reader: impl FnMut(&str) -> Option<T>,
    new_key: &str,
    legacy_key: &str,
    default: T,
) -> T {
    if let Some(value) = reader(new_key) {
        return value;
    }
    if let Some(value) = reader(legacy_key) {
        return value;
    }
    default
}

#[cfg(test)]
#[path = "../tests/unit/gamepatch_support.rs"]
mod tests;
