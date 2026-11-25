//! INPUT: exact-base-APK-gated NativeUtil signature queries through the shared VM
//! OUTPUT: the original APK signer's SHA-1 as a VM-owned uppercase hex String
//! POS: Title-only value source; no PackageManager, SDK initialization or identity verification
#![forbid(unsafe_code)]

use rudroid_common::jni_env::{JObject, JniEnv, Native};

// The base APK accepted by identity::verify has one v2 signer/certificate.
// Its 471-byte certificate DER has SHA-256
// e2bb0d8f9d7c76e5256ed29e9cd0b026f8aad3cc5e5dfde1da6df141ea6aabf2.
// Original NativeUtil.getSignatureHash("SHA-1") hashes Signature.toByteArray()
// and uppercases the hex result. Revalidate this value whenever the APK gate changes;
// it is the certificate hash, not an account/device ID or verification result.
const SIGNER_SHA1: &std::ffi::CStr = c"DD96D1D8A18AA4165136F002CD56ADAECFD7955F";

extern "C" fn signature_sha1(env: JniEnv, _class: JObject) -> JObject {
    env.new_string_utf(SIGNER_SHA1)
}

/// Called only after exact APK identity and default DEX catalog validation.
pub(super) fn register(env: JniEnv) -> i32 {
    env.register_class(
        c"com/chillyroomsdk/sdkbridge/util/NativeUtil",
        &[Native::new0(
            c"getSignatureSHA1Hash",
            c"()Ljava/lang/String;",
            signature_sha1,
        )],
    )
}
