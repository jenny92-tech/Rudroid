//! Deploy Selection from Host Config (ticket 09).
//!
//! When the Host Config provides a `[plugins].enabled` list it is an explicit
//! allowlist; otherwise every capability in the deploy dir loads, matching the
//! reference behavior (`PLUGIN_ARCHITECTURE.md` load order step 1). Matching
//! is by file stem — the capability id (see `crates/docs/MIGRATION-MAP.md`). An
//! allowlist entry with no deployed artifact is a fatal, actionable error
//! (reference diagnostic `plugin_allowlist_missing`).
#![forbid(unsafe_code)]

use rudroid_host_config::HostConfig;

/// Which deployed capabilities the loader should open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Load every capability found in the deploy dir (no `[plugins]` config).
    All,
    /// Load exactly these ids (file stems), in allowlist order.
    Allowlist(Vec<String>),
}

impl Selection {
    /// Reference contract (`plugin_host.cpp` allowlist validation): entries
    /// must be non-empty plugin IDs — no path separators — with a trailing
    /// `.so` stripped (so `android_base.so` selects the `android_base`
    /// stem); duplicates after stripping are rejected. A `.dylib` suffix is
    /// NOT stripped (the reference only knows `.so`; macOS deploys should
    /// list bare ids). An empty list stays an empty allowlist: it deploys
    /// nothing by choice, like the reference.
    pub fn from_config(config: &HostConfig) -> Result<Selection, SelectionError> {
        match config.enabled_plugins() {
            Some(ids) => {
                let mut validated: Vec<String> = Vec::with_capacity(ids.len());
                for id in ids {
                    if id.is_empty() {
                        return Err(SelectionError::InvalidAllowlist {
                            reason: "plugins.enabled entries must be non-empty strings"
                                .to_owned(),
                        });
                    }
                    if id.contains('/') || id.contains('\\') {
                        return Err(SelectionError::InvalidAllowlist {
                            reason: format!(
                                "plugins.enabled entries must be plugin IDs, not paths: {id}"
                            ),
                        });
                    }
                    let id = match id.strip_suffix(".so") {
                        Some(stripped) if !stripped.is_empty() => stripped.to_owned(),
                        _ => id,
                    };
                    if validated.iter().any(|seen| seen == &id) {
                        return Err(SelectionError::DuplicateAllowlistEntry { id });
                    }
                    validated.push(id);
                }
                Ok(Selection::Allowlist(validated))
            }
            None => Ok(Selection::All),
        }
    }

    /// Filter scanned entries. `Err` lists missing ids and what is installed.
    pub fn apply<'a>(
        &self,
        entries: Vec<rudroid_common::DeployEntry>,
    ) -> Result<Vec<rudroid_common::DeployEntry>, SelectionError> {
        match self {
            Selection::All => Ok(entries),
            Selection::Allowlist(ids) => {
                let installed: Vec<&str> =
                    entries.iter().map(|e| e.stem.as_str()).collect();
                let mut selected = Vec::with_capacity(ids.len());
                let mut missing = Vec::new();
                for id in ids {
                    match entries.iter().find(|e| &e.stem == id) {
                        Some(entry) => selected.push(entry.clone()),
                        None => missing.push(id.clone()),
                    }
                }
                if !missing.is_empty() {
                    return Err(SelectionError::AllowlistMissing {
                        missing,
                        installed: installed.iter().map(|s| (*s).to_owned()).collect(),
                    });
                }
                Ok(selected)
            }
        }
    }
}

#[derive(Debug)]
pub enum SelectionError {
    AllowlistMissing {
        missing: Vec<String>,
        installed: Vec<String>,
    },
    /// Reference `invalid_plugin_allowlist`: empty entries, path-shaped
    /// entries, or a non-array `plugins.enabled` (the typed Host Config
    /// cannot express the non-array case; empty and path-shaped remain).
    InvalidAllowlist { reason: String },
    /// Reference `duplicate_plugin_allowlist_entry`.
    DuplicateAllowlistEntry { id: String },
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionError::AllowlistMissing { missing, installed } => write!(
                f,
                "plugins.enabled references deployed-missing capability(ies): {} — \
                 deployed here: [{}]; install them or remove them from plugins.enabled",
                missing.join(", "),
                installed.join(", ")
            ),
            SelectionError::InvalidAllowlist { reason } => {
                write!(f, "invalid plugins.enabled allowlist: {reason}")
            }
            SelectionError::DuplicateAllowlistEntry { id } => write!(
                f,
                "plugins.enabled contains duplicate plugin: {id} — list each plugin ID only once"
            ),
        }
    }
}

impl std::error::Error for SelectionError {}

#[cfg(test)]
#[path = "../tests/unit/select.rs"]
mod tests;
