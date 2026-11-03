//! INPUT:  UnityPlayer's default Android proxy query in the fixed Linux environment
//! OUTPUT:  explicit null proxy result; null URLs preserve Java exception semantics
//! POS:    safe compatibility query; native Curl owns all network connections
#![forbid(unsafe_code)]

use rudroid_common::jni_env::{JObject, JniEnv};

pub(super) extern "C" fn settings(env: JniEnv, _player: JObject, url: JObject) -> JObject {
    if url.is_null() {
        if !env.exception_check() {
            let class = env.find_class(c"java/lang/NullPointerException");
            env.throw_new(class, c"Proxy URL is null");
            env.delete_local_ref(class);
        }
        return std::ptr::null_mut();
    }
    // No Android Java system-property proxy is configured by this runtime.
    // C's unresolved-method fallback supplied the same absent-proxy result;
    // make that one known query explicit instead of triggering a JNI miss.
    std::ptr::null_mut()
}
