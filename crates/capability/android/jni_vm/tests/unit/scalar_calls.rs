//! INPUT: exact scalar descriptors, registered natives and real JNI tables
//! OUTPUT: scalar width/classification, accessor rejection and exact promoted native calls
//! POS: unit regressions for the shared scalar boundary; C V probes are linked only here
use super::*;
#[test]
fn scalar_classification_accepts_only_prototypes_the_dispatcher_can_call() {
    for (signature, returned, argument) in [
        ("()B", Return::Byte, Argument::None),
        ("(C)C", Return::Char, Argument::Char),
        ("(S)S", Return::Short, Argument::Short),
        ("(D)D", Return::Double, Argument::Double),
        ("(F)D", Return::Double, Argument::Float),
    ] {
        assert_eq!(
            classify(signature, 16),
            Some(MethodValue::NativeScalar(16, returned, argument))
        );
    }
    for signature in [
        "(II)B",
        "(Ljava/lang/Object;)D",
        "(D)Dgarbage",
        "(V)B",
        "I)B",
        "(B)I",
        "(B))B",
    ] {
        assert_eq!(classify(signature, 16), None, "{signature}");
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn real_jni_scalar_a_variadic_and_compiler_va_lists_preserve_bits() {
    use crate::{
        abi,
        session::{self, shims, test_support},
    };
    use rudroid_common::jni_env::{JObject, JniEnv, Native};
    extern "C" fn byte(_: JniEnv, _: JObject, value: i8) -> i8 {
        value
    }
    extern "C" fn character(_: JniEnv, _: JObject, value: u16) -> u16 {
        value
    }
    extern "C" fn short(_: JniEnv, _: JObject, value: i16) -> i16 {
        value
    }
    extern "C" fn double(_: JniEnv, _: JObject, value: f64) -> f64 {
        value
    }
    extern "C" {
        fn rudroid_test_scalar_calls(
            env: *mut c_void,
            object: *mut c_void,
            class: *mut c_void,
            byte: *mut c_void,
            character: *mut c_void,
            short: *mut c_void,
            double: *mut c_void,
        ) -> i32;
    }
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    session::init_with_host(test_support::stub_host());
    let s = session::session().unwrap();
    s.take_pending();
    // SAFETY: process-pinned VM table; the fixture uses descriptor-matched exact scalar native ABIs.
    let env = unsafe { JniEnv::from_raw(abi::env_ptr()) };
    let class = env.find_class(c"test/ScalarCalls");
    assert_eq!(
        env.register_class(
            c"test/ScalarCalls",
            &[
                Native::new1(c"byte", c"(B)B", byte),
                Native::new1(c"char", c"(C)C", character),
                Native::new1(c"short", c"(S)S", short),
                Native::new1(c"double", c"(D)D", double)
            ]
        ),
        0
    );
    let object = env.alloc_object(class);
    let methods = [
        env.get_method_id(class, c"byte", c"(B)B"),
        env.get_method_id(class, c"char", c"(C)C"),
        env.get_method_id(class, c"short", c"(S)S"),
        env.get_method_id(class, c"double", c"(D)D"),
    ];
    assert!(methods.iter().all(|id| !id.is_null()));
    // SAFETY: exact fixture ABI, live references/methods, compiler owns each va_list lifetime.
    assert_eq!(
        unsafe {
            rudroid_test_scalar_calls(
                env.as_raw(),
                object,
                class,
                methods[0],
                methods[1],
                methods[2],
                methods[3],
            )
        },
        0
    );
    assert!(!env.exception_check());
    let byte_id = methods[0];
    let arg = JValue { b: 1 };
    // SAFETY: valid storage/IDs, wrong accessor must fail before any typed native call.
    assert_eq!(
        unsafe { shims::call_int_method_a(env.as_raw(), object, byte_id, &arg) },
        0
    );
    assert!(env.exception_check());
    env.exception_clear();
    assert_eq!(s.unregister_natives(class), 0);
    // SAFETY: a cached ID after unregister must fail without invoking revoked code.
    assert_eq!(
        unsafe { call_byte_method_a(env.as_raw(), object, byte_id, &arg) },
        0
    );
    assert!(env.exception_check());
    env.exception_clear();
}
