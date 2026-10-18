//! INPUT:  private items of `src/session.rs` via `super`
//! OUTPUT:  `frame_and_lifetime_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/session.rs`; declared there with `#[path]`
use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Typed view of one env-table slot.
unsafe fn slot<F: Copy>(index: usize) -> F {
    let (_, env_table) = crate::abi::tables_for_tests();
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
    unsafe { std::mem::transmute_copy(&env_table[index]) }
}

fn global_session() -> &'static Session {
    test_support::stub_clear();
    test_support::reset_shared_vm_slot();
    init_with_host(test_support::stub_host());
    let session = session().expect("global session");
    session.take_pending();
    session
}

fn env() -> *mut c_void {
    abi::env_ptr()
}

fn exception_check() -> bool {
    shims::exception_check(env()) == abi::JNI_TRUE
}

fn new_string(text: &str) -> *mut c_void {
    let text = CString::new(text).unwrap();
    unsafe { shims::new_string_utf(env(), text.as_ptr()) }
}

/// Exercise the actual guest-facing JavaVM table, rather than calling
/// a separate attachment model that could drift from the shims.
unsafe fn vm_slot<F: Copy>(index: usize) -> F {
    let (vm_table, _) = abi::tables_for_tests();
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
    unsafe { std::mem::transmute_copy(&vm_table[index]) }
}

#[test]
fn get_env_queries_attachment_without_attaching_or_leaving_stale_output() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let get_env: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, c_int) -> c_int =
        unsafe { vm_slot(abi::vm_slot::GET_ENV) };
    let detach: unsafe extern "C" fn(*mut c_void) -> c_int =
        unsafe { vm_slot(abi::vm_slot::DETACH_CURRENT_THREAD) };
    let baseline = session.attached_thread_count();

    for attach_slot in [
        abi::vm_slot::ATTACH_CURRENT_THREAD,
        abi::vm_slot::ATTACH_CURRENT_THREAD_AS_DAEMON,
    ] {
        let attach: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut c_void) -> c_int =
            unsafe { vm_slot(attach_slot) };
        std::thread::spawn(move || {
            let vm = abi::java_vm_ptr();
            let mut output = abi::env_ptr();
            assert_eq!(unsafe { get_env(vm, &mut output, abi::JNI_VERSION_1_6) }, abi::JNI_EDETACHED);
            assert!(output.is_null(), "a detached thread cannot reuse a stale JNIEnv");
            assert_eq!(session.attached_thread_count(), baseline, "GetEnv is only a query");

            assert_eq!(unsafe { attach(vm, &mut output, std::ptr::null_mut()) }, abi::JNI_OK);
            assert_eq!(output, abi::env_ptr(), "one shared env facade");
            assert_eq!(session.attached_thread_count(), baseline + 1, "Attach creates thread state");
            assert_eq!(unsafe { attach(vm, &mut output, std::ptr::null_mut()) }, abi::JNI_OK);
            assert_eq!(session.attached_thread_count(), baseline + 1, "repeat Attach is idempotent");
            output = std::ptr::null_mut();
            assert_eq!(unsafe { get_env(vm, &mut output, abi::JNI_VERSION_1_6) }, abi::JNI_OK);
            assert_eq!(output, abi::env_ptr());

            assert_eq!(unsafe { detach(vm) }, abi::JNI_OK);
            assert_eq!(unsafe { get_env(vm, &mut output, abi::JNI_VERSION_1_6) }, abi::JNI_EDETACHED);
            assert!(output.is_null());
            assert_eq!(session.attached_thread_count(), baseline);
            assert_eq!(unsafe { detach(vm) }, abi::JNI_OK, "detaching twice is harmless");
        }).join().unwrap();
    }

    let mut output = std::ptr::null_mut();
    assert_eq!(unsafe { get_env(abi::java_vm_ptr(), &mut output, abi::JNI_VERSION_1_6) }, abi::JNI_OK);
    assert_eq!(output, abi::env_ptr(), "the initializing thread remains attached");
}

#[test]
fn get_env_accepts_only_supported_versions_and_clears_failed_output() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let get_env: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, c_int) -> c_int =
        unsafe { vm_slot(abi::vm_slot::GET_ENV) };
    let baseline = session.attached_thread_count();
    for version in [abi::JNI_VERSION_1_1, abi::JNI_VERSION_1_2, abi::JNI_VERSION_1_4, abi::JNI_VERSION_1_6] {
        let mut output = std::ptr::null_mut();
        assert_eq!(unsafe { get_env(abi::java_vm_ptr(), &mut output, version) }, abi::JNI_OK);
        assert_eq!(output, abi::env_ptr());
    }
    for version in [0, -1, 0x0001_0003, 0x0001_0005, 0x0001_0008] {
        let mut output = abi::env_ptr();
        assert_eq!(unsafe { get_env(abi::java_vm_ptr(), &mut output, version) }, abi::JNI_EVERSION);
        assert!(output.is_null(), "unsupported versions clear the output");
    }
    assert_eq!(unsafe { get_env(abi::java_vm_ptr(), std::ptr::null_mut(), abi::JNI_VERSION_1_6) }, abi::JNI_ERR);
    assert_eq!(session.attached_thread_count(), baseline);
}

#[test]
fn one_threads_local_operation_does_not_block_another_threads_state() {
    let session = Arc::new(Session::new());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let held = Arc::clone(&session);
    let holder = std::thread::spawn(move || {
        held.with_thread(|_| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        held.detach_current_thread();
    });
    started_rx.recv().unwrap();
    let other = Arc::clone(&session);
    let reader = std::thread::spawn(move || {
        let clean = other.with_thread(|state| state.pending.is_none());
        done_tx.send(clean).unwrap();
        other.detach_current_thread();
    });
    let result = done_rx.recv_timeout(std::time::Duration::from_secs(2));
    // Release and join even on red, so a regression never hangs cargo.
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    reader.join().unwrap();
    assert_eq!(result.ok(), Some(true), "independent thread state must make progress");
    assert_eq!(session.attached_thread_count(), 0);
}

#[test]
fn tls_cache_shares_the_record_and_detach_releases_it() {
    let session = Session::new();
    session.with_thread(|_| assert!(session.current_thread_is_attached()));
    let key = Session::thread_key().unwrap();
    let weak = THREAD_KEY.with(|thread| {
        let cached = thread.1.borrow();
        let record = &cached.as_ref().unwrap().state;
        assert!(Arc::ptr_eq(record, session.threads.lock().unwrap().get(&key).unwrap()));
        Arc::downgrade(record)
    });
    session.detach_current_thread();
    assert!(!session.current_thread_is_attached());
    assert!(weak.upgrade().is_none(), "neither registry nor TLS retains detached state");
}

#[test]
fn tls_cache_keeps_distinct_sessions_states_separate() {
    let first = Session::new();
    let second = Session::new();
    first.set_pending("first", "one");
    assert!(second.current_pending().is_none());
    second.set_pending("second", "two");
    assert_eq!(first.current_pending().unwrap().message, "one");
    assert_eq!(second.current_pending().unwrap().message, "two");
    first.detach_current_thread();
    let key = second.threads.lock().unwrap().keys().copied().next().unwrap();
    second.detach_key(key);
}

#[test]
fn throw_keeps_the_throwable_identity() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    session.set_pending("java/lang/IllegalStateException", "boom");
    let exc = shims::exception_occurred(env());
    assert!(!exc.is_null());
    shims::exception_clear(env());
    assert_eq!(shims::throw(env(), exc), abi::JNI_OK);
    let again = shims::exception_occurred(env());
    assert_eq!(
        shims::is_same_object(env(), again, exc),
        abi::JNI_TRUE,
        "the rethrown exception is the same object"
    );
    let pending = session.current_pending().expect("rethrown");
    assert_eq!(pending.class, "java/lang/IllegalStateException");
    assert_eq!(pending.message, "boom");
    shims::exception_clear(env());
    session.delete_ref(again, false, "DeleteLocalRef");
    session.delete_ref(exc, false, "DeleteLocalRef");
    // Throwing a non-throwable reference fails without raising anything.
    let class = session.find_class("java/lang/String").unwrap();
    assert_eq!(shims::throw(env(), class), abi::JNI_ERR);
    assert!(!exception_check());
    session.delete_ref(class, false, "DeleteLocalRef");
}

#[test]
fn local_frames_free_their_locals_and_keep_the_result() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let push: unsafe extern "C" fn(*mut c_void, c_int) -> c_int =
        unsafe { slot(abi::env_slot::PushLocalFrame) };
    let pop: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        unsafe { slot(abi::env_slot::PopLocalFrame) };
    let ensure: unsafe extern "C" fn(*mut c_void, c_int) -> c_int =
        unsafe { slot(abi::env_slot::EnsureLocalCapacity) };
    let baseline_locals = session.with_thread(|s| s.locals.len());

    assert_eq!(unsafe { push(env(), 4) }, abi::JNI_OK);
    assert!(!exception_check(), "PushLocalFrame succeeds without raising");
    let temporary = new_string("temporary");
    let kept = new_string("kept");
    let Some(RefTarget::Str(temporary_id)) = session.resolve(temporary) else { panic!() };
    let Some(RefTarget::Str(kept_id)) = session.resolve(kept) else { panic!() };
    // Nested frame: its local dies with it, the outer frame survives.
    assert_eq!(unsafe { push(env(), 2) }, abi::JNI_OK);
    let inner = new_string("inner");
    assert!(unsafe { pop(env(), std::ptr::null_mut()) }.is_null());
    assert!(session.resolve(inner).is_none(), "inner frame local freed");
    assert!(session.resolve(temporary).is_some(), "outer frame untouched");

    let result = unsafe { pop(env(), kept) };
    assert!(!result.is_null());
    assert!(session.resolve(temporary).is_none(), "frame local freed");
    assert!(session.resolve(kept).is_none(), "the popped handle itself is gone");
    assert_eq!(
        session.string_bytes(result, "probe").as_deref(),
        Some(b"kept".as_slice()),
        "the result is re-registered in the outer frame"
    );
    assert_eq!(session.with_thread(|s| s.locals.len()), baseline_locals + 1);
    assert!(!session.strings.lock().unwrap().contains_key(&temporary_id), "entity reclaimed");
    session.delete_ref(result, false, "DeleteLocalRef");
    assert!(!session.strings.lock().unwrap().contains_key(&kept_id));

    assert_eq!(unsafe { ensure(env(), 16) }, abi::JNI_OK);
    assert_eq!(unsafe { push(env(), -1) }, abi::JNI_ERR, "negative capacity fails");
    assert!(exception_check());
    shims::exception_clear(env());
}

#[test]
fn pop_without_push_clears_the_base_frame() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let pop: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        unsafe { slot(abi::env_slot::PopLocalFrame) };
    std::thread::spawn(move || {
        let local = new_string("base");
        assert!(unsafe { pop(env(), std::ptr::null_mut()) }.is_null());
        assert!(session.resolve(local).is_none(), "reference clears the base frame");
        assert!(!exception_check());
        session.detach_current_thread();
    })
    .join()
    .unwrap();
}

#[test]
fn monitors_are_reentrant_owner_checked_and_exclusive() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let enter: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int =
        unsafe { slot(abi::env_slot::MonitorEnter) };
    let exit: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int =
        unsafe { slot(abi::env_slot::MonitorExit) };
    let class = session.find_class("rudroid/Monitor").unwrap();
    assert_eq!(unsafe { enter(env(), class) }, abi::JNI_OK);
    assert_eq!(unsafe { enter(env(), class) }, abi::JNI_OK, "reentrant");
    assert!(!exception_check());

    // Another thread blocks until the owner fully exits.
    let acquired = Arc::new(AtomicBool::new(false));
    let global = session.new_ref(session.resolve(class).unwrap(), true) as usize;
    let flag = Arc::clone(&acquired);
    let waiter = std::thread::spawn(move || {
        let global = global as *mut c_void;
        assert_eq!(unsafe { enter(env(), global) }, abi::JNI_OK);
        flag.store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(unsafe { exit(env(), global) }, abi::JNI_OK);
        session.detach_current_thread();
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(unsafe { exit(env(), class) }, abi::JNI_OK);
    assert!(!acquired.load(std::sync::atomic::Ordering::Acquire), "still held once");
    assert_eq!(unsafe { exit(env(), class) }, abi::JNI_OK);
    waiter.join().unwrap();
    assert!(acquired.load(std::sync::atomic::Ordering::Acquire));

    // Exiting a monitor this thread does not own is an explicit error.
    assert_eq!(unsafe { exit(env(), class) }, abi::JNI_ERR);
    assert!(exception_check());
    shims::exception_clear(env());
    session.delete_ref(global as *mut c_void, true, "DeleteGlobalRef");
    session.delete_ref(class, false, "DeleteLocalRef");
}

#[repr(C)]
struct NativeMethod {
    name: *const c_char,
    signature: *const c_char,
    fn_ptr: *mut c_void,
}

unsafe extern "C" fn native_a() {}
unsafe extern "C" fn native_b() {}

#[test]
fn register_natives_records_the_bindings() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let register: unsafe extern "C" fn(*mut c_void, *mut c_void, *const NativeMethod, c_int) -> c_int =
        unsafe { slot(abi::env_slot::RegisterNatives) };
    let unregister: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int =
        unsafe { slot(abi::env_slot::UnregisterNatives) };
    let class = session.find_class("com/unity3d/player/UnityPlayer").unwrap();
    let methods = [
        NativeMethod {
            name: c"nativeRender".as_ptr(),
            signature: c"()Z".as_ptr(),
            fn_ptr: native_a as *mut c_void,
        },
        NativeMethod {
            name: c"nativeResume".as_ptr(),
            signature: c"()V".as_ptr(),
            fn_ptr: native_b as *mut c_void,
        },
    ];
    assert_eq!(unsafe { register(env(), class, methods.as_ptr(), 2) }, abi::JNI_OK);
    assert!(!exception_check(), "RegisterNatives raises nothing");
    // Reference parity: a null class is logged and answers JNI_OK.
    assert_eq!(unsafe { register(env(), std::ptr::null_mut(), methods.as_ptr(), 2) }, abi::JNI_OK);
    assert_eq!(unsafe { register(env(), class, methods.as_ptr(), -1) }, abi::JNI_ERR);
    assert_eq!(unsafe { unregister(env(), class) }, abi::JNI_OK);
    assert!(!exception_check());
    session.delete_ref(class, false, "DeleteLocalRef");
}

#[test]
fn status_returning_slots_never_fall_back_to_the_zero_thunk() {
    // An unimplemented thunk answers 0, which for these entries would
    // read as JNI_OK: every one of them must be a real implementation.
    for name in [
        "Throw",
        "ThrowNew",
        "PushLocalFrame",
        "EnsureLocalCapacity",
        "RegisterNatives",
        "UnregisterNatives",
        "MonitorEnter",
        "MonitorExit",
        "GetJavaVM",
    ] {
        let index = abi::ENV_SLOT_NAMES.iter().position(|n| *n == name).unwrap();
        assert!(abi::ENV_IMPLEMENTED.contains(&index), "{name} must be implemented");
    }
}

#[test]
fn registered_natives_are_queryable_and_unregistered_per_class() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let class = session.find_class("rudroid/Natives").unwrap();
    let methods = [NativeMethod {
        name: c"nativeTick".as_ptr(),
        signature: c"(J)V".as_ptr(),
        fn_ptr: native_a as *mut c_void,
    }];
    let methods_ptr = methods.as_ptr() as *const JniNativeMethod;
    assert_eq!(unsafe { shims::register_natives(env(), class, methods_ptr, 1) }, abi::JNI_OK);
    assert_eq!(
        session.registered_native("rudroid/Natives", "nativeTick", "(J)V"),
        Some(native_a as usize)
    );
    assert_eq!(session.registered_native("rudroid/Natives", "nativeTick", "()V"), None);
    assert_eq!(shims::unregister_natives(env(), class), abi::JNI_OK);
    assert_eq!(session.registered_native("rudroid/Natives", "nativeTick", "(J)V"), None);
    session.delete_ref(class, false, "DeleteLocalRef");
}

#[test]
fn native_static_calls_reenter_replace_and_revoke_cached_method_ids() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session=global_session();
    unsafe extern "C" fn false_body(_env:*mut c_void,_class:*mut c_void,_string:*mut c_void)->u8 { 0 }
    unsafe extern "C" fn reenter_body(env:*mut c_void,class:*mut c_void,string:*mut c_void)->u8 {
        let chars=shims::get_string_utf_chars(env,string,std::ptr::null_mut());
        assert_eq!(CStr::from_ptr(chars),c"native-reentry");
        shims::release_string_utf_chars(env,string,chars);
        let binding=JniNativeMethod { name:c"load".as_ptr(),signature:c"(Ljava/lang/String;)Z".as_ptr(),fn_ptr:false_body as *mut c_void };
        assert_eq!(shims::register_natives(env,class,&binding,1),abi::JNI_OK);
        1
    }
    unsafe extern "C" fn no_args_body(_env:*mut c_void,_class:*mut c_void)->u8 { 1 }
    let class=session.find_class("native/BootstrapProbe").unwrap();
    let string=session.new_string(b"native-reentry");
    let binding=JniNativeMethod { name:c"load".as_ptr(),signature:c"(Ljava/lang/String;)Z".as_ptr(),fn_ptr:reenter_body as *mut c_void };
    unsafe {
        assert_eq!(shims::register_natives(env(),class,&binding,1),abi::JNI_OK);
        let method=shims::get_static_method_id(env(),class,binding.name,binding.signature);
        assert!(!method.is_null());
        let mut args=[string];
        assert_eq!(shims::call_static_boolean_method_a(env(),class,method,args.as_mut_ptr().cast()),1);
        assert_eq!(shims::get_static_method_id(env(),class,binding.name,binding.signature),method);
        assert_eq!(shims::call_static_boolean_method_a(env(),class,method,args.as_mut_ptr().cast()),0);
        assert!(!exception_check());
        // Wrong return accessor must never enter the native body.
        assert_eq!(shims::call_static_int_method_a(env(),class,method,args.as_mut_ptr().cast()),0);
        assert!(exception_check()); shims::exception_clear(env());
        assert_eq!(shims::unregister_natives(env(),class),abi::JNI_OK);
        assert_eq!(shims::call_static_boolean_method_a(env(),class,method,args.as_mut_ptr().cast()),0);
        assert!(exception_check()); shims::exception_clear(env());
        assert_eq!(shims::register_natives(env(),class,&binding,1),abi::JNI_OK);
        assert_eq!(shims::get_static_method_id(env(),class,binding.name,binding.signature),method);
        assert_eq!(shims::call_static_boolean_method_a(env(),class,method,std::ptr::null_mut()),0);
        assert!(exception_check()); shims::exception_clear(env());
        let plain=JniNativeMethod { name:c"ready".as_ptr(),signature:c"()Z".as_ptr(),fn_ptr:no_args_body as *mut c_void };
        assert_eq!(shims::register_natives(env(),class,&plain,1),abi::JNI_OK);
        let ready=shims::get_static_method_id(env(),class,plain.name,plain.signature);
        assert_eq!(shims::call_static_boolean_method_a(env(),class,ready,std::ptr::null_mut()),1);
        let null_binding=JniNativeMethod { name:c"load".as_ptr(),signature:binding.signature,fn_ptr:std::ptr::null_mut() };
        let replacement=JniNativeMethod { name:binding.name,signature:binding.signature,fn_ptr:false_body as *mut c_void };
        let invalid=[replacement,null_binding];
        assert_eq!(shims::register_natives(env(),class,invalid.as_ptr(),2),abi::JNI_ERR);
        shims::exception_clear(env());
        assert_eq!(session.registered_native("native/BootstrapProbe","load","(Ljava/lang/String;)Z"),Some(reenter_body as usize));
        extern "C" fn unsupported_body(_: *mut c_void, _: *mut c_void, _: i32, _: i32) -> u8 { 1 }
        let unsupported=JniNativeMethod { name:c"unknownShape".as_ptr(),signature:c"(II)Z".as_ptr(),fn_ptr:unsupported_body as *mut c_void };
        assert_eq!(shims::register_natives(env(),class,&unsupported,1),abi::JNI_OK);
        let unsupported_id=shims::get_static_method_id(env(),class,unsupported.name,unsupported.signature);
        assert!(!unsupported_id.is_null());
        assert_eq!(shims::call_static_boolean_method_a(env(),class,unsupported_id,args.as_mut_ptr().cast()),0);
        assert!(exception_check()); shims::exception_clear(env());
        assert_eq!(shims::unregister_natives(env(),class),abi::JNI_OK);
    }
    session.delete_ref(class,false,"DeleteLocalRef");
    session.delete_ref(string,false,"DeleteLocalRef");
}

#[test]
fn rethrown_throwable_metadata_is_reclaimed_with_its_last_reference() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    for _ in 0..50 {
        session.set_pending("java/lang/RuntimeException", "cycle");
        let exc = shims::exception_occurred(env());
        let Some(RefTarget::Throwable(id)) = session.resolve(exc) else { panic!() };
        shims::exception_clear(env());
        assert_eq!(shims::throw(env(), exc), abi::JNI_OK);
        session.delete_ref(exc, false, "DeleteLocalRef");
        assert!(session.throwables.lock().unwrap().contains_key(&id), "still pending");
        shims::exception_clear(env());
        assert!(!session.throwables.lock().unwrap().contains_key(&id), "reclaimed");
    }
}

#[test]
fn pending_throwable_pins_are_shared_and_rethrowing_balances_ownership() {
    let session = Arc::new(Session::new());
    session.set_pending("java/lang/RuntimeException", "shared");
    let pending = session.current_pending().unwrap();
    let weak = Arc::downgrade(&pending);
    let id = pending.id;
    let local = session.exception_occurred();
    assert!(Arc::ptr_eq(&pending, &session.throwables.lock().unwrap()[&id]), "observed and pending states own the same immutable entity");
    drop(pending);
    let target = session.resolve(local).unwrap();
    let global = session.new_ref(target, true) as usize;
    session.delete_ref(local, false, "DeleteLocalRef");
    let worker_session = Arc::clone(&session);
    std::thread::spawn(move || {
        for _ in 0..10 {
            assert_eq!(worker_session.throw(global as *mut c_void), abi::JNI_OK);
            assert!(Arc::ptr_eq(&worker_session.current_pending().unwrap(), &worker_session.throwables.lock().unwrap()[&id]), "cross-thread rethrow shares the original payload");
        }
        worker_session.detach_current_thread();
    }).join().unwrap();
    session.delete_ref(global as *mut c_void, true, "DeleteGlobalRef");
    assert_eq!(session.throwables.lock().unwrap().len(), 1, "main thread still pending");
    session.take_pending();
    assert!(session.throwables.lock().unwrap().is_empty());
    assert!(session.liveness.lock().unwrap().is_empty());
    assert!(weak.upgrade().is_none(), "the final JNI pin and pending owner release the entity");
}

#[test]
fn exception_burst_releases_metadata_capacity_with_pending_survivor() {
    let session = Session::new();
    let exceptions: Vec<_> = (0..16_000).map(|_| {
        session.set_pending("java/lang/RuntimeException", "temporary");
        session.exception_occurred()
    }).collect();
    assert!(session.throwables.lock().unwrap().capacity() >= 16_000);
    for exception in exceptions { session.delete_ref(exception, false, "DeleteLocalRef"); }
    assert_eq!(session.throwables.lock().unwrap().len(), 1, "pending exception still lives");
    assert!(session.throwables.lock().unwrap().capacity() <= 512, "a pending exception must not retain burst-sized metadata buckets");
    session.take_pending();
    assert!(session.throwables.lock().unwrap().is_empty());
    assert!(session.throwables.lock().unwrap().capacity() <= 512);
}

#[test]
fn exhausted_serials_fail_instead_of_aliasing_live_ids() {
    let session = Session::new();
    session.install_host(test_support::stub_host());
    let first = session.new_string(b"first");
    session.serial.store(SERIAL_LIMIT - 2, Ordering::Relaxed);
    // The last two serials still work: one entity + its local handle.
    let last = session.new_string(b"last");
    assert!(!last.is_null());
    let (_, serial) = handle_parts(last);
    assert_eq!(serial, SERIAL_LIMIT - 1, "the top serial stays inside the handle payload");
    // Exhausted: explicit failure, nothing overwritten.
    assert!(session.new_string(b"overflow").is_null());
    assert_eq!(
        session.current_pending().map(|p| p.class.clone()).as_deref(),
        Some("java/lang/OutOfMemoryError")
    );
    session.take_pending();
    assert!(session.find_class("rudroid/Late").is_none());
    assert_eq!(session.string_bytes(first, "probe").as_deref(), Some(b"first".as_slice()));
    assert_eq!(session.string_bytes(last, "probe").as_deref(), Some(b"last".as_slice()));
    assert_eq!(session.strings.lock().unwrap().len(), 2);
}

fn armed<R>(f: impl FnOnce() -> R) -> R {
    SWEEP_PROBE.with(|armed| armed.set(true));
    let result = f();
    SWEEP_PROBE.with(|armed| armed.set(false));
    result
}

#[test]
fn new_string_survives_a_sweep_inside_its_creation() {
    let session = Session::new();
    let handle = armed(|| session.new_string(b"fresh"));
    assert_eq!(
        session.string_bytes(handle, "probe").as_deref(),
        Some(b"fresh".as_slice()),
        "a thread exiting mid-NewStringUTF must not reclaim the new entity"
    );
}

#[test]
fn pop_local_frame_result_survives_a_sweep_inside_the_pop() {
    let session = Session::new();
    assert_eq!(session.push_local_frame(2), abi::JNI_OK);
    let inner = session.new_string(b"result");
    let result = armed(|| session.pop_local_frame(inner));
    assert_eq!(
        session.string_bytes(result, "probe").as_deref(),
        Some(b"result".as_slice()),
        "a thread exiting mid-PopLocalFrame must not reclaim the result"
    );
}

#[test]
fn a_second_exception_has_its_own_identity_and_type() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    session.set_pending("java/lang/IllegalStateException", "first");
    let first = shims::exception_occurred(env());
    // A new exception raised while the first is still pending.
    session.set_pending("java/lang/ArithmeticException", "second");
    let second = shims::exception_occurred(env());
    assert_eq!(shims::is_same_object(env(), first, second), abi::JNI_FALSE);
    shims::exception_clear(env());
    assert_eq!(shims::throw(env(), second), abi::JNI_OK);
    let pending = session.current_pending().unwrap();
    assert_eq!((pending.class.as_str(), pending.message.as_str()), ("java/lang/ArithmeticException", "second"));
    shims::exception_clear(env());
    assert_eq!(shims::throw(env(), first), abi::JNI_OK);
    let pending = session.current_pending().unwrap();
    assert_eq!((pending.class.as_str(), pending.message.as_str()), ("java/lang/IllegalStateException", "first"));
    shims::exception_clear(env());
    session.delete_ref(first, false, "DeleteLocalRef");
    session.delete_ref(second, false, "DeleteLocalRef");
}

#[test]
fn exited_thread_without_detach_is_cleaned_up() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = global_session();
    let (key, string_id) = std::thread::spawn(move || {
        // Auto-attach by use; the thread exits WITHOUT DetachCurrentThread.
        let handle = new_string("orphan");
        let Some(RefTarget::Str(id)) = session.resolve(handle) else { panic!() };
        (Session::thread_key().unwrap(), id)
    })
    .join()
    .unwrap();
    assert!(
        !session.threads.lock().unwrap().contains_key(&key),
        "thread record dropped"
    );
    assert!(
        !session.strings.lock().unwrap().contains_key(&string_id),
        "the exited thread's locals no longer pin their entities"
    );
}
