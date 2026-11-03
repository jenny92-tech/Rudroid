//! Descriptor validation, deterministic ordering and diagnostics
//! (ticket 31).
//!
//! Mirrors the reference `plugin_catalog::validate_and_sort` semantics:
//! duplicate ids, missing/self/cyclic dependencies, exactly one android_base,
//! at most one Unity profile, and gamepatch package gates. Diagnostics use
//! the reference's machine-readable JSON schema
//! (`rudroid.plugin-diagnostic.v1`) so launchers can parse failures.
#![forbid(unsafe_code)]

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use rudroid_common::{package_gate, Kind};

use super::DescriptorInfo;

/// A topology failure, printable as the reference's diagnostic JSON line.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: &'static str,
    pub plugin: String,
    pub required_plugin: Option<String>,
    pub message: String,
    pub hint: String,
    pub installed_plugins: Vec<String>,
}

/// Escape one string for embedding in the diagnostic JSON line: quotes,
/// backslashes, control characters (named escapes, then `\u00xx`) — Unicode
/// passes through as UTF-8, which JSON allows verbatim.
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

impl Diagnostic {
    /// One machine-readable stderr line, schema `rudroid.plugin-diagnostic.v1`.
    pub fn to_json(&self) -> String {
        let installed: Vec<String> = self
            .installed_plugins
            .iter()
            .map(|id| format!("\"{}\"", json_escape(id)))
            .collect();
        let required = self
            .required_plugin
            .as_ref()
            .map(|id| format!("\"required_plugin\":\"{}\",", json_escape(id)))
            .unwrap_or_default();
        format!(
            "{{\"schema\":\"rudroid.plugin-diagnostic.v1\",\"severity\":\"fatal\",\"code\":\"{}\",\"plugin\":\"{}\",{}\"message\":\"{}\",\"hint\":\"{}\",\"installed_plugins\":[{}]}}",
            json_escape(self.code),
            json_escape(&self.plugin),
            required,
            json_escape(&self.message),
            json_escape(&self.hint),
            installed.join(",")
        )
    }

    fn installed(capabilities: &[DescriptorInfo]) -> Vec<String> {
        let mut ids: Vec<String> = capabilities.iter().map(|c| c.id.clone()).collect();
        ids.sort();
        ids
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Reference load order: android_base first (JNI plugins share its surface),
/// then a topological order of declared dependencies with role-then-id as
/// the deterministic tie-breaker. Returns Err on missing/self/cyclic deps.
pub fn validate_and_sort(
    capabilities: &[DescriptorInfo],
    config_package: Option<&str>,
) -> Result<Vec<DescriptorInfo>, Diagnostic> {
    // Duplicate ids.
    let mut seen: HashSet<&str> = HashSet::new();
    for capability in capabilities {
        if !seen.insert(capability.id.as_str()) {
            return Err(Diagnostic {
                code: "duplicate_plugin_id",
                plugin: capability.id.clone(),
                required_plugin: None,
                message: format!("duplicate capability id `{}` in the deploy dir", capability.id),
                hint: format!("remove one of the {} libraries", capability.id),
                installed_plugins: Diagnostic::installed(capabilities),
            });
        }
    }

    // Exactly one android_base (reference bootstrap requirement).
    let android_base_count = capabilities.iter().filter(|c| c.kind == Kind::AndroidBase).count();
    if android_base_count != 1 {
        return Err(Diagnostic {
            code: "invalid_android_base_count",
            plugin: "loader".to_owned(),
            required_plugin: Some("android_base".to_owned()),
            message: format!(
                "bootstrap requires exactly one android_base capability (found {android_base_count})"
            ),
            hint: "install exactly one android_base in the deploy dir".to_owned(),
            installed_plugins: Diagnostic::installed(capabilities),
        });
    }

    // At most one Unity profile — exactly one is the runtime contract; the
    // reference rejects two profiles, zero profiles fails later at run.
    let profile_count = capabilities.iter().filter(|c| c.kind == Kind::UnityProfile).count();
    if profile_count > 1 {
        let ids: Vec<String> = capabilities
            .iter()
            .filter(|c| c.kind == Kind::UnityProfile)
            .map(|c| c.id.clone())
            .collect();
        return Err(Diagnostic {
            code: "invalid_unity_profile_count",
            plugin: "loader".to_owned(),
            required_plugin: None,
            message: format!("at most one Unity profile may be deployed (found {profile_count}: {})", ids.join(", ")),
            hint: "deploy only the profile the Title needs".to_owned(),
            installed_plugins: Diagnostic::installed(capabilities),
        });
    }

    // At most one logging plugin (reference `multiple_logging_plugins`).
    let logging_count = capabilities.iter().filter(|c| c.kind == Kind::Logging).count();
    if logging_count > 1 {
        return Err(Diagnostic {
            code: "multiple_logging_plugins",
            plugin: "loader".to_owned(),
            required_plugin: None,
            message: format!("allows at most one logging plugin (found {logging_count})"),
            hint: "keep at most one logging plugin in the deploy dir".to_owned(),
            installed_plugins: Diagnostic::installed(capabilities),
        });
    }

    // target_package belongs to game patches only (reference
    // `invalid_target_package`): any other kind declaring one is a malformed
    // descriptor, not a gate mismatch.
    for capability in capabilities {
        if capability.target_package.is_some() && capability.kind != Kind::GamePatch {
            return Err(Diagnostic {
                code: "invalid_target_package",
                plugin: capability.id.clone(),
                required_plugin: None,
                message: format!(
                    "only game patches may declare target package: {}",
                    capability.id
                ),
                hint: "remove target_package from the descriptor".to_owned(),
                installed_plugins: Diagnostic::installed(capabilities),
            });
        }
    }

    // Gamepatch package gates.
    for capability in capabilities {
        if !package_gate(capability.target_package.as_deref(), config_package) {
            return Err(Diagnostic {
                code: "target_package_mismatch",
                plugin: capability.id.clone(),
                required_plugin: None,
                message: format!(
                    "capability `{}` targets package `{}` which does not match the configured package",
                    capability.id,
                    capability.target_package.clone().unwrap_or_default()
                ),
                hint: "remove the gamepatch or deploy the package it targets".to_owned(),
                installed_plugins: Diagnostic::installed(capabilities),
            });
        }
    }

    // Dependency topology.
    let by_id: HashMap<&str, &DescriptorInfo> =
        capabilities.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut required_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for capability in capabilities {
        let mut deps = Vec::new();
        for required in &capability.required_plugins {
            if required.is_empty() {
                return Err(Diagnostic {
                    code: "invalid_dependency",
                    plugin: capability.id.clone(),
                    required_plugin: None,
                    message: format!(
                        "capability `{}` declares an empty dependency",
                        capability.id
                    ),
                    hint: "fix the plugin dependency manifest".to_owned(),
                    installed_plugins: Diagnostic::installed(capabilities),
                });
            }
            if required == &capability.id {
                return Err(Diagnostic {
                    code: "self_dependency",
                    plugin: capability.id.clone(),
                    required_plugin: Some(required.clone()),
                    message: format!("capability `{}` requires itself", capability.id),
                    hint: "fix the descriptor's required list".to_owned(),
                    installed_plugins: Diagnostic::installed(capabilities),
                });
            }
            if deps.contains(&required.as_str()) {
                return Err(Diagnostic {
                    code: "duplicate_dependency",
                    plugin: capability.id.clone(),
                    required_plugin: Some(required.clone()),
                    message: format!(
                        "capability `{}` repeats dependency `{}`",
                        capability.id, required
                    ),
                    hint: "list each required plugin ID once".to_owned(),
                    installed_plugins: Diagnostic::installed(capabilities),
                });
            }
            deps.push(required.as_str());
            match by_id.get(required.as_str()) {
                Some(_provider) => {}
                None => {
                    return Err(Diagnostic {
                        code: "missing_dependency",
                        plugin: capability.id.clone(),
                        required_plugin: Some(required.clone()),
                        message: format!(
                            "capability `{}` requires missing capability `{}`",
                            capability.id, required
                        ),
                        hint: format!("install {required} in the deploy dir or remove {}", capability.id),
                        installed_plugins: Diagnostic::installed(capabilities),
                    });
                }
            }
        }
        required_of.insert(capability.id.as_str(), deps);
    }

    // Kahn's algorithm; independent peers order by (kind, id).
    let mut remaining_deps: HashMap<&str, usize> = required_of
        .iter()
        .map(|(id, deps)| (*id, deps.len()))
        .collect();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, deps) in &required_of {
        for dep in deps {
            dependents.entry(dep).or_default().push(id);
        }
    }
    let mut ready: BinaryHeap<_> = capabilities
        .iter()
        .filter(|c| remaining_deps[c.id.as_str()] == 0)
        .map(|c| Reverse((c.kind != Kind::AndroidBase, c.kind, c.id.as_str())))
        .collect();
    let mut ordered = Vec::with_capacity(capabilities.len());
    while let Some(Reverse((_, _, id))) = ready.pop() {
        // android_base always maps first (reference bootstrap contract);
        // independent peers otherwise order by (kind, id).
        let picked = by_id[id];
        ordered.push(picked.clone());
        for dependent in dependents.get(picked.id.as_str()).into_iter().flatten() {
            let count = remaining_deps.get_mut(dependent).unwrap();
            *count -= 1;
            if *count == 0 {
                let c = by_id[dependent];
                ready.push(Reverse((c.kind != Kind::AndroidBase, c.kind, c.id.as_str())));
            }
        }
    }
    if ordered.len() != capabilities.len() {
        let stuck: Vec<String> = capabilities
            .iter()
            .map(|c| c.id.clone())
            .filter(|id| !ordered.iter().any(|c| &c.id == id))
            .collect();
        return Err(Diagnostic {
            code: "dependency_cycle",
            plugin: stuck.join(","),
            required_plugin: None,
            message: format!("dependency cycle among: {}", stuck.join(", ")),
            hint: "break the required_plugins cycle in the descriptors".to_owned(),
            installed_plugins: Diagnostic::installed(capabilities),
        });
    }

    Ok(ordered)
}

#[cfg(test)]
#[path = "../tests/unit/validate.rs"]
mod tests;
