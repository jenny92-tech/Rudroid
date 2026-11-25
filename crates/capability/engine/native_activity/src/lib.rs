//! INPUT:  Host API table, `[native_activity]` config and guest Main mapping
//! OUTPUT:  `engine_native_activity` EngineProfile (validate/plan/prepare/bootstrap/run)
//! POS:    proves a second engine can reuse Android/host without Unity
#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_int, c_void, CStr, CString};
use std::sync::Mutex;

use rudroid_common::descriptor;
use rudroid_common::handles::HandleTable;
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::registry::{
    HostStr, RudroidEngineProfile, RudroidGuestPlanBuilder, RudroidModuleEvent,
};
use rudroid_common::ModuleKind;

rudroid_common::declare_descriptor!(
    "engine_native_activity",
    rudroid_common::Kind::EngineProfile,
    package = None::<&str>,
    requires = Some("android_base,android_elf_load")
);

const DEFAULT_ENTRYPOINT: &CStr = c"ANativeActivity_onCreate";
const LOG_TAG: &CStr = c"NATIVE_ACTIVITY";

static SESSIONS: HandleTable<Session> = HandleTable::new();

struct Session {
    api: &'static HostApiV0,
    library: String,
    entrypoint: CString,
    main_base: Mutex<Option<usize>>,
    internal_data_path: CString,
    external_data_path: CString,
}

/// Minimal NDK `ANativeActivityCallbacks` layout (16 function pointers).
#[repr(C)]
struct ANativeActivityCallbacks {
    on_create_reserved: [*mut c_void; 5],
    on_destroy: Option<unsafe extern "C" fn(*mut ANativeActivity)>,
    rest: [*mut c_void; 10],
}

/// Minimal NDK `ANativeActivity` POD. Window/asset manager left null in this proof.
#[repr(C)]
struct ANativeActivity {
    callbacks: *mut ANativeActivityCallbacks,
    vm: *mut c_void,
    env: *mut c_void,
    clazz: *mut c_void,
    internal_data_path: *const std::os::raw::c_char,
    external_data_path: *const std::os::raw::c_char,
    sdk_version: i32,
    instance: *mut c_void,
    asset_manager: *mut c_void,
    obb_path: *const std::os::raw::c_char,
}

struct SessionGuard(*mut c_void);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = SESSIONS.remove(self.0);
    }
}

fn valid_guest_relative_path(path: &str) -> bool {
    if path.is_empty() || std::path::Path::new(path).is_absolute() {
        return false;
    }
    path.split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn read_library(api: &HostApiV0) -> Result<String, &'static str> {
    let library = api
        .config_string(c"native_activity.library", 512)
        .filter(|value| !value.is_empty())
        .ok_or("native_activity.library is required")?;
    if !valid_guest_relative_path(&library) {
        return Err(
            "native_activity.library must be a non-empty relative path without `.` or `..`",
        );
    }
    Ok(library)
}

fn read_entrypoint(api: &HostApiV0) -> CString {
    let text = api
        .config_string_with_fallback(c"native_activity.entrypoint", DEFAULT_ENTRYPOINT, 256)
        .map(|(_, value)| value)
        .unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_string_lossy().into_owned());
    CString::new(text.replace('\0', "")).unwrap_or_else(|_| {
        CString::new(DEFAULT_ENTRYPOINT.to_bytes()).expect("default entrypoint")
    })
}

fn data_path(api: &HostApiV0, key: &CStr, fallback_key: &CStr) -> CString {
    let text = api
        .config_string(key, 4096)
        .filter(|value| !value.is_empty())
        .or_else(|| api.config_string(fallback_key, 4096).filter(|value| !value.is_empty()))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|path| path.into_os_string().into_string().ok())
                .unwrap_or_else(|| ".".into())
        });
    CString::new(text.replace('\0', "")).unwrap_or_else(|_| CString::new(".").expect("literal"))
}

extern "C" fn validate_config(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if let Err(reason) = read_library(api) {
        api.log_line(LOG_TAG, reason);
        return -1;
    }
    let entry = read_entrypoint(api);
    if entry.as_bytes().is_empty() {
        api.log_line(LOG_TAG, "native_activity.entrypoint is empty");
        return -1;
    }
    0
}

extern "C" fn build_guest_plan(
    api: Option<&'static HostApiV0>,
    builder: Option<&RudroidGuestPlanBuilder>,
) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    let Some(builder) = builder else {
        return -1;
    };
    let Some(add_entry) = builder.add_entry else {
        return -1;
    };
    let Ok(library) = read_library(api) else {
        return -1;
    };
    let shared = CString::new("lib/arm64-v8a/libc++_shared.so").expect("literal");
    if add_entry(builder.ctx, HostStr::new(&shared), 0, 0, 0, 0) != 0 {
        return -1;
    }
    let path = match CString::new(library) {
        Ok(path) => path,
        Err(_) => return -1,
    };
    add_entry(
        builder.ctx,
        HostStr::new(&path),
        ModuleKind::Main as u32,
        1,
        0,
        0,
    )
}

extern "C" fn capture_main(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let Some(event) = event else {
        return;
    };
    if event.kind != ModuleKind::Main as u32 {
        return;
    }
    let Some(session) = SESSIONS.get(user) else {
        return;
    };
    *session.main_base.lock().unwrap_or_else(|p| p.into_inner()) = Some(event.base);
}

extern "C" fn prepare(api: Option<&'static HostApiV0>) -> *mut c_void {
    let Some(api) = api else {
        return std::ptr::null_mut();
    };
    if api.abi_version != HOST_API_VERSION {
        api.log_line(LOG_TAG, "host api version mismatch; prepare refused");
        return std::ptr::null_mut();
    }
    let library = match read_library(api) {
        Ok(library) => library,
        Err(reason) => {
            api.log_line(LOG_TAG, reason);
            return std::ptr::null_mut();
        }
    };
    let entrypoint = read_entrypoint(api);
    let session = Session {
        api,
        library,
        entrypoint,
        main_base: Mutex::new(None),
        internal_data_path: data_path(api, c"paths.android_files", c"paths.game_files"),
        external_data_path: data_path(
            api,
            c"paths.android_external_files",
            c"paths.game_files",
        ),
    };
    let handle = SESSIONS.insert(session);
    if !api.on_module_loaded(ModuleKind::Main, capture_main, handle) {
        api.log_line(LOG_TAG, "Main module subscription refused");
        let _ = SESSIONS.remove(handle);
        return std::ptr::null_mut();
    }
    api.log_line(LOG_TAG, "native activity session prepared");
    handle
}

extern "C" fn bootstrap(session: *mut c_void, _native_library_dir: HostStr<'_>) -> c_int {
    if SESSIONS.get(session).is_none() {
        return -1;
    }
    0
}

extern "C" fn run(session_handle: *mut c_void) -> c_int {
    let guard = SessionGuard(session_handle);
    let Some(session) = SESSIONS.get(session_handle) else {
        return -1;
    };
    let api = session.api;
    let Some(base) = *session.main_base.lock().unwrap_or_else(|p| p.into_inner()) else {
        api.log_line(LOG_TAG, "guest Main module was not mapped before run");
        return -1;
    };
    let Some(address) = api.guest_symbol(base, &session.entrypoint) else {
        api.log_line(
            LOG_TAG,
            &format!(
                "entrypoint {} missing from guest Main",
                session.entrypoint.to_string_lossy()
            ),
        );
        return -1;
    };
    let mut callbacks = ANativeActivityCallbacks {
        on_create_reserved: [std::ptr::null_mut(); 5],
        on_destroy: None,
        rest: [std::ptr::null_mut(); 10],
    };
    let vm = api
        .java_vm()
        .map(|vm| vm.as_raw())
        .unwrap_or(std::ptr::null_mut());
    let mut activity = ANativeActivity {
        callbacks: &mut callbacks,
        vm,
        env: std::ptr::null_mut(),
        clazz: std::ptr::null_mut(),
        internal_data_path: session.internal_data_path.as_ptr(),
        external_data_path: session.external_data_path.as_ptr(),
        sdk_version: 28,
        instance: std::ptr::null_mut(),
        asset_manager: std::ptr::null_mut(),
        obb_path: std::ptr::null(),
    };
    // SAFETY: address is a guest export from the mapped Main image with the
    // ANativeActivity_onCreate ABI; activity/callbacks stay live for the call.
    let on_create: unsafe extern "C" fn(*mut ANativeActivity, *mut c_void, usize) =
        unsafe { std::mem::transmute(address) };
    // SAFETY: guest entrypoint; activity POD and callbacks are live for the call.
    unsafe { on_create(&mut activity, std::ptr::null_mut(), 0) };
    // Window/input lifecycle stays stubbed (no ANativeWindow in this proof).
    if let Some(on_destroy) = callbacks.on_destroy {
        // SAFETY: guest wrote an onDestroy function pointer into the NDK slot.
        unsafe { on_destroy(&mut activity) };
    }

    api.log_line(
        LOG_TAG,
        &format!(
            "native activity entrypoint returned (library={}, base={base:#x})",
            session.library
        ),
    );
    drop(guard);
    0
}

#[allow(unsafe_code)] // Standardized plugin export; body only calls safe Host API helpers.
#[no_mangle]
extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let profile = RudroidEngineProfile {
        id: HostStr::new(descriptor::leak_cstr("engine_native_activity")),
        validate_config: Some(validate_config),
        build_guest_plan: Some(build_guest_plan),
        prepare: Some(prepare),
        bootstrap: Some(bootstrap),
        run: Some(run),
    };
    if !api.register_profile(&profile) {
        api.log_line(LOG_TAG, "failed to register engine_native_activity profile");
        return -5;
    }
    api.log_line(LOG_TAG, "engine_native_activity profile registered");
    0
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
