//! INPUT:  Host API config getters and the loader guest-plan builder
//! OUTPUT:  Unity ARM64 guest .so order, mono reserved paths and validate_config
//! POS:    owns Unity-specific plan entries outside the engine-agnostic loader
use std::ffi::{c_int, CStr, CString};
use std::path::PathBuf;

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::{HostStr, RudroidGuestPlanBuilder};
use rudroid_common::ModuleKind;
use rudroid_unity_common::unity_arm64_guest_entries;

use crate::mono_service;

/// Reject misconfigured Unity startup / Mono env before guest bring-up.
pub(crate) extern "C" fn validate_config(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if let Err(reason) = rudroid_unity_common::unity_startup_policy_from_api(api) {
        api.log_line(c"UNITY", &format!("invalid unity startup policy: {reason}"));
        return -1;
    }
    let mono_configured = rudroid_unity_common::mono_section_present_in_api(api);
    if let Some(svc) = mono_service::query(api) {
        if let Some(validate) = svc.validate_config {
            return validate(Some(api));
        }
    } else if mono_configured {
        api.log_line(
            c"UNITY",
            "unity.mono requires the optional unity_mono runtime capability",
        );
        return -1;
    }
    0
}

fn add(
    builder: &RudroidGuestPlanBuilder,
    path: &CStr,
    role: u32,
    required: bool,
    after: u32,
    flags: u32,
) -> c_int {
    let Some(add_entry) = builder.add_entry else {
        return -1;
    };
    add_entry(
        builder.ctx,
        HostStr::new(path),
        role,
        if required { 1 } else { 0 },
        after,
        flags,
    )
}

/// Append the Unity ARM64 guest library order. Mono config strips Il2cpp and
/// makes libunity required when `unity.mono.*` keys are present.
pub(crate) extern "C" fn build_guest_plan(
    api: Option<&'static HostApiV0>,
    builder: Option<&RudroidGuestPlanBuilder>,
) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    let Some(builder) = builder else {
        return -1;
    };
    // Only an actual `[unity.mono]` config strips Il2cpp. Deploying
    // libunity_mono.so alone must not flip IL2CPP titles onto the Mono plan.
    let mono = rudroid_unity_common::mono_section_present_in_api(api);

    for entry in unity_arm64_guest_entries(mono) {
        let path = CString::new(entry.relative_path).expect("guest paths have no NUL");
        if add(
            builder,
            &path,
            entry.role_id,
            entry.required,
            entry.after_role_id,
            entry.flags,
        ) != 0
        {
            return -1;
        }
    }

    // Real Unity NativeLoader.load owns engine JNI_OnLoad when libunity is present.
    let game_files = api
        .config_string(c"paths.game_files", 4096)
        .map(PathBuf::from);
    if game_files
        .as_ref()
        .is_some_and(|root| root.join("lib/arm64-v8a/libunity.so").is_file())
    {
        let Some(set_defer) = builder.set_defer_jni_on_load else {
            return -1;
        };
        for kind in [ModuleKind::Unity, ModuleKind::Il2cpp] {
            if set_defer(builder.ctx, kind as u32) != 0 {
                return -1;
            }
        }
    }

    if mono {
        if let Some(svc) = mono_service::query(api) {
            if let Some(add_reserved) = svc.add_reserved_paths {
                if add_reserved(Some(api), Some(builder)) != 0 {
                    return -1;
                }
            }
        }
    }
    0
}
