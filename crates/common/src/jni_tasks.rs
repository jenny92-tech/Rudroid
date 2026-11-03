//! INPUT:  shared VM task objects, result references and main-Looper listener requests
//! OUTPUT:  completed local tasks with VM-owned immutable callback snapshots
//! POS:    common GMS/Play Core callback machinery; no capability state or native owner pointers
#![forbid(unsafe_code)]
use crate::{
    jni_abi::{declare_field, declare_superclass, invoke_proxy_callback, post_main_runnable},
    jni_env::{JFieldId, JObject, JniEnv, Native},
};
use std::ffi::CStr;
#[derive(Clone, Copy)]
pub enum Namespace {
    Gms,
    PlayCore,
}
impl Namespace {
    pub fn task(self) -> &'static CStr {
        match self {
            Self::Gms => c"com/google/android/gms/tasks/Task",
            Self::PlayCore => c"com/google/android/play/core/tasks/Task",
        }
    }
    fn number(self) -> i32 {
        match self {
            Self::Gms => 0,
            Self::PlayCore => 1,
        }
    }
}
const JOB: &CStr = c"org/rudroid/runtime/TaskCompletion";
fn alloc(e: JniEnv, name: &CStr) -> JObject {
    let c = e.find_class(name);
    let o = e.alloc_object(c);
    e.delete_local_ref(c);
    o
}
fn field(e: JniEnv, o: JObject, name: &CStr, sig: &CStr) -> JFieldId {
    let c = e.get_object_class(o);
    let f = e.get_field_id(c, name, sig);
    e.delete_local_ref(c);
    f
}
fn get(e: JniEnv, o: JObject, name: &CStr) -> JObject {
    e.get_object_field(o, field(e, o, name, c"Ljava/lang/Object;"))
}
fn put(e: JniEnv, o: JObject, name: &CStr, v: JObject) {
    e.set_object_field(o, field(e, o, name, c"Ljava/lang/Object;"), v)
}
fn error(e: JniEnv, message: &CStr) {
    let c = e.find_class(c"java/lang/IllegalStateException");
    e.throw_new(c, message);
    e.delete_local_ref(c);
}
fn initialize(e: JniEnv, o: JObject, value: JObject, namespace: Namespace) {
    put(e, o, c"result", value);
    e.set_int_field(o, field(e, o, c"taskNamespace", c"I"), namespace.number());
}
pub fn completed(e: JniEnv, namespace: Namespace, result: JObject) -> JObject {
    let task = alloc(e, namespace.task());
    if !task.is_null() {
        initialize(e, task, result, namespace);
    }
    task
}
extern "C" fn init_gms(e: JniEnv, o: JObject, result: JObject) {
    initialize(e, o, result, Namespace::Gms)
}
extern "C" fn init_play(e: JniEnv, o: JObject, result: JObject) {
    initialize(e, o, result, Namespace::PlayCore)
}
extern "C" fn yes(_e: JniEnv, _o: JObject) -> u8 {
    1
}
extern "C" fn no(_e: JniEnv, _o: JObject) -> u8 {
    0
}
extern "C" fn absent(_e: JniEnv, _o: JObject) -> JObject {
    std::ptr::null_mut()
}
extern "C" fn result(e: JniEnv, o: JObject) -> JObject {
    get(e, o, c"result")
}
extern "C" fn listener(_e: JniEnv, _o: JObject, _value: JObject) {}
extern "C" fn run(e: JniEnv, job: JObject) {
    let receiver = get(e, job, c"listener");
    let payload = get(e, job, c"payload");
    let ns = e.get_int_field(job, field(e, job, c"taskNamespace", c"I"));
    let success = e.get_boolean_field(job, field(e, job, c"success", c"Z")) != 0;
    let callback = match (ns, success) {
        (0, false) => Some((
            c"com/google/android/gms/tasks/OnCompleteListener",
            c"onComplete",
            c"(Lcom/google/android/gms/tasks/Task;)V",
        )),
        (0, true) => Some((
            c"com/google/android/gms/tasks/OnSuccessListener",
            c"onSuccess",
            c"(Ljava/lang/Object;)V",
        )),
        (1, false) => Some((
            c"com/google/android/play/core/tasks/OnCompleteListener",
            c"onComplete",
            c"(Lcom/google/android/play/core/tasks/Task;)V",
        )),
        (1, true) => Some((
            c"com/google/android/play/core/tasks/OnSuccessListener",
            c"onSuccess",
            c"(Ljava/lang/Object;)V",
        )),
        _ => None,
    };
    if let Some((interface, name, sig)) = callback {
        invoke_proxy_callback(e, receiver, interface, name, sig, payload);
    } else {
        error(e, c"invalid task callback namespace");
    }
    e.delete_local_ref(payload);
    e.delete_local_ref(receiver);
}
fn listen(e: JniEnv, task: JObject, listener: JObject, success: bool) -> JObject {
    if listener.is_null() {
        return e.new_local_ref(task);
    }
    let ns = e.get_int_field(task, field(e, task, c"taskNamespace", c"I"));
    if !(0..=1).contains(&ns) {
        error(e, c"invalid task namespace");
        return std::ptr::null_mut();
    }
    let job = alloc(e, JOB);
    if job.is_null() {
        return job;
    }
    let payload = if success {
        get(e, task, c"result")
    } else {
        e.new_local_ref(task)
    };
    put(e, job, c"listener", listener);
    put(e, job, c"payload", payload);
    e.set_int_field(job, field(e, job, c"taskNamespace", c"I"), ns);
    e.set_boolean_field(job, field(e, job, c"success", c"Z"), u8::from(success));
    e.delete_local_ref(payload);
    let queued = !e.exception_check() && post_main_runnable(e, job);
    e.delete_local_ref(job);
    if !queued {
        if !e.exception_check() {
            error(e, c"main Looper refused task callback");
        }
        return std::ptr::null_mut();
    }
    e.new_local_ref(task)
}
extern "C" fn complete(e: JniEnv, o: JObject, l: JObject) -> JObject {
    listen(e, o, l, false)
}
extern "C" fn success(e: JniEnv, o: JObject, l: JObject) -> JObject {
    listen(e, o, l, true)
}
extern "C" fn failure(e: JniEnv, o: JObject, _l: JObject) -> JObject {
    e.new_local_ref(o)
}
pub fn register(e: JniEnv, namespace: Namespace) -> i32 {
    let task = namespace.task();
    for (class, name, sig) in [
        (task, c"result", c"Ljava/lang/Object;"),
        (task, c"taskNamespace", c"I"),
        (JOB, c"listener", c"Ljava/lang/Object;"),
        (JOB, c"payload", c"Ljava/lang/Object;"),
        (JOB, c"taskNamespace", c"I"),
        (JOB, c"success", c"Z"),
    ] {
        if declare_field(e, class, name, sig, false) != 0 {
            return -1;
        }
    }
    if declare_superclass(e, JOB, c"java/lang/Runnable") != 0
        || e.register_class(JOB, &[Native::new0(c"run", c"()V", run)]) != 0
    {
        return -1;
    }
    let (ctor,complete_sig,success_sig,failure_sig,interfaces,callback_sig)=match namespace{Namespace::Gms=>(init_gms as extern "C" fn(JniEnv,JObject,JObject),c"(Lcom/google/android/gms/tasks/OnCompleteListener;)Lcom/google/android/gms/tasks/Task;",c"(Lcom/google/android/gms/tasks/OnSuccessListener;)Lcom/google/android/gms/tasks/Task;",c"(Lcom/google/android/gms/tasks/OnFailureListener;)Lcom/google/android/gms/tasks/Task;",[c"com/google/android/gms/tasks/OnCompleteListener",c"com/google/android/gms/tasks/OnSuccessListener",c"com/google/android/gms/tasks/OnFailureListener"],c"(Lcom/google/android/gms/tasks/Task;)V"),Namespace::PlayCore=>(init_play as extern "C" fn(JniEnv,JObject,JObject),c"(Lcom/google/android/play/core/tasks/OnCompleteListener;)Lcom/google/android/play/core/tasks/Task;",c"(Lcom/google/android/play/core/tasks/OnSuccessListener;)Lcom/google/android/play/core/tasks/Task;",c"(Lcom/google/android/play/core/tasks/OnFailureListener;)Lcom/google/android/play/core/tasks/Task;",[c"com/google/android/play/core/tasks/OnCompleteListener",c"com/google/android/play/core/tasks/OnSuccessListener",c"com/google/android/play/core/tasks/OnFailureListener"],c"(Lcom/google/android/play/core/tasks/Task;)V")};
    if e.register_class(
        task,
        &[
            Native::new1(c"<init>", c"(Ljava/lang/Object;)V", ctor),
            Native::new0(c"isComplete", c"()Z", yes),
            Native::new0(c"isSuccessful", c"()Z", yes),
            Native::new0(c"isCanceled", c"()Z", no),
            Native::new0(c"getResult", c"()Ljava/lang/Object;", result),
            Native::new0(c"getException", c"()Ljava/lang/Exception;", absent),
            Native::new1(c"addOnCompleteListener", complete_sig, complete),
            Native::new1(c"addOnSuccessListener", success_sig, success),
            Native::new1(c"addOnFailureListener", failure_sig, failure),
        ],
    ) != 0
    {
        return -1;
    }
    for (class, name, sig) in [
        (interfaces[0], c"onComplete", callback_sig),
        (interfaces[1], c"onSuccess", c"(Ljava/lang/Object;)V"),
        (interfaces[2], c"onFailure", c"(Ljava/lang/Exception;)V"),
    ] {
        if e.register_class(class, &[Native::new1(name, sig, listener)]) != 0 {
            return -1;
        }
    }
    0
}
