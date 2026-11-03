//! INPUT: real native method dispatch and callback-created JNI locals
//! OUTPUT: native-return local ownership reclamation and returned-object lifetime regression
//! POS: reproduces automatic native-frame leakage without starting a game
use super::*;
use rudroid_common::jni_env::{JObject, JniEnv, Native};

fn weak(s: &Session, object: JObject) -> JObject {
    let target = s.resolve(object).unwrap();
    let id = s.next_serial().unwrap();
    s.weaks.lock().unwrap().insert(id, target);
    make_handle(TAG_WEAK, id)
}

#[test]
fn cleared_weak_native_argument_is_null_without_inventing_an_exception() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let s = Session::new();
    s.install_host(test_support::stub_host());
    let value = s.new_string(b"temporary weak owner");
    let weak = weak(&s, value);
    s.delete_ref(value, false, "last owner");
    assert!(s.resolve(weak).is_none());
    let result = s.with_native_frame(|s| {
        let argument = s.native_argument(weak)?;
        assert!(argument.is_null());
        Ok(9i32)
    });
    assert_eq!(result, 9);
    assert!(s.take_pending().is_none());
    s.weaks.lock().unwrap().remove(&handle_parts(weak).1);
}

#[test]
fn native_argument_survives_last_global_deletion_and_returns_to_the_caller() {
    static ORIGINAL: AtomicU64 = AtomicU64::new(0);
    static BODY_VALID: AtomicU64 = AtomicU64::new(0);
    extern "C" fn identity(env: JniEnv, receiver: JObject, value: JObject) -> JObject {
        env.delete_global_ref(ORIGINAL.swap(0, Ordering::Relaxed) as JObject);
        env.delete_local_ref(receiver); // only the native-owned argument local
        BODY_VALID.store(
            u64::from(
                env.with_string_utf(value, |s| s.to_bytes() == b"return-after-last-global")
                    == Some(true),
            ),
            Ordering::Relaxed,
        );
        value
    }
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host());
    let s = session().unwrap();
    s.take_pending();
    // SAFETY: process-live shared VM and exact native reference prototype.
    let env = unsafe { JniEnv::from_raw(abi::env_ptr()) };
    assert_eq!(
        env.register_class(
            c"test/NativeArgumentRoots",
            &[Native::new1(
                c"identity",
                c"(Ljava/lang/String;)Ljava/lang/String;",
                identity
            )]
        ),
        0
    );
    let class = env.find_class(c"test/NativeArgumentRoots");
    let receiver = env.alloc_object(class);
    let value = env.new_string_utf(c"return-after-last-global");
    let weak = env.new_weak_global_ref(value);
    let global = env.new_global_ref(value);
    env.delete_local_ref(value);
    ORIGINAL.store(global as u64, Ordering::Relaxed);
    let returned = env.call_object(
        receiver,
        env.method(
            class,
            c"identity",
            c"(Ljava/lang/String;)Ljava/lang/String;",
        )
        .unwrap(),
        &[rudroid_common::jni_env::Arg::object(global)],
    );
    assert_eq!(BODY_VALID.load(Ordering::Relaxed), 1);
    assert!(!returned.is_null());
    assert_eq!(
        env.with_string_utf(returned, |text| text.to_bytes().len()),
        Some(24)
    );
    assert!(env.is_instance_of(receiver, class));
    assert!(!env.exception_check());
    assert!(!env.is_same_object(weak, std::ptr::null_mut()));
    env.delete_local_ref(returned);
    assert!(env.is_same_object(weak, std::ptr::null_mut()));
    env.delete_weak_global_ref(weak);
    env.delete_local_ref(receiver);
    env.delete_local_ref(class);
}

#[test]
fn native_scope_restores_nested_frames_and_preserves_return_and_pending_owners() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let s = Session::new();
    s.install_host(test_support::stub_host());
    let caller = s.new_string(b"caller");
    let baseline = s.with_thread(|t| (t.frame, t.locals.len(), t.native_floor));
    let mut dead = std::ptr::null_mut();
    let result = s.with_native_frame(|s| {
        assert_eq!(s.push_local_frame(4), 0);
        let temporary = s.new_string(b"temporary");
        dead = weak(s, temporary);
        assert_eq!(s.push_local_frame(4), 0);
        let returned = s.new_string(b"returned");
        s.set_pending(
            "java/lang/IllegalStateException",
            "native exception survives cleanup",
        );
        Ok(returned)
    });
    assert!(s.resolve(dead).is_none());
    assert_eq!(
        s.with_string_bytes(result, |bytes| bytes.to_vec()),
        Some(b"returned".to_vec())
    );
    let pending = s.take_pending().unwrap();
    assert_eq!(pending.message, "native exception survives cleanup");
    s.delete_ref(result, false, "test cleanup");
    assert_eq!(
        s.with_thread(|t| (t.frame, t.locals.len(), t.native_floor)),
        baseline
    );
    assert!(s.resolve(caller).is_some());
    s.weaks.lock().unwrap().remove(&handle_parts(dead).1);
    s.delete_ref(caller, false, "test cleanup");
}

#[test]
fn native_scope_protects_caller_arguments_boundary_and_unwind_cleanup() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let s = Session::new();
    s.install_host(test_support::stub_host());
    let caller = s.new_string(b"caller");
    let baseline = s.with_thread(|t| (t.frame, t.locals.len(), t.native_floor));
    assert_eq!(
        s.with_native_frame(|s| {
            let input = s.native_argument(caller)?;
            assert_ne!(input, caller);
            s.delete_ref(input, false, "native owns its argument local");
            assert!(s.resolve(caller).is_some());
            assert_eq!(s.detach_current_thread(), abi::JNI_ERR);
            assert!(s.current_thread_is_attached());
            assert!(s.pop_local_frame(std::ptr::null_mut()).is_null());
            assert!(s.take_pending().is_some());
            Ok(7i32)
        }),
        7
    );
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.with_native_frame(|s| -> Result<(), ()> {
            s.new_string(b"unwind temporary");
            s.push_local_frame(4);
            s.new_string(b"nested unwind temporary");
            panic!("expected native scope Rust unwind")
        })
    }));
    assert!(unwind.is_err());
    assert_eq!(
        s.with_thread(|t| (t.frame, t.locals.len(), t.native_floor)),
        baseline
    );
    assert!(s.resolve(caller).is_some());
    s.delete_ref(caller, false, "test cleanup");
}

#[test]
fn native_scope_rejects_invalid_arguments_and_depth_overflow_before_the_body() {
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let s = Session::new();
    s.install_host(test_support::stub_host());
    let mut called = false;
    let baseline = s.with_thread(|t| (t.frame, t.locals.len(), t.native_floor));
    let value = s.with_native_frame(|s| {
        s.native_argument(1usize as *mut c_void)?;
        called = true;
        Ok(5i32)
    });
    assert_eq!(value, 0);
    assert!(!called);
    assert!(s.take_pending().is_some());
    assert_eq!(
        s.with_thread(|t| (t.frame, t.locals.len(), t.native_floor)),
        baseline
    );
    s.with_thread(|t| t.frame = usize::MAX);
    assert_eq!(
        s.with_native_frame(|_| {
            called = true;
            Ok(5i32)
        }),
        0
    );
    assert!(!called);
    assert!(s.take_pending().is_some());
    s.with_thread(|t| t.frame = 0);
}

#[test]
fn returning_from_native_reclaims_unreturned_locals_without_clearing_the_caller() {
    static WEAK: AtomicU64 = AtomicU64::new(0);
    extern "C" fn temporary(env: JniEnv, _: JObject) {
        let value = env.new_string_utf(c"unreturned-native-temporary");
        WEAK.store(env.new_weak_global_ref(value) as u64, Ordering::Relaxed);
        // Native methods are allowed to leave their local references to the
        // VM's automatic frame cleanup when returning to their caller.
    }
    let _serial = test_support::SERIAL
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    init_with_host(test_support::stub_host());
    let s = session().unwrap();
    s.take_pending();
    // SAFETY: stable shared VM table with exact zero-argument native ABI.
    let env = unsafe { JniEnv::from_raw(abi::env_ptr()) };
    assert_eq!(
        env.register_class(
            c"test/NativeFrameCleanup",
            &[Native::new0(c"temporary", c"()V", temporary),]
        ),
        0
    );
    let class = env.find_class(c"test/NativeFrameCleanup");
    let receiver = env.alloc_object(class);
    let caller = env.new_string_utf(c"caller-owned-local");
    let baseline = s.with_thread(|thread| (thread.frame, thread.locals.len()));
    env.call_void(
        receiver,
        env.method(class, c"temporary", c"()V").unwrap(),
        &[],
    );
    let weak = WEAK.swap(0, Ordering::Relaxed) as JObject;
    assert!(!weak.is_null());
    let dead = env.is_same_object(weak, std::ptr::null_mut());
    env.delete_weak_global_ref(weak);
    assert!(
        dead,
        "native-return temporary owner must die without an explicit DeleteLocalRef"
    );
    assert_eq!(
        s.with_thread(|thread| (thread.frame, thread.locals.len())),
        baseline
    );
    assert_eq!(
        env.with_string_utf(caller, |text| text.to_bytes().len()),
        Some(18)
    );
    assert!(!env.exception_check());
    for local in [caller, receiver, class] {
        env.delete_local_ref(local);
    }
}
