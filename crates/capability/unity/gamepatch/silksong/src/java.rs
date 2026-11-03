//! INPUT:  shared JNI Context path authority and the Silksong package gate
//! OUTPUT:  the title's getAPKPath Java resource callback
//! POS:    supplies the installed/extracted package location without fabricating identity
#![forbid(unsafe_code)]

use rudroid_common::jni_env::{JObject, JniEnv, Native};

extern "C" fn package_path(env: JniEnv, _class: JObject) -> JObject {
    let player = env.find_class(c"com/unity3d/player/UnityPlayer");
    let activity = env.get_static_object_field(player,
        env.get_static_field_id(player, c"currentActivity", c"Landroid/app/Activity;"));
    let context = env.find_class(c"android/content/Context");
    let result = env.method(context, c"getPackageCodePath", c"()Ljava/lang/String;")
        .map_or(std::ptr::null_mut(), |method| env.call_object(activity, method, &[]));
    for local in [context, activity, player] { env.delete_local_ref(local); }
    result
}

pub(super) fn register(env: JniEnv) -> i32 {
    env.register_class(c"com/game/signverify/SignVerify", &[
        Native::new0(c"getAPKPath", c"()Ljava/lang/String;", package_path),
    ])
}
