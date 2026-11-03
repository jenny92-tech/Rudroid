//! INPUT: shared class/method authority, exact native callbacks and real C JNI ABI probes
//! OUTPUT: override/provenance rejection, all ten nonvirtual return families and revocation tests
//! POS: adversarial tests for shared nonvirtual selection; no locks cross guest callbacks
use super::super::{test_support, MethodValue, Session};
use super::*;

#[test]
fn nonvirtual_selection_obeys_supplied_class_and_exact_method_provenance() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    test_support::stub_clear();
    let s = Session::new();
    s.install_host(test_support::stub_host());
    let base = s.find_class("test/NonvirtualBase").unwrap();
    let child = s.find_class("test/NonvirtualChild").unwrap();
    let other = s.find_class("test/NonvirtualOther").unwrap();
    s.set_superclass(child, base);
    test_support::stub_set("test/NonvirtualBase", "value", "()I", MethodValue::Int(11));
    test_support::stub_set("test/NonvirtualChild", "value", "()I", MethodValue::Int(22));
    let base_id = s.resolve_method(base, "value", "()I", "test").unwrap();
    let child_id = s.resolve_method(child, "value", "()I", "test").unwrap();
    let object = s.alloc_object(child);
    let call = |class, method| {
        s.call_checked_dispatch(
            object,
            method,
            abi::MethodKind::Int,
            "CallNonvirtualIntMethodA",
            Dispatch::Nonvirtual(class),
        )
    };
    assert_eq!(
        s.call_checked_dispatch(
            object,
            base_id,
            abi::MethodKind::Int,
            "virtual",
            Dispatch::Virtual
        ),
        Some(MethodValue::Int(22))
    );
    assert_eq!(call(base, base_id), Some(MethodValue::Int(11)));
    assert_eq!(call(child, child_id), Some(MethodValue::Int(22)));
    assert_eq!(call(base, child_id), None);
    assert!(s.take_pending().is_some());
    assert_eq!(
        call(child, base_id),
        None,
        "child GetMethodID resolves its override"
    );
    assert!(s.take_pending().is_some());
    assert_eq!(call(other, base_id), None);
    assert!(s.take_pending().is_some());
    assert_eq!(call(std::ptr::null_mut(), base_id), None);
    assert!(s.take_pending().is_some());
    s.set_pending("java/lang/IllegalStateException", "prior exception");
    assert_eq!(call(base, base_id), None);
    assert!(s.take_pending().is_some());
    assert_eq!(
        s.call_checked_dispatch(
            object,
            base_id,
            abi::MethodKind::Float,
            "wrong accessor",
            Dispatch::Nonvirtual(base)
        ),
        None
    );
    assert!(s.take_pending().is_some());
    // Inherited IDs remain valid when GetMethodID on the supplied subclass
    // resolves the same declaration, rather than a child override.
    test_support::stub_set(
        "test/NonvirtualBase",
        "inherited",
        "()I",
        MethodValue::Int(33),
    );
    let inherited = s.resolve_method(child, "inherited", "()I", "test").unwrap();
    assert_eq!(call(child, inherited), Some(MethodValue::Int(33)));
    s.delete_ref(object, false, "DeleteLocalRef");
    assert_eq!(call(base, base_id), None, "deleted receiver is rejected");
    assert!(s.take_pending().is_some());
    let interface = s.find_class("java/lang/Runnable").unwrap();
    let class_type = s.find_class("java/lang/Class").unwrap();
    let interfaces = s.new_object_array(1, class_type, interface);
    let proxy = s.proxy_class(base, interfaces).unwrap();
    let proxy_object = s.alloc_object(proxy);
    test_support::stub_set(
        "java/lang/Runnable",
        "defaultValue",
        "()I",
        MethodValue::Int(44),
    );
    let default = s
        .resolve_method(interface, "defaultValue", "()I", "test")
        .unwrap();
    assert_eq!(
        s.call_checked_dispatch(
            proxy_object,
            default,
            abi::MethodKind::Int,
            "interface default",
            Dispatch::Nonvirtual(interface)
        ),
        Some(MethodValue::Int(44))
    );
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn actual_nonvirtual_a_variadic_and_compiler_va_lists_cover_all_return_kinds() {
    use rudroid_common::jni_env::{JObject, JniEnv, Native};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static VOIDS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn object(_: JniEnv, receiver: JObject) -> JObject {
        receiver
    }
    extern "C" fn boolean(_: JniEnv, _: JObject) -> u8 {
        1
    }
    extern "C" fn byte(_: JniEnv, _: JObject) -> i8 {
        -127
    }
    extern "C" fn character(_: JniEnv, _: JObject) -> u16 {
        65535
    }
    extern "C" fn short(_: JniEnv, _: JObject) -> i16 {
        -32767
    }
    extern "C" fn int(_: JniEnv, _: JObject) -> i32 {
        0x1234567
    }
    extern "C" fn long(_: JniEnv, _: JObject) -> i64 {
        0x1122334455667788
    }
    extern "C" fn float(_: JniEnv, _: JObject) -> f32 {
        1.25
    }
    extern "C" fn double(_: JniEnv, _: JObject) -> f64 {
        1234567.125
    }
    extern "C" fn void(_: JniEnv, _: JObject) {
        VOIDS.fetch_add(1, Ordering::Relaxed);
    }
    extern "C" {
        fn rudroid_test_nonvirtual_calls(
            env: *mut c_void,
            object: *mut c_void,
            class: *mut c_void,
            methods: *const *mut c_void,
        ) -> i32;
    }
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    super::super::init_with_host(test_support::stub_host());
    let s = super::super::session().unwrap();
    s.take_pending();
    // SAFETY: process-pinned shared table, trusted exact native prototypes.
    let env = unsafe { JniEnv::from_raw(abi::env_ptr()) };
    let class = env.find_class(c"test/NonvirtualAbi");
    assert_eq!(
        env.register_class(
            c"test/NonvirtualAbi",
            &[
                Native::new0(c"object", c"()Ljava/lang/Object;", object),
                Native::new0(c"boolean", c"()Z", boolean),
                Native::new0(c"byte", c"()B", byte),
                Native::new0(c"char", c"()C", character),
                Native::new0(c"short", c"()S", short),
                Native::new0(c"int", c"()I", int),
                Native::new0(c"long", c"()J", long),
                Native::new0(c"float", c"()F", float),
                Native::new0(c"double", c"()D", double),
                Native::new0(c"void", c"()V", void),
            ]
        ),
        0
    );
    let receiver = env.alloc_object(class);
    let methods = [
        (c"object", c"()Ljava/lang/Object;"),
        (c"boolean", c"()Z"),
        (c"byte", c"()B"),
        (c"char", c"()C"),
        (c"short", c"()S"),
        (c"int", c"()I"),
        (c"long", c"()J"),
        (c"float", c"()F"),
        (c"double", c"()D"),
        (c"void", c"()V"),
    ]
    .map(|(name, signature)| env.get_method_id(class, name, signature));
    assert!(methods.iter().all(|method| !method.is_null()));
    VOIDS.store(0, Ordering::Relaxed);
    // SAFETY: the C compiler owns every va_list; all refs/IDs and ten exact
    // prototypes remain live, and no Java argument storage is required.
    assert_eq!(
        unsafe { rudroid_test_nonvirtual_calls(env.as_raw(), receiver, class, methods.as_ptr()) },
        0
    );
    assert_eq!(VOIDS.load(Ordering::Relaxed), 3);
    assert!(!env.exception_check());
    assert_eq!(s.unregister_natives(class), 0);
    // SAFETY: a revoked cached ID must fail before calling its old address.
    unsafe {
        call_nonvirtual_void_a(env.as_raw(), receiver, class, methods[9], std::ptr::null());
    }
    assert!(env.exception_check());
    assert_eq!(VOIDS.load(Ordering::Relaxed), 3);
    env.exception_clear();
    assert!(define_class_unavailable(
        env.as_raw(),
        std::ptr::null(),
        std::ptr::null_mut(),
        std::ptr::null(),
        i32::MAX
    )
    .is_null());
    assert!(env.exception_check());
    env.exception_clear();
    env.delete_local_ref(receiver);
    env.delete_local_ref(class);
}
