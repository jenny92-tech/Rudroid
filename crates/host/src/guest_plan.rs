//! INPUT: host configuration, package paths and prior guest module events
//! OUTPUT: ordered guest plan and canonical preload validation
//! POS: data-only planning before any native entry point runs; engine entries
//! come from the registered EngineProfile build_guest_plan callback
#![deny(unsafe_op_in_unsafe_fn)]
use std::ffi::{c_int, c_void};
use std::path::{Component, Path, PathBuf};
use rudroid_common::ModuleKind;
use rudroid_common::registry::{
    HostStr, RudroidGuestPlanBuilder, RUDROID_GUEST_ENTRY_AFTER_FMOD,
    RUDROID_GUEST_ENTRY_FMOD_JNI_OK,
};
use super::{GuestError, ModuleEvent};

/// One guest module the bring-up maps, in reference order.
#[derive(Debug, Clone)]
pub struct GuestEntry {
    pub kind: Option<ModuleKind>,
    /// Path relative to the game_files dir (process-pinned via builder leak).
    pub relative_path: &'static str,
    pub required: bool,
    /// Ticket 56: reference plugin gating. In the C++ stack the addon
    /// plugins load these libraries themselves inside their Unity
    /// module-loaded callback; the kit realizes the same load+notify outcome
    /// through this plan, so the gate keeps the reference order observable.
    /// An entry with unsatisfied gates is never attempted (logged).
    pub gates: Vec<EntryGate>,
}

/// Reference-order gate for a plan entry (ticket 56).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryGate {
    /// Only after this kind's loaded event fired (the reference
    /// `AfterUnityGuestModule` acts solely on the Unity callback).
    KindLoaded(ModuleKind),
    /// Only when the fmod module loaded AND its `JNI_OnLoad` outcome is
    /// acceptable — absent export or a reference-supported version
    /// (sdk_fmod `supported_jni_version`).
    FmodJniOk,
}

impl EntryGate {
    pub(super) fn name(self) -> &'static str {
        match self {
            EntryGate::KindLoaded(kind) => match kind {
                ModuleKind::Unity => "unity loaded",
                ModuleKind::Fmod => "fmod loaded",
                other => other.name(),
            },
            EntryGate::FmodJniOk => "fmod JNI_OnLoad version usable",
        }
    }
}

/// `true` when every gate of the entry is satisfied by the events fired so
/// far (the plan processes entries in reference order, so earlier events are
/// already recorded).
pub(super) fn gates_satisfied(gates: &[EntryGate], events: &[ModuleEvent]) -> bool {
    gates.iter().all(|gate| match gate {
        EntryGate::KindLoaded(kind) => events.iter().any(|event| event.kind == *kind),
        EntryGate::FmodJniOk => events
            .iter()
            .any(|event| event.kind == ModuleKind::Fmod && fmod_jni_usable(event.jni_on_load)),
    })
}

/// The oracle `supported_jni_version` set (sdk_fmod.cpp): 1.1/1.2/1.4/1.6/1.8.
/// A module without the export is usable (the oracle leaves `g_fmod_jni_ok`
/// true when the address is missing).
fn fmod_jni_usable(jni_on_load: Option<i32>) -> bool {
    match jni_on_load {
        None => true,
        Some(version) => matches!(
            version,
            0x0001_0001 | 0x0001_0002 | 0x0001_0004 | 0x0001_0006 | 0x0001_0008
        ),
    }
}

/// The guest load plan derived from the Host Config plus the registered
/// engine profile's `build_guest_plan` callback.
#[derive(Debug, Clone)]
pub struct GuestPlan {
    /// `[paths].game_files` — the reference chdirs here before loading.
    pub game_files: PathBuf,
    /// Explicit dependency support libraries, in mapping order. They are
    /// mapped before `entries`, fire no module event and never run JNI_OnLoad.
    pub preload: Vec<String>,
    pub entries: Vec<GuestEntry>,
    /// Extra paths the mapper must reserve (engine/mono may append these).
    pub reserved_paths: Vec<PathBuf>,
    /// ModuleKinds whose JNI_OnLoad is deferred to NativeLoader.load.
    pub defer_jni_on_load: Vec<ModuleKind>,
}

pub(super) fn valid_preload_path(path: &str) -> bool {
    if path.is_empty() || !Path::new(path).is_relative() {
        return false;
    }
    // `Path::components` normalizes an interior `.` on some platforms, so
    // also inspect the configured spelling itself.
    path.split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Resolve one explicit preload below the already-canonical `game_files`
/// root. The loader receives no guest-supplied path, but configuration can
/// still contain a symlink; never let that symlink or a dot component escape
/// the game directory. Only regular files reach the executable mapper.
pub(super) fn configured_preload_path(root: &Path, relative: &str) -> Result<PathBuf, GuestError> {
    let configured = PathBuf::from(relative);
    if !valid_preload_path(relative) {
        return Err(GuestError::LoadFailed {
            path: configured,
            required: true,
            reason: "configured guest preload must be a non-empty relative path \
                     below game_files without `.` or `..` components"
                .to_owned(),
        });
    }
    let candidate = root.join(relative);
    let resolved = std::fs::canonicalize(&candidate).map_err(|error| GuestError::LoadFailed {
        path: candidate,
        required: true,
        reason: format!("configured guest preload cannot be resolved: {error}"),
    })?;
    if !resolved.starts_with(root) {
        return Err(GuestError::LoadFailed {
            path: resolved,
            required: true,
            reason: "configured guest preload resolves outside game_files".to_owned(),
        });
    }
    let metadata = std::fs::metadata(&resolved).map_err(|error| GuestError::LoadFailed {
        path: resolved.clone(),
        required: true,
        reason: format!("configured guest preload metadata is unavailable: {error}"),
    })?;
    if !metadata.is_file() {
        return Err(GuestError::LoadFailed {
            path: resolved,
            required: true,
            reason: "configured guest preload is not a regular file".to_owned(),
        });
    }
    Ok(resolved)
}

struct PlanBuildCtx {
    entries: Vec<GuestEntry>,
    reserved_paths: Vec<PathBuf>,
    defer_jni_on_load: Vec<ModuleKind>,
    error: Option<String>,
}

extern "C" fn builder_add_entry(
    ctx: *mut c_void,
    relative_path: HostStr<'_>,
    role_id: u32,
    required: c_int,
    after_role_id: u32,
    flags: u32,
) -> c_int {
    // SAFETY: loader-owned PlanBuildCtx for the duration of build_guest_plan.
    let Some(ctx) = (unsafe { (ctx as *mut PlanBuildCtx).as_mut() }) else {
        return -1;
    };
    let Some(path) = relative_path.to_cstr().map(|p| p.to_string_lossy().into_owned()) else {
        ctx.error = Some("guest plan entry path is null".into());
        return -1;
    };
    if path.is_empty() {
        ctx.error = Some("guest plan entry path is empty".into());
        return -1;
    }
    if !valid_preload_path(&path) {
        ctx.error = Some(format!(
            "guest plan entry path must be a non-empty relative path without `.` or `..`: {path}"
        ));
        return -1;
    }
    let path: &'static str = Box::leak(path.into_boxed_str());
    let kind = if role_id == 0 {
        None
    } else {
        match ModuleKind::from_u32(role_id) {
            Some(kind) => Some(kind),
            None => {
                ctx.error = Some(format!("unknown guest plan role_id {role_id}"));
                return -1;
            }
        }
    };
    let mut gates = Vec::new();
    if after_role_id != 0 {
        match ModuleKind::from_u32(after_role_id) {
            Some(after) => gates.push(EntryGate::KindLoaded(after)),
            None => {
                ctx.error = Some(format!("unknown guest plan after_role_id {after_role_id}"));
                return -1;
            }
        }
    }
    if flags & RUDROID_GUEST_ENTRY_AFTER_FMOD != 0 {
        gates.push(EntryGate::KindLoaded(ModuleKind::Fmod));
    }
    if flags & RUDROID_GUEST_ENTRY_FMOD_JNI_OK != 0 {
        gates.push(EntryGate::FmodJniOk);
    }
    ctx.entries.push(GuestEntry {
        kind,
        relative_path: path,
        required: required != 0,
        gates,
    });
    0
}

extern "C" fn builder_add_reserved_path(ctx: *mut c_void, canonical_path: HostStr<'_>) -> c_int {
    // SAFETY: loader-owned PlanBuildCtx for the duration of build_guest_plan.
    let Some(ctx) = (unsafe { (ctx as *mut PlanBuildCtx).as_mut() }) else {
        return -1;
    };
    let Some(path) = canonical_path.to_cstr().map(|p| PathBuf::from(p.to_string_lossy().as_ref())) else {
        ctx.error = Some("guest plan reserved path is null".into());
        return -1;
    };
    if path.as_os_str().is_empty() {
        ctx.error = Some("guest plan reserved path is empty".into());
        return -1;
    }
    ctx.reserved_paths.push(path);
    0
}

extern "C" fn builder_set_defer_jni_on_load(ctx: *mut c_void, role_id: u32) -> c_int {
    // SAFETY: loader-owned PlanBuildCtx for the duration of build_guest_plan.
    let Some(ctx) = (unsafe { (ctx as *mut PlanBuildCtx).as_mut() }) else {
        return -1;
    };
    let Some(kind) = ModuleKind::from_u32(role_id) else {
        ctx.error = Some(format!("unknown defer_jni_on_load role_id {role_id}"));
        return -1;
    };
    if !ctx.defer_jni_on_load.contains(&kind) {
        ctx.defer_jni_on_load.push(kind);
    }
    0
}

impl GuestPlan {
    /// Resolve game_files/preload from config, then ask the registered
    /// EngineProfile to append engine-specific entries. Call after the
    /// register-JNI phase so the profile is present; without a profile the
    /// plan keeps empty entries (preload-only / synthetic fixtures).
    pub fn from_config(config: &rudroid_host_config::HostConfig) -> Result<GuestPlan, GuestError> {
        let paths = config.paths();
        let Some(game_files) = paths.game_files else {
            return Err(GuestError::MissingConfigKey("paths.game_files".to_owned()));
        };
        for relative in &paths.guest_preload {
            if !valid_preload_path(relative) {
                return Err(GuestError::LoadFailed {
                    path: PathBuf::from(relative),
                    required: true,
                    reason: "configured guest preload must be a non-empty relative path \
                             below game_files without `.` or `..` components"
                        .to_owned(),
                });
            }
        }
        let mut plan = GuestPlan {
            game_files: PathBuf::from(game_files),
            preload: paths.guest_preload,
            entries: Vec::new(),
            reserved_paths: Vec::new(),
            defer_jni_on_load: Vec::new(),
        };
        if let Some(profile) = crate::runtime::registered_profile() {
            if let Some(build) = profile.build_guest_plan {
                let mut ctx = PlanBuildCtx {
                    entries: Vec::new(),
                    reserved_paths: Vec::new(),
                    defer_jni_on_load: Vec::new(),
                    error: None,
                };
                let builder = RudroidGuestPlanBuilder {
                    ctx: &mut ctx as *mut PlanBuildCtx as *mut c_void,
                    add_entry: Some(builder_add_entry),
                    add_reserved_path: Some(builder_add_reserved_path),
                    set_defer_jni_on_load: Some(builder_set_defer_jni_on_load),
                };
                let rc = build(Some(crate::host_api::table()), Some(&builder));
                if rc != 0 {
                    return Err(GuestError::LoadFailed {
                        path: plan.game_files.clone(),
                        required: true,
                        reason: ctx
                            .error
                            .unwrap_or_else(|| format!("engine build_guest_plan returned {rc}")),
                    });
                }
                if let Some(error) = ctx.error {
                    return Err(GuestError::LoadFailed {
                        path: plan.game_files.clone(),
                        required: true,
                        reason: error,
                    });
                }
                plan.entries = ctx.entries;
                plan.reserved_paths = ctx.reserved_paths;
                plan.defer_jni_on_load = ctx.defer_jni_on_load;
            }
        }
        Ok(plan)
    }
}

#[cfg(test)]
#[path = "../tests/unit/guest_plan.rs"]
mod tests;
