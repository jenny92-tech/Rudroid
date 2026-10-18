//! INPUT: shared JNI VM strings and the authoritative already-mapped guest image chain
//! OUTPUT: System.load/loadLibrary checks for mapped images or provided services, without initialization replay
//! POS: Java native-library requests join the ELF owner; unknown libraries fail explicitly
#![forbid(unsafe_code)]
use rudroid_common::{host_api::{HostApiV0, HOST_API_VERSION}, jni_env::{JObject, JniEnv, Native, JNI_VERSION_1_6}};
use std::{ffi::CString, sync::atomic::{AtomicUsize,Ordering}};
static REPORTS: AtomicUsize = AtomicUsize::new(0);
fn fail(env: JniEnv, message: &str) {
    let class=env.find_class(c"java/lang/UnsatisfiedLinkError");
    let message=CString::new(message.replace('\0'," ")).unwrap();
    env.throw_new(class,&message);env.delete_local_ref(class);
}
fn load(env:JniEnv,name:JObject,library:bool) {
    let Some(units)=env.string_utf16(name) else {fail(env,"native library name is null or invalid");return;};
    let Ok(requested)=String::from_utf16(&units) else {fail(env,"native library name is not valid UTF-16");return;};
    if requested.is_empty() || requested.len()>4096 || requested.contains('\0') || library && requested.contains('/') {
        fail(env,"invalid native library name");return;
    }
    let path=if library {format!("lib{requested}.so")}else{requested.clone()};
    // The loader already maps and initializes the selected guest modules.
    // System.load must acknowledge that same image, never initialize it twice.
    let name=std::path::Path::new(&path).file_name().and_then(|n|n.to_str()).and_then(|n|CString::new(n).ok());
    let loaded=name.is_some_and(|name| {
        super::libdl::java_service_available(&name)
            || super::exec::reopen_guest(&name).is_some_and(super::exec::close_guest)
    });
    if REPORTS.fetch_add(1,Ordering::Relaxed)<3 {
        eprintln!("[ELF-JAVA] native library request {requested:?}: already_loaded={loaded}");
    }
    if !loaded {fail(env,&format!("native library has no initialized guest image or active service provider: {requested}"));}
}
extern "C" fn system_load(env:JniEnv,_class:JObject,path:JObject){load(env,path,false)}
extern "C" fn system_load_library(env:JniEnv,_class:JObject,name:JObject){load(env,name,true)}
pub(super) fn register(api:Option<&HostApiV0>)->i32 {
    let Some(api)=api.filter(|api|api.abi_version==HOST_API_VERSION) else{return -1;};
    let Some(vm)=api.java_vm() else{return 0;};
    let Some(env)=vm.get_env(JNI_VERSION_1_6) else{return -1;};
    env.register_class(c"java/lang/System",&[
        Native::new1(c"load",c"(Ljava/lang/String;)V",system_load),
        Native::new1(c"loadLibrary",c"(Ljava/lang/String;)V",system_load_library),
    ])
}
