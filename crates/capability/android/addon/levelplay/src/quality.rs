//! INPUT: IronSource AdQuality configuration and Unity listener requests
//! OUTPUT: explicit offline initialization failure queued on the shared main Looper
//! POS: vendor SDK façade; listener and job edges belong to the single JNI VM
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::{
    declare_field, declare_superclass, invoke_proxy_arguments, post_main_runnable,
};
use rudroid_common::jni_env::{Arg, JFieldId, JObject, JniEnv, Native};
use std::ffi::CStr;
const SDK: &CStr = c"com/ironsource/adqualitysdk/sdk/unity/IronSourceAdQuality";
const LISTENER: &CStr = c"com/ironsource/adqualitysdk/sdk/unity/UnityISAdQualityInitListener";
const JOB: &CStr = c"org/rudroid/levelplay/QualityInitialization";
fn field(env: JniEnv, class: &CStr, name: &CStr, sig: &CStr, static_: bool) -> JFieldId {
    let class = env.find_class(class);
    let result = if static_ {
        env.get_static_field_id(class, name, sig)
    } else {
        env.get_field_id(class, name, sig)
    };
    env.delete_local_ref(class);
    result
}
extern "C" fn set_listener(env: JniEnv, class: JObject, listener: JObject) {
    env.set_static_object_field(
        class,
        field(env, SDK, c"listener", c"Ljava/lang/Object;", true),
        listener,
    );
}
extern "C" fn initialize(
    env: JniEnv,
    class: JObject,
    _key: JObject,
    _user: JObject,
    _test: u8,
    _debug: u8,
    _level: JObject,
    _segment: JObject,
    _consent: u8,
    _id: JObject,
    _enabled: u8,
) {
    let listener = env.get_static_object_field(
        class,
        field(env, SDK, c"listener", c"Ljava/lang/Object;", true),
    );
    if listener.is_null() {
        return;
    }
    let job = super::alloc(env, JOB);
    env.set_object_field(
        job,
        field(env, JOB, c"listener", c"Ljava/lang/Object;", false),
        listener,
    );
    post_main_runnable(env, job);
    env.delete_local_ref(job);
    env.delete_local_ref(listener);
}
extern "C" fn run(env: JniEnv, job: JObject) {
    let listener = env.get_object_field(
        job,
        field(env, JOB, c"listener", c"Ljava/lang/Object;", false),
    );
    let class = env.find_class(c"java/lang/Integer");
    let number = env
        .method(class, c"<init>", c"(I)V")
        .map_or(std::ptr::null_mut(), |ctor| env.new_object(class, ctor, &[Arg::int(-1)]));
    let error = env.new_string_utf(c"AdQuality unavailable in offline Linux runtime");
    invoke_proxy_arguments(
        env,
        listener,
        LISTENER,
        c"adQualitySdkInitFailed",
        c"(ILjava/lang/String;)V",
        &[Arg::int(-1), Arg::object(error)],
        &[number, error],
    );
    for value in [class, number, error, listener] {
        env.delete_local_ref(value);
    }
}
extern "C" fn from_int(env: JniEnv, class: JObject, value: i32) -> JObject {
    let result = env.alloc_object(class);
    env.set_int_field(result, env.get_field_id(class, c"value", c"I"), value);
    result
}
extern "C" fn value(env: JniEnv, object: JObject) -> i32 {
    let class = env.get_object_class(object);
    let result = env.get_int_field(object, env.get_field_id(class, c"value", c"I"));
    env.delete_local_ref(class);
    result
}
extern "C" fn failure(_env: JniEnv, _object: JObject, _code: i32, _message: JObject) {}
pub(super) fn register(env: JniEnv) -> i32 {
    if declare_field(env, SDK, c"listener", c"Ljava/lang/Object;", true) != 0
        || declare_field(env, JOB, c"listener", c"Ljava/lang/Object;", false) != 0
        || declare_superclass(env, JOB, c"java/lang/Runnable") != 0
    {
        return -1;
    }
    let sdk = env.find_class(SDK);
    if sdk.is_null() {
        return -1;
    }
    let registered = env.register(sdk, &[
        Native::new1(c"setUnityISAdQualityInitListener",c"(Lcom/ironsource/adqualitysdk/sdk/unity/UnityISAdQualityInitListener;)V",set_listener),
        Native::new9(c"initialize",c"(Ljava/lang/String;Ljava/lang/String;ZZLcom/ironsource/adqualitysdk/sdk/ISAdQualityLogLevel;Ljava/lang/String;ZLcom/ironsource/adqualitysdk/sdk/ISAdQualityDeviceIdType;Z)V",initialize),
        Native::new1(c"changeUserId",c"(Ljava/lang/String;)V",super::one),
        Native::new1(c"setSegment",c"(Ljava/lang/String;)V",super::one),
        Native::new1(c"setUserConsent",c"(Z)V",super::boolean),
    ]);
    env.delete_local_ref(sdk);
    if registered != 0 {
        return -1;
    }
    for (class,natives) in [
        (LISTENER,vec![Native::new2(c"adQualitySdkInitFailed",c"(ILjava/lang/String;)V",failure),Native::new0(c"adQualitySdkInitSuccess",c"()V",super::empty)]),
        (JOB,vec![Native::new0(c"run",c"()V",run)]),
    ]{if env.register_class(class,&natives)!=0{return -1;}}
    for (class, sig) in [
        (
            c"com/ironsource/adqualitysdk/sdk/ISAdQualityLogLevel",
            c"(I)Lcom/ironsource/adqualitysdk/sdk/ISAdQualityLogLevel;",
        ),
        (
            c"com/ironsource/adqualitysdk/sdk/ISAdQualityDeviceIdType",
            c"(I)Lcom/ironsource/adqualitysdk/sdk/ISAdQualityDeviceIdType;",
        ),
    ] {
        if declare_field(env, class, c"value", c"I", false) != 0
            || env.register_class(
                class,
                &[
                    Native::new1(c"fromInt", sig, from_int),
                    Native::new0(c"getValue", c"()I", value),
                ],
            ) != 0
        {
            return -1;
        }
    }
    0
}
