//! INPUT:  shared Unity player lifecycle and the observed Unity 2022.3 Java surface
//! OUTPUT:  unity_2022_3 descriptor, engine Spec and early JNI registration
//! POS:    Context-only Unity 2022 profile; no native-phase patch
//! rudroid-unity_2022_3: Unity 2022.3 engine profile capability.
//!
//! The prepare/bootstrap/run player lifecycle lives in
//! `rudroid_unity_player_lifecycle`; this crate owns only the engine Spec and
//! the early registration entry point. Its JNI/Java shape is a candidate
//! derived from Soul Knight 8.7.0 (Unity 2022.3.62f2) and requires an actual
//! launch before service compatibility is accepted.

#![deny(unsafe_code)]

use std::ffi::c_int;

use rudroid_common::host_api::HostApiV0;
use rudroid_unity_common::RUDROID_UNITY_PROFILE_NONE;
use rudroid_unity_player_lifecycle::{self as lifecycle, Profile, Spec, INIT_JNI_CONTEXT};

rudroid_common::declare_descriptor!(ENGINE_SPEC.id, rudroid_common::Kind::EngineProfile);

/// Engine ABI for the observed Unity 2022.3 player surface.
pub const ENGINE_SPEC: Spec = Spec {
    id: "unity_2022_3",
    log_tag: c"UNITY2022",
    registered_message: "Unity 2022.3 JNI and early engine profile registered",
    player_class: "com/unity3d/player/UnityPlayer",
    player_constructor_signature: "(Landroid/content/Context;)V",
    init_jni_signature: INIT_JNI_CONTEXT,
    flags: RUDROID_UNITY_PROFILE_NONE,
    init_jni_int_arg: 0,
    render_sleep_us: 0,
    extra_player_callbacks: &[],
    publishes_current_activity: true,
    register_java_services: Some(lifecycle::java::register_2021),
};

struct Unity2022;

impl Profile for Unity2022 {
    const SPEC: &'static Spec = &ENGINE_SPEC;
}

#[allow(unsafe_code)] // Standardized plugin export; the body only calls the safe lifecycle helper.
#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    lifecycle::register_jni::<Unity2022>(api)
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
