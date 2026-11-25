//! INPUT: reserved guest paths, a pinned ELF API and a synchronous post-Main caller
//! OUTPUT: one load and optional JNI initialization per canonical explicit path
//! POS: owns explicit load reservations across reentrant native callbacks
use super::{call_jni_on_load_exec, fetch_last_error, valid_preload_path, ElfLoadApi};
use std::{collections::HashMap, ffi::{c_int, c_void}, path::{Path, PathBuf}};
#[cfg(test)]
use std::ffi::c_char;

/// Per-call context for the generic post-Main-JNI load operation. It exists
/// only while `bring_up` invokes registered hooks and is never retained.
pub(super) struct LoadBridge<'a> {
    pub(super) api: &'a ElfLoadApi,
    pub(super) game_files: &'a Path,
    pub(super) images: std::sync::Mutex<HashMap<PathBuf, ExplicitImageState>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExplicitImageState {
    ReservedByBringUp,
    InProgress,
    Ready { base: usize, jni_on_load: Option<i32> },
    Failed,
}

fn accepted_jni_on_load_version(version: i32) -> bool {
    matches!(
        version,
        0x0001_0001 | 0x0001_0002 | 0x0001_0004 | 0x0001_0006 | 0x0001_0008
    )
}

fn jni_exception_pending() -> bool {
    let raw = crate::runtime::shared_vm();
    // SAFETY: the loader's shared VM is process-pinned; this wrapper is used
    // only for the current JNI_OnLoad thread's ExceptionCheck.
    let Some(vm) = (unsafe { rudroid_common::jni_env::JavaVm::from_raw(raw) }) else {
        return true;
    };
    let Some(env) = vm.get_env(0x0001_0006) else {
        return true;
    };
    if env.exception_check() {
        env.exception_describe();
        env.exception_clear();
        true
    } else {
        false
    }
}

/// Generic loader-owned implementation of an explicit Java
/// `System.loadLibrary` sequence. The path stays below the canonical
/// `game_files` root; the resulting image is registered for `so_symbol` and
/// its `JNI_OnLoad` is called with the shared VM. Handles remain pinned for
/// process life. Non-zero codes are intentionally generic — the calling
/// adapter logs its own stage-specific failure.
pub(super) unsafe extern "C" fn load_bridge_jni_on_load(
    user: *mut c_void,
    relative: rudroid_common::registry::HostStr<'_>,
    out_base: *mut usize,
) -> c_int {
    if user.is_null() {
        return 1;
    }
    // SAFETY: the loader passes a live `LoadBridge` for the duration of the
    // synchronous hook call and never exposes it beyond that call.
    let bridge = unsafe { &*(user as *const LoadBridge<'_>) };
    let Some(relative) = relative.to_cstr().and_then(|value| value.to_str().ok()) else {
        return 1;
    };
    if !valid_preload_path(relative) {
        return 1;
    }
    let candidate = bridge.game_files.join(relative);
    let Ok(path) = std::fs::canonicalize(&candidate) else {
        return 2;
    };
    if !path.starts_with(bridge.game_files) || !path.is_file() {
        return 2;
    }
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return 2;
    };
    let mut images = bridge.images.lock().unwrap();
    match images.get(&path) {
        Some(ExplicitImageState::ReservedByBringUp) => {
            rudroid_common::diagnostic!(
                "[GUEST] explicit load {relative} refused: the path is already owned by a configured preload or the normal bring-up plan; refusing to remap or replay JNI_OnLoad"
            );
            return 6;
        }
        Some(ExplicitImageState::Ready { base, .. }) => {
            if !out_base.is_null() {
                // SAFETY: the callback contract makes out_base writable for
                // one usize when non-null.
                unsafe { *out_base = *base };
            }
            return 0;
        }
        Some(ExplicitImageState::InProgress | ExplicitImageState::Failed) => return 6,
        None => {}
    }
    let (Some(exec_load), Some(exec_symbol), Some(exec_load_base)) = (
        bridge.api.exec_load,
        bridge.api.exec_symbol,
        bridge.api.exec_load_base,
    ) else {
        return 3;
    };
    images.insert(path.clone(), ExplicitImageState::InProgress);
    drop(images);
    // SAFETY: pinned capability entry; c_path is NUL-free and the bridge
    // belongs to the active bring-up.
    let handle = unsafe { exec_load(c_path.as_ptr()) };
    if handle.is_null() {
        let reason = unsafe { fetch_last_error(bridge.api) }.unwrap_or_else(|| {
            "android_elf_load exec backend reported no diagnostic".to_owned()
        });
        rudroid_common::diagnostic!("[GUEST] explicit load {relative} failed: {reason}");
        bridge.images.lock().unwrap().insert(path, ExplicitImageState::Failed);
        return 4;
    }
    // SAFETY: handle is the live exec mapping just returned by the pinned
    // capability; the base query belongs to the same mapping.
    let base = unsafe { exec_load_base(handle) };
    crate::runtime::register_guest_exec_module(base, handle);
    // SAFETY: the same exec mapping and the registered shared VM satisfy the
    // documented call contract.
    let jni_on_load = match unsafe { call_jni_on_load_exec(exec_symbol, handle, relative) } {
        Ok(Some(version)) if accepted_jni_on_load_version(version) => Some(version),
        Ok(Some(version)) => {
            bridge.images.lock().unwrap().insert(path, ExplicitImageState::Failed);
            crate::runtime::unregister_guest_exec_module(base);
            rudroid_common::diagnostic!(
                "[GUEST] explicit load {relative} rejected JNI_OnLoad version {version:#x}"
            );
            return 5;
        }
        Ok(None) => None,
        Err(error) => {
            bridge.images.lock().unwrap().insert(path, ExplicitImageState::Failed);
            crate::runtime::unregister_guest_exec_module(base);
            rudroid_common::diagnostic!("[GUEST] explicit load {relative} failed: {error}");
            return 5;
        }
    };
    if jni_on_load.is_some() && jni_exception_pending() {
        bridge.images.lock().unwrap().insert(path, ExplicitImageState::Failed);
        crate::runtime::unregister_guest_exec_module(base);
        rudroid_common::diagnostic!(
            "[GUEST] explicit load {relative} left a pending JNI exception after JNI_OnLoad"
        );
        return 5;
    }
    bridge.images.lock().unwrap().insert(
        path,
        ExplicitImageState::Ready {
            base,
            jni_on_load,
        },
    );
    if let Some(version) = jni_on_load {
        rudroid_common::diagnostic!(
            "[GUEST] explicit load {relative} JNI_OnLoad accepted version {version:#x}"
        );
    } else {
        rudroid_common::diagnostic!(
            "[GUEST] explicit load {relative} has no JNI_OnLoad (standard optional entry)"
        );
    }
    if !out_base.is_null() {
        // SAFETY: the callback contract makes out_base writable for one usize
        // when non-null.
        unsafe { *out_base = base };
    }
    0
}

#[cfg(test)]
#[path = "../tests/unit/guest_load_bridge.rs"]
mod load_bridge_tests;
