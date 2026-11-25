//! INPUT: verified framework class, object and field metadata
//! OUTPUT: owned local references and process-global singletons
//! POS: shared framework JNI object factories
#![forbid(unsafe_code)]
use std::{ffi::{c_void, CStr},sync::OnceLock};
use rudroid_common::jni_env::JniEnv;

pub(super) fn allocate(env: JniEnv, name: &CStr) -> *mut c_void {
    // This factory provides an actual facade object; it declares its type.
    // Guest lookups must remain non-creating under an application catalog.
    let class = rudroid_common::jni_abi::declare_class(env, name);
    let object = env.alloc_object(class); env.delete_local_ref(class); object
}
pub(super) fn set_string_field(env: JniEnv, object: *mut c_void, name: &CStr, string: *mut c_void) {
    let class = env.get_object_class(object); let field = env.get_field_id(class,name,c"Ljava/lang/String;");
    env.set_object_field(object,field,string); env.delete_local_ref(class); env.delete_local_ref(string);
}
pub(super) fn singleton(env: JniEnv, cache: &OnceLock<usize>, create: impl FnOnce()->*mut c_void) -> *mut c_void {
    let global = *cache.get_or_init(|| {
        let object = create();
        if object.is_null() { return 0; }
        let result = env.new_global_ref(object); env.delete_local_ref(object); result as usize
    });
    if global == 0 { std::ptr::null_mut() } else { env.new_local_ref(global as *mut c_void) }
}
