//! INPUT:  HostApi, Host Config views and shared IL2CPP/Mono ABI evidence
//! OUTPUT:  plain Unity helper modules for profiles, hooks, gamepatches and the loader
//! POS:    shares Unity/IL2CPP/Mono helpers without a descriptor or cross-cdylib statics

pub mod il2cpp;
pub mod il2cpp_abi;
pub mod managed_hooks;
pub mod unity_objects;
pub mod hud_layout;
mod touch_controls;
pub mod gamepatch_support;
pub mod mono_abi;
pub mod mono;
pub mod unity_startup;
pub mod boot_view;
pub mod module_roles;
pub mod guest_entries;
pub mod player_jni;

pub use guest_entries::{unity_arm64_guest_entries, UnityGuestEntrySpec};

pub use boot_view::BootView;
pub use mono::{mono_config, mono_config_from_api, mono_section_present_in_api, MonoConfig};
pub use mono_abi::{MonoServiceV1, MONO_SERVICE_ABI, MONO_SERVICE_NAME};
pub use player_jni::{
    dispatch_player_input_event, finish_player, PlayerInput, PlayerInputDispatch,
};
pub use unity_startup::{
    unity_startup_policy, unity_startup_policy_from_api, UnityRenderThread, UnityStartupPolicy,
    MAX_BOOT_CONFIG_BYTES,
};

/// Spec/initJni shape flags (player_lifecycle `Spec`, not the C profile POD).
pub const RUDROID_UNITY_PROFILE_NONE: u32 = 0;
pub const RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG: u32 = 1 << 0;

#[cfg(test)]
#[path = "../tests/unit/profile_flags.rs"]
mod profile_flags_tests;

#[cfg(test)]
#[path = "../tests/unit/player_jni.rs"]
mod player_jni_tests;
