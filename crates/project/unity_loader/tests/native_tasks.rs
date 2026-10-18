//! INPUT:  complete production VM/framework/GMS/Play Core kit and isolated Java listener fixtures
//! OUTPUT:  asynchronous callbacks, preserved result snapshots, reentrant jobs and clean queue teardown
//! POS:    native shared-VM acceptance; does not contact Google or claim online account/review success
use rudroid_common::{
    jni_abi::declare_superclass,
    jni_env::{Arg, JObject, JniEnv, Method, Native},
};
use rudroid_unity_loader::host_api;
use std::{ffi::CStr, path::PathBuf, sync::Mutex};
static CALLBACKS: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn method(e: JniEnv, o: JObject, name: &CStr, sig: &CStr) -> Method {
    let c = e.get_object_class(o);
    let m = e.method(c, name, sig).unwrap();
    e.delete_local_ref(c);
    m
}
extern "C" fn succeeded(e: JniEnv, _listener: JObject, result: JObject) {
    CALLBACKS
        .lock()
        .unwrap()
        .push(format!("success:{}", e.string(result).unwrap()));
}
extern "C" fn completed(e: JniEnv, _listener: JObject, task: JObject) {
    let value = e.call_object(
        task,
        method(e, task, c"getResult", c"()Ljava/lang/Object;"),
        &[],
    );
    CALLBACKS
        .lock()
        .unwrap()
        .push(format!("complete:{}", e.string(value).unwrap()));
    e.delete_local_ref(value);
}
extern "C" fn failed(_e: JniEnv, _listener: JObject, _error: JObject) {
    panic!("completed successful local task must not invoke failure listener");
}
fn allocate(e: JniEnv, name: &CStr) -> JObject {
    let c = e.find_class(name);
    let o = e.alloc_object(c);
    e.delete_local_ref(c);
    o
}
#[test]
#[ignore = "run on handheld with a trusted RUDROID_RESOURCE_KIT; uses isolated objects and the real main Looper"]
fn both_task_namespaces_deliver_owned_snapshots_only_from_the_main_queue() {
    let kit = PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("production kit"));
    let config = rudroid_host_config::HostConfig::parse("").unwrap();
    host_api::install_config(Some(config)).unwrap();
    let host = host_api::table();
    // SAFETY: trusted same-kit code, process-pinned throughout JNI methods, references and jobs.
    let libraries: Vec<_> = unsafe {
        [
            "libplatform_log_stderr.so",
            "libreply_catalog.so",
            "libandroid_jni_vm.so",
            "libandroid_base.so",
            "libsdk_google_gms.so",
            "libsdk_google_play_core.so",
        ]
        .into_iter()
        .map(|n| std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(n)).unwrap()))
        .collect()
    };
    for phase in [
        rudroid_unity_loader::phases::Phase::EarlyInit,
        rudroid_unity_loader::phases::Phase::RegisterJni,
        rudroid_unity_loader::phases::Phase::Init,
    ] {
        for l in &libraries {
            // SAFETY: exact phase ABI and pinned libraries.
            match unsafe { host_api::run_phase_entry(phase, l) } {
                Ok(()) | Err(host_api::InitError::NotPresent) => {}
                Err(e) => panic!("phase failed: {e:?}"),
            }
        }
    }
    let e = host.java_vm().unwrap().get_env(0x10006).unwrap();
    let ui = e.find_class(c"org/rudroid/runtime/UiThread");
    let drain = e.static_method(ui, c"drain", c"()V").unwrap();
    for (task_name,success_name,complete_name,failure_name,success_sig,complete_sig,failure_sig,callback_sig) in [
 (c"com/google/android/gms/tasks/Task",c"com/google/android/gms/tasks/OnSuccessListener",c"com/google/android/gms/tasks/OnCompleteListener",c"com/google/android/gms/tasks/OnFailureListener",c"(Lcom/google/android/gms/tasks/OnSuccessListener;)Lcom/google/android/gms/tasks/Task;",c"(Lcom/google/android/gms/tasks/OnCompleteListener;)Lcom/google/android/gms/tasks/Task;",c"(Lcom/google/android/gms/tasks/OnFailureListener;)Lcom/google/android/gms/tasks/Task;",c"(Lcom/google/android/gms/tasks/Task;)V"),
 (c"com/google/android/play/core/tasks/Task",c"com/google/android/play/core/tasks/OnSuccessListener",c"com/google/android/play/core/tasks/OnCompleteListener",c"com/google/android/play/core/tasks/OnFailureListener",c"(Lcom/google/android/play/core/tasks/OnSuccessListener;)Lcom/google/android/play/core/tasks/Task;",c"(Lcom/google/android/play/core/tasks/OnCompleteListener;)Lcom/google/android/play/core/tasks/Task;",c"(Lcom/google/android/play/core/tasks/OnFailureListener;)Lcom/google/android/play/core/tasks/Task;",c"(Lcom/google/android/play/core/tasks/Task;)V")]{
  CALLBACKS.lock().unwrap().clear();let (success_class,complete_class,failure_class)=if task_name==c"com/google/android/gms/tasks/Task"{(c"org/rudroid/tests/GmsSuccessListener",c"org/rudroid/tests/GmsCompleteListener",c"org/rudroid/tests/GmsFailureListener")}else{(c"org/rudroid/tests/PlaySuccessListener",c"org/rudroid/tests/PlayCompleteListener",c"org/rudroid/tests/PlayFailureListener")};
  assert_eq!(declare_superclass(e,success_class,success_name),0);assert_eq!(declare_superclass(e,complete_class,complete_name),0);assert_eq!(declare_superclass(e,failure_class,failure_name),0);
  assert_eq!(e.register_class(success_class,&[Native::new1(c"onSuccess",c"(Ljava/lang/Object;)V",succeeded)]),0);assert_eq!(e.register_class(complete_class,&[Native::new1(c"onComplete",callback_sig,completed)]),0);assert_eq!(e.register_class(failure_class,&[Native::new1(c"onFailure",c"(Ljava/lang/Exception;)V",failed)]),0);
  let s=allocate(e,success_class);let c=allocate(e,complete_class);let f=allocate(e,failure_class);let class=e.find_class(task_name);let ctor=e.method(class,c"<init>",c"(Ljava/lang/Object;)V").unwrap();let first=e.new_string(&"first".encode_utf16().collect::<Vec<_>>());let task=e.new_object(class,ctor,&[Arg::object(first)]);e.delete_local_ref(first);
  for (name,sig,listener) in [(c"addOnSuccessListener",success_sig,s),(c"addOnCompleteListener",complete_sig,c),(c"addOnFailureListener",failure_sig,f)]{let returned=e.call_object(task,method(e,task,name,sig),&[Arg::object(listener)]);assert!(e.is_same_object(returned,task));e.delete_local_ref(returned);}
  assert!(CALLBACKS.lock().unwrap().is_empty(),"listener registration must not call back synchronously");
  let second=e.new_string(&"second".encode_utf16().collect::<Vec<_>>());e.call_void(task,ctor,&[Arg::object(second)]);e.delete_local_ref(second);
  for o in [task,class,s,c,f]{e.delete_local_ref(o);}e.call_static_void(ui,drain,&[]);assert_eq!(&*CALLBACKS.lock().unwrap(),&["success:first","complete:second"]);assert!(!e.exception_check());
 }
    let factory = e.find_class(c"com/google/android/play/core/review/ReviewManagerFactory");
    let create = e
        .static_method(
            factory,
            c"create",
            c"(Landroid/content/Context;)Lcom/google/android/play/core/review/ReviewManager;",
        )
        .unwrap();
    let manager = e.call_static_object(factory, create, &[Arg::object(std::ptr::null_mut())]);
    assert!(!manager.is_null());
    e.delete_local_ref(manager);
    e.delete_local_ref(factory);
    e.delete_local_ref(ui);
    assert!(!e.exception_check());
}
