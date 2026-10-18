//! INPUT: shared JNI/HostApi and IronSource LevelPlay Unity bridge requests
//! OUTPUT: offline initialization failure dispatched through the main Looper
//! POS: optional Android Addon; shared VM owns bridge, listener and queued job
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_abi::{
    declare_field, declare_superclass, invoke_proxy_callback, post_main_runnable,
};
use rudroid_common::jni_env::{JFieldId, JObject, JniEnv, Native, JNI_VERSION_1_6};
use std::ffi::{c_void, CStr};
use std::sync::OnceLock;
mod quality;
rudroid_common::declare_descriptor!(
    "sdk_levelplay",
    rudroid_common::Kind::AndroidAddon,
    package = None::<&str>,
    requires = Some("android_base")
);
const BRIDGE: &CStr = c"com/ironsource/unity/androidbridge/LevelPlayBridge";
const JOB: &CStr = c"org/rudroid/levelplay/Initialization";
static INSTANCE: OnceLock<usize> = OnceLock::new();
fn alloc(env: JniEnv, name: &CStr) -> JObject {
    let class = env.find_class(name);
    let result = env.alloc_object(class);
    env.delete_local_ref(class);
    result
}
extern "C" fn instance(env: JniEnv, _: JObject) -> JObject {
    let value = *INSTANCE.get_or_init(|| {
        let value = alloc(env, BRIDGE);
        let result = env.new_global_ref(value);
        env.delete_local_ref(value);
        result as usize
    });
    env.new_local_ref(value as *mut c_void)
}
fn listener_field(env: JniEnv) -> JFieldId {
    let class = env.find_class(JOB);
    let result = env.get_field_id(class, c"listener", c"Ljava/lang/Object;");
    env.delete_local_ref(class);
    result
}
extern "C" fn initialize(env: JniEnv, _: JObject, _key: JObject, _user: JObject, listener: JObject) {
    if listener.is_null() {
        return;
    }
    let job = alloc(env, JOB);
    env.set_object_field(job, listener_field(env), listener);
    post_main_runnable(env, job);
    env.delete_local_ref(job);
}
extern "C" fn run(env: JniEnv, job: JObject) {
    let listener = env.get_object_field(job, listener_field(env));
    let error = env.new_string_utf(
        c"{\"errorCode\":-1,\"errorMessage\":\"Advertising unavailable in offline Linux runtime\"}",
    );
    invoke_proxy_callback(
        env,
        listener,
        c"com/ironsource/unity/androidbridge/IUnityLevelPlayInitListener",
        c"onInitFailed",
        c"(Ljava/lang/String;)V",
        error,
    );
    env.delete_local_ref(error);
    env.delete_local_ref(listener);
}
extern "C" fn one(_: JniEnv, _: JObject, _: JObject) {}
extern "C" fn two(_: JniEnv, _: JObject, _: JObject, _: JObject) {}
extern "C" fn three(_: JniEnv, _: JObject, _: JObject, _: JObject, _: JObject) {}
extern "C" fn boolean(_: JniEnv, _: JObject, _: u8) {}
extern "C" fn empty(_: JniEnv, _: JObject) {}
extern "C" fn denied(_: JniEnv, _: JObject, _: JObject) -> u8 {
    0
}
/// Loader-owned HostApi; this capability remains mapped while JNI callbacks exist.
#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> i32 {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let Some(vm) = api.java_vm() else {
        return -3;
    };
    let Some(env) = vm.get_env(JNI_VERSION_1_6) else {
        return -3;
    };
    if declare_field(env, JOB, c"listener", c"Ljava/lang/Object;", false) != 0
        || declare_superclass(env, JOB, c"java/lang/Runnable") != 0
    {
        return -4;
    }
    for (class,natives) in [
        (BRIDGE,vec![Native::new0(c"getInstance",c"()Lcom/ironsource/unity/androidbridge/LevelPlayBridge;",instance),Native::new3(c"initialize",c"(Ljava/lang/String;Ljava/lang/String;Lcom/ironsource/unity/androidbridge/IUnityLevelPlayInitListener;)V",initialize),Native::new3(c"setPluginData",c"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",three),Native::new2(c"setMetaData",c"(Ljava/lang/String;Ljava/lang/String;)V",two),Native::new2(c"setMetaData",c"(Ljava/lang/String;[Ljava/lang/String;)V",two),Native::new2(c"setNetworkData",c"(Ljava/lang/String;Ljava/lang/String;)V",two),Native::new1(c"setConsent",c"(Z)V",boolean),Native::new1(c"setAdaptersDebug",c"(Z)V",boolean),Native::new1(c"setDynamicUserId",c"(Ljava/lang/String;)Z",denied),Native::new1(c"setSegment",c"(Ljava/lang/String;)V",one),Native::new1(c"setUnityImpressionDataListener",c"(Lcom/ironsource/unity/androidbridge/UnityImpressionDataListener;)V",one),Native::new0(c"validateIntegration",c"()V",empty),Native::new0(c"launchTestSuite",c"()V",empty)]),
        (JOB,vec![Native::new0(c"run",c"()V",run)]),
        (c"com/ironsource/unity/androidbridge/IUnityLevelPlayInitListener",vec![Native::new1(c"onInitFailed",c"(Ljava/lang/String;)V",one),Native::new1(c"onInitSuccess",c"(Ljava/lang/String;)V",one)]),
    ]{
        if env.register_class(class, &natives) != 0 {
            return -4;
        }
    }
    if quality::register(env) != 0 || env.exception_check() {
        -4
    } else {
        0
    }
}
