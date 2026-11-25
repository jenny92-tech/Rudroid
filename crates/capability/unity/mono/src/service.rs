//! INPUT:  Host API config getters and `rudroid.elf.guest_libdl.v1`
//! OUTPUT:  `MonoServiceV1` callbacks for validate / reserved paths / start
//! POS:    Unity Mono plan/prepare hooks without host-core coupling
#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_int, CString};

use rudroid_common::host_api::HostApiV0;
use rudroid_common::native_alias::GuestLibdl;
use rudroid_common::registry::{HostStr, RudroidGuestPlanBuilder};
use rudroid_unity_common::mono_abi::{MonoServiceV1, MonoStartup, MONO_SERVICE_ABI};
use rudroid_unity_common::{mono_config_from_api, mono_section_present_in_api, MonoConfig};

use crate::start;

pub(crate) static MONO_SERVICE: MonoServiceV1 = MonoServiceV1 {
    abi_version: MONO_SERVICE_ABI,
    validate_config: Some(validate_config),
    add_reserved_paths: Some(add_reserved_paths),
    start_bridge: Some(start_bridge),
};

extern "C" fn validate_config(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if !mono_section_present_in_api(api) {
        return 0;
    }
    for env_name in [c"UA_MONO64_RUNTIME", c"UA_MONO64_MANAGED"] {
        if api.env_cstr(env_name).is_some() {
            api.log_line(
                c"MONO",
                &format!(
                    "{} overrides the configured Mono artifact paths",
                    env_name.to_string_lossy()
                ),
            );
            return -1;
        }
    }
    match mono_config_from_api(api) {
        Ok(Some(_)) | Ok(None) => 0,
        Err(reason) => {
            api.log_line(c"MONO", &reason);
            -1
        }
    }
}

extern "C" fn add_reserved_paths(
    api: Option<&'static HostApiV0>,
    builder: Option<&RudroidGuestPlanBuilder>,
) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    let Some(builder) = builder else {
        return -1;
    };
    let config = match mono_config_from_api(api) {
        Ok(Some(config)) => config,
        Ok(None) => return 0,
        Err(reason) => {
            api.log_line(c"MONO", &reason);
            return -1;
        }
    };
    let Some(add) = builder.add_reserved_path else {
        return -1;
    };
    for path in [config.bridge.as_path(), config.runtime.as_path()] {
        let Ok(c_path) = CString::new(path.as_os_str().as_encoded_bytes()) else {
            return -1;
        };
        if add(builder.ctx, HostStr::new(&c_path)) != 0 {
            return -1;
        }
    }
    if let Some(dir) = config.bridge.parent() {
        for helper in ["libMonoPosixHelper.so", "libmono-native.so"] {
            if let Ok(helper_path) = dir.join(helper).canonicalize() {
                let Ok(c_helper) = CString::new(helper_path.as_os_str().as_encoded_bytes()) else {
                    return -1;
                };
                if add(builder.ctx, HostStr::new(&c_helper)) != 0 {
                    return -1;
                }
            }
        }
    }
    0
}

extern "C" fn start_bridge(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    let config = match mono_config_from_api(api) {
        Ok(Some(config)) => config,
        Ok(None) => return 0,
        Err(reason) => {
            api.log_error(c"MONO", &reason);
            return -1;
        }
    };
    let Some(guest) = api.query_service_typed::<GuestLibdl>() else {
        api.log_error(c"MONO", "rudroid.elf.guest_libdl.v1 is not registered");
        return -1;
    };
    match start_from_config(&config, guest, Some(api)) {
        Ok(()) => 0,
        Err(reason) => {
            api.log_error(c"MONO", &reason);
            -1
        }
    }
}

fn start_from_config(
    config: &MonoConfig,
    guest: &'static GuestLibdl,
    api: Option<&'static HostApiV0>,
) -> Result<(), String> {
    let cstring = |path: &std::path::Path| -> Result<CString, String> {
        CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| format!("{} is not a valid C string", path.display()))
    };
    let bridge = cstring(&config.bridge)?;
    let runtime = cstring(&config.runtime)?;
    let managed_dir = cstring(&config.managed_dir)?;
    let guest_lib_dir = cstring(&config.guest_lib_dir)?;
    let unity_version = CString::new(config.unity_version.as_str())
        .map_err(|_| "unity_version is not a valid C string".to_owned())?;
    let plugins_map_owned = match &config.plugins_map {
        Some(path) => Some(cstring(path)?),
        None => None,
    };
    let plugins_map = match plugins_map_owned.as_deref() {
        Some(path) => HostStr::new(path),
        None => HostStr::null(),
    };
    let startup = MonoStartup {
        bridge: HostStr::new(&bridge),
        runtime: HostStr::new(&runtime),
        managed_dir: HostStr::new(&managed_dir),
        guest_lib_dir: HostStr::new(&guest_lib_dir),
        unity_version: HostStr::new(&unity_version),
        plugins_map,
        frame_pacing: config.frame_pacing,
        display_refresh_hz: config.display_refresh_hz,
    };
    start(&startup, guest, api)
}
