//! INPUT: same-kit Host API, shared JNI VM and the native Main module event
//! OUTPUT: package-scoped Terraria Activity fields and fixed-display frame-rate callback
//! POS: Title capability; Android base and other games receive no custom fields
#![deny(unsafe_op_in_unsafe_fn)]
mod java;
use rudroid_common::{host_api::{HostApiV0,HOST_API_VERSION},registry::RudroidModuleEvent,ModuleKind};
use std::{ffi::c_void,sync::OnceLock};

rudroid_common::declare_descriptor!(
    "terraria_android", rudroid_common::Kind::GamePatch,
    package = Some("com.and.games505.TerrariaPaid"),
    requires = Some("android_base,unity_2021_3")
);
static HOST:OnceLock<HostApiV0>=OnceLock::new();

fn applicable(api:&HostApiV0)->bool {
    api.config_string(c"package.packageName",256).as_deref()==Some("com.and.games505.TerrariaPaid")
}
#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(api:Option<&HostApiV0>)->i32 {
    let Some(api)=api.filter(|api|api.abi_version==HOST_API_VERSION) else{return -2;};
    if !applicable(api){return 0;}
    let _=HOST.set(*api);
    let Some(env)=api.java_vm().and_then(|vm|vm.get_env(0x10006)) else{return -1;};
    java::register(env)
}
#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api:Option<&HostApiV0>)->i32 {
    let Some(api)=api.filter(|api|api.abi_version==HOST_API_VERSION) else{return -2;};
    if !applicable(api){return 0;}
    if api.on_module_loaded(ModuleKind::Main,initialize,std::ptr::null_mut()){0}else{-1}
}
extern "C" fn initialize(event:Option<&RudroidModuleEvent>,_user:*mut c_void) {
    if !event.is_some_and(|e|e.kind==ModuleKind::Main as u32){return;}
    let Some(api)=HOST.get() else{return;};
    if let Some(env)=api.java_vm().and_then(|vm|vm.get_env(0x10006)) {
        if java::initialize(env){api.log_line(c"TERRARIA","Activity keyboard/mouse state initialized");}
        else{api.log_line(c"TERRARIA","Activity keyboard/mouse initialization failed");}
    }
}
