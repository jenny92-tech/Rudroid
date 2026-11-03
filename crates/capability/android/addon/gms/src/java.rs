//! INPUT: shared JNI session and GMS Play Games authentication requests
//! OUTPUT: completed unauthenticated tasks through common's VM-owned callback machinery
//! POS: optional GMS facade; no duplicate task ownership, queue or native object pointers
#![forbid(unsafe_code)]
use rudroid_common::{
    jni_env::{JObject, JniEnv, Native},
    jni_tasks::{self, Namespace},
};
use std::ffi::CStr;
fn alloc(e: JniEnv, name: &CStr) -> JObject {
    let c = e.find_class(name);
    let o = e.alloc_object(c);
    e.delete_local_ref(c);
    o
}
extern "C" fn no(_: JniEnv, _: JObject) -> u8 {
    0
}
extern "C" fn initialize(_: JniEnv, _: JObject, _: JObject) {}
extern "C" fn client(e: JniEnv, _: JObject, _: JObject) -> JObject {
    alloc(e, c"com/google/android/gms/games/GamesSignInClient")
}
extern "C" fn authentication(e: JniEnv, _: JObject) -> JObject {
    let auth = alloc(e, c"com/google/android/gms/games/AuthenticationResult");
    let task = jni_tasks::completed(e, Namespace::Gms, auth);
    e.delete_local_ref(auth);
    task
}
pub(super) fn register(e: JniEnv) -> i32 {
    if jni_tasks::register(e, Namespace::Gms) != 0 {
        return -1;
    }
    for (class, natives) in [
        (
            c"com/google/android/gms/games/PlayGamesSdk",
            vec![Native::new1(
                c"initialize",
                c"(Landroid/content/Context;)V",
                initialize,
            )],
        ),
        (
            c"com/google/android/gms/games/PlayGames",
            vec![Native::new1(
                c"getGamesSignInClient",
                c"(Landroid/app/Activity;)Lcom/google/android/gms/games/GamesSignInClient;",
                client,
            )],
        ),
        (
            c"com/google/android/gms/games/GamesSignInClient",
            vec![
                Native::new0(
                    c"isAuthenticated",
                    c"()Lcom/google/android/gms/tasks/Task;",
                    authentication,
                ),
                Native::new0(
                    c"signIn",
                    c"()Lcom/google/android/gms/tasks/Task;",
                    authentication,
                ),
            ],
        ),
        (
            c"com/google/android/gms/games/AuthenticationResult",
            vec![Native::new0(c"isAuthenticated", c"()Z", no)],
        ),
    ] {
        if e.register_class(class, &natives) != 0 {
            return -1;
        }
    }
    0
}
