//! INPUT:  mono-mode flag for Unity ARM64 guest library order
//! OUTPUT:  ordered entry specs (path, role, required, after, flags)
//! POS:    shared Unity guest-plan data for player_lifecycle and synthetic kits
#![forbid(unsafe_code)]

use crate::module_roles;
use rudroid_common::registry::{RUDROID_GUEST_ENTRY_AFTER_FMOD, RUDROID_GUEST_ENTRY_FMOD_JNI_OK};

/// One Unity ARM64 guest library the engine profile appends to the guest plan.
#[derive(Debug, Clone, Copy)]
pub struct UnityGuestEntrySpec {
    pub relative_path: &'static str,
    pub role_id: u32,
    pub required: bool,
    pub after_role_id: u32,
    pub flags: u32,
}

/// Reference-order Unity ARM64 `.so` list. When `mono` is true, Il2cpp is
/// omitted and libunity becomes required.
pub fn unity_arm64_guest_entries(mono: bool) -> Vec<UnityGuestEntrySpec> {
    let unity_required = mono;
    let all = [
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libc++_shared.so",
            role_id: 0,
            required: false,
            after_role_id: 0,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libmain.so",
            role_id: module_roles::MAIN,
            required: true,
            after_role_id: 0,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libil2cpp.so",
            role_id: module_roles::IL2CPP,
            required: true,
            after_role_id: 0,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libunity.so",
            role_id: module_roles::UNITY,
            required: unity_required,
            after_role_id: 0,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/lib_burst_generated.so",
            role_id: module_roles::BURST,
            required: false,
            after_role_id: module_roles::UNITY,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libUnityHelpers_Android.so",
            role_id: module_roles::UNITY_HELPERS,
            required: false,
            after_role_id: module_roles::UNITY,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libxlua.so",
            role_id: module_roles::XLUA,
            required: false,
            after_role_id: module_roles::UNITY,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libfmod.so",
            role_id: module_roles::FMOD,
            required: false,
            after_role_id: module_roles::UNITY,
            flags: 0,
        },
        UnityGuestEntrySpec {
            relative_path: "lib/arm64-v8a/libfmodstudio.so",
            role_id: module_roles::FMOD_STUDIO,
            required: false,
            after_role_id: module_roles::UNITY,
            flags: RUDROID_GUEST_ENTRY_AFTER_FMOD | RUDROID_GUEST_ENTRY_FMOD_JNI_OK,
        },
    ];
    all.into_iter()
        .filter(|entry| !(mono && entry.role_id == module_roles::IL2CPP))
        .collect()
}
