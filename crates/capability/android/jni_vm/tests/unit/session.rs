//! INPUT:  private items of `src/session.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/session.rs`; declared there with `#[path]`
use super::*;
use abi::JNI_TRUE;

const AUDIO: &str = "android/media/AudioManager";

fn local_session() -> Session {
    let session = Session::new();
    session.install_host(test_support::stub_host());
    session
}

fn clear_pending(session: &Session) {
    session.take_pending();
}

fn class_handle(session: &Session, name: &str) -> *mut c_void {
    session.find_class(name).expect("FindClass succeeds")
}

// ---- class resolution -------------------------------------------------

#[test]
fn find_class_registers_and_aliases_the_same_class() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    let a = class_handle(&session, AUDIO);
    let b = class_handle(&session, AUDIO);
    let other = class_handle(&session, "java/lang/String");
    assert_ne!(a, b, "every FindClass hands out a fresh local handle");
    assert_eq!(session.resolve(a), session.resolve(b), "same class identity");
    assert_ne!(session.resolve(a), session.resolve(other));
    // Empty names fail explicitly with a pending exception.
    assert!(session.find_class("").is_none());
    assert!(session.with_thread(|s| s.pending.is_some()));
}

// ---- method resolution: the four explicit-failure classes -------------

#[test]
fn unknown_method_is_null_with_pending_nosuchmethod() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    test_support::stub_set(AUDIO, "getStreamVolume", "(I)I", MethodValue::Int(100));
    let class = class_handle(&session, AUDIO);
    assert!(session.resolve_method(class, "rudroid52_absent", "(I)I", "GetStaticMethodID").is_none());
    let pending = session.with_thread(|s| s.pending.clone()).unwrap();
    assert_eq!(pending.class, "java/lang/NoSuchMethodError");
}

#[test]
fn wrong_signature_is_null_with_pending_nosuchmethod() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    test_support::stub_set(AUDIO, "getStreamVolume", "(I)I", MethodValue::Int(100));
    let class = class_handle(&session, AUDIO);
    // Known method name, mismatching descriptor: same explicit failure
    // class (spec: both are NoSuchMethodError; the v1 registry API has
    // no enumeration to report a finer distinction).
    assert!(session.resolve_method(class, "getStreamVolume", "(I)V", "GetStaticMethodID").is_none());
    assert!(session.with_thread(|s| s.pending.is_some()));
    // The exact key still resolves once the failed lookup's pending
    // exception is cleared.
    clear_pending(&session);
    assert!(session.resolve_method(class, "getStreamVolume", "(I)I", "GetStaticMethodID").is_some());
}

#[test]
fn type_mismatch_fails_instead_of_serving_a_default() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    test_support::stub_set(AUDIO, "isBluetoothA2dpOn", "()Z", MethodValue::Bool(false));
    let class = class_handle(&session, AUDIO);
    let method = session
        .resolve_method(class, "isBluetoothA2dpOn", "()Z", "GetStaticMethodID")
        .expect("bool method resolves");
    // An int call on a bool-valued method: zero result + pending.
    assert!(
        session
            .call_checked(class, method, abi::MethodKind::Int, "CallStaticIntMethodA", false)
            .is_none()
    );
    let pending = session.with_thread(|s| s.pending.clone()).unwrap();
    assert_eq!(pending.class, "java/lang/IllegalArgumentException");
    clear_pending(&session);
    // The matching accessor answers.
    assert_eq!(
        session.call_checked(class, method, abi::MethodKind::Bool, "CallStaticBooleanMethodA", false),
        Some(MethodValue::Bool(false))
    );
    // And the reverse mismatch is equally explicit.
    test_support::stub_set(AUDIO, "getStreamVolume", "(I)I", MethodValue::Int(100));
    let int_method = session
        .resolve_method(class, "getStreamVolume", "(I)I", "GetStaticMethodID")
        .unwrap();
    assert!(
        session
            .call_checked(class, int_method, abi::MethodKind::Bool, "CallStaticBooleanMethodA", false)
            .is_none()
    );
}

#[test]
fn pending_exception_blocks_calls_until_cleared() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    test_support::stub_set(AUDIO, "getStreamVolume", "(I)I", MethodValue::Int(100));
    let class = class_handle(&session, AUDIO);
    let method = session
        .resolve_method(class, "getStreamVolume", "(I)I", "GetStaticMethodID")
        .unwrap();
    session.set_pending("java/lang/Throwable", "probe");
    // Blocked: zero result, no execution, exception stays pending.
    assert_eq!(
        session.call_checked(class, method, abi::MethodKind::Int, "CallStaticIntMethodA", false),
        None
    );
    assert!(session.with_thread(|s| s.pending.is_some()));
    assert!(session.find_class("java/lang/String").is_none(), "FindClass blocked too");
    // Cleared: the same call executes again.
    clear_pending(&session);
    assert_eq!(
        session.call_checked(class, method, abi::MethodKind::Int, "CallStaticIntMethodA", false),
        Some(MethodValue::Int(100))
    );
}

#[test]
fn method_without_catalog_entry_never_resolves() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session(); // no stub entries at all
    let class = class_handle(&session, AUDIO);
    assert!(session.resolve_method(class, "anything", "()V", "GetMethodID").is_none());
    assert!(session.with_thread(|s| s.pending.is_some()));
}

// ---- strings ------------------------------------------------------------

#[test]
fn strings_round_trip_through_the_session_table() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    let handle = session.new_string(b"rudroid52");
    assert!(!handle.is_null());
    assert_eq!(
        session.string_bytes(handle, "probe").as_deref(),
        Some(b"rudroid52".as_slice())
    );
    // An alien handle is an explicit failure, not a zero-length string.
    assert!(session.string_bytes(std::ptr::null_mut(), "probe").is_none());
    assert!(session.with_thread(|s| s.pending.is_some()));
}

// ---- references -----------------------------------------------------------

#[test]
fn global_ref_outlives_the_local_it_came_from() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    test_support::stub_set(AUDIO, "getStreamVolume", "(I)I", MethodValue::Int(100));
    let local = class_handle(&session, AUDIO);
    let global = session.new_ref(session.resolve(local).unwrap(), true);
    // Delete the local, keep using the class through the global ref.
    session.delete_ref(local, false, "DeleteLocalRef");
    let via_global =
        session.resolve_method(global, "getStreamVolume", "(I)I", "GetStaticMethodID");
    assert!(
        via_global.is_some(),
        "the global ref keeps the class resolvable after its local died"
    );
    // A fresh local view of the global aliases the same identity.
    let fresh_local = session.new_ref(session.resolve(global).unwrap(), false);
    assert_eq!(session.resolve(fresh_local), session.resolve(global));
    // Deleting the global leaves the live local untouched.
    session.delete_ref(global, true, "DeleteGlobalRef");
    assert!(session.resolve(fresh_local).is_some());
    assert!(session.resolve(global).is_none(), "the deleted global fails explicitly");
    // Deleting it again is an explicit error, not a silent no-op.
    clear_pending(&session);
    session.delete_ref(global, true, "DeleteGlobalRef");
    assert!(session.with_thread(|s| s.pending.is_some()));
}

// ---- unimplemented slots (global session) ---------------------------------

#[test]
fn unimplemented_slot_fails_with_named_log_and_pending() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    test_support::stub_clear();
    test_support::reset_shared_vm_slot();
    init_with_host(test_support::stub_host());
    // Drive a named thunk exactly like a guest would (env is the first
    // register argument for every env-table entry — pass a live env).
    let returned = unsafe {
        abi::unimplemented::FromReflectedMethod(0x1 as *const std::ffi::c_void)
    };
    assert_eq!(returned, 0, "zero default");
    let session = session().expect("global session initialized");
    let pending = session.with_thread(|s| s.pending.clone()).expect("thunk sets pending");
    assert_eq!(pending.class, "java/lang/InternalError");
    clear_pending(session);
    test_support::stub_clear();
}

// ---- the full guest ABI path through the RAW table slots ------------------

#[test]
fn exception_refs_follow_local_and_global_lifetimes() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    test_support::stub_clear();
    test_support::reset_shared_vm_slot();
    init_with_host(test_support::stub_host());
    let session = session().unwrap();
    clear_pending(session);
    let baseline = session.with_thread(|s| s.locals.len());
    for _ in 0..100 {
        session.set_pending("java/lang/Throwable", "probe");
        let local = shims::exception_occurred(abi::env_ptr());
        let second = shims::exception_occurred(abi::env_ptr());
        assert_eq!(session.resolve(local), session.resolve(second));
        let global = session.new_ref(session.resolve(local).unwrap(), true);
        shims::exception_clear(abi::env_ptr());
        session.delete_ref(local, false, "DeleteLocalRef");
        session.delete_ref(second, false, "DeleteLocalRef");
        assert!(!session.with_thread(|s| s.pending.is_some()));
        assert!(matches!(session.resolve(global), Some(RefTarget::Throwable(_))));
        session.delete_ref(global, true, "DeleteGlobalRef");
        assert_eq!(session.with_thread(|s| s.locals.len()), baseline);
    }
}

#[test]
fn guest_calls_walk_the_raw_function_table() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    use std::ffi::CString;

    test_support::stub_clear();
    test_support::reset_shared_vm_slot();
    init_with_host(test_support::stub_host());
    test_support::stub_set(AUDIO, "getStreamVolume", "(I)I", MethodValue::Int(100));
    test_support::stub_set(AUDIO, "isBluetoothA2dpOn", "()Z", MethodValue::Bool(false));

    let (vm_table, env_table) = crate::abi::tables_for_tests();
    let load = |table: &[usize], index: usize| table[index] as *const ();
    unsafe {
        // GetEnv slot: version negotiation, then the shared env.
        let get_env: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, c_int) -> c_int =
            std::mem::transmute(load(vm_table, abi::vm_slot::GET_ENV));
        let mut env: *mut c_void = std::ptr::null_mut();
        assert_eq!(
            get_env(abi::java_vm_ptr(), &mut env, 0x0001_0008),
            abi::JNI_EVERSION,
            "beyond the supported maximum fails explicitly"
        );
        assert_eq!(
            get_env(abi::java_vm_ptr(), &mut env, abi::JNI_VERSION_1_6),
            abi::JNI_OK
        );
        assert_eq!(env, abi::env_ptr());

        let get_version: unsafe extern "C" fn(*mut c_void) -> c_int =
            std::mem::transmute(load(env_table, abi::env_slot::GetVersion));
        assert_eq!(get_version(env), abi::JNI_VERSION_1_6);

        // FindClass -> GetStaticMethodID -> CallStaticIntMethodA: the
        // catalog fixed reply crosses the raw slot boundary.
        let find_class: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void =
            std::mem::transmute(load(env_table, abi::env_slot::FindClass));
        let class_name = CString::new(AUDIO).unwrap();
        let class = find_class(env, class_name.as_ptr());
        assert!(!class.is_null());

        let get_static: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *const c_char,
            *const c_char,
        ) -> *mut c_void = std::mem::transmute(load(env_table, abi::env_slot::GetStaticMethodID));
        let volume = CString::new("getStreamVolume").unwrap();
        let int_sig = CString::new("(I)I").unwrap();
        let method = get_static(env, class, volume.as_ptr(), int_sig.as_ptr());
        assert!(!method.is_null());

        let call_int: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> c_int =
            std::mem::transmute(load(env_table, abi::env_slot::CallStaticIntMethodA));
        let mut arg: i32 = 3;
        assert_eq!(
            call_int(env, class, method, &mut arg as *mut i32 as *mut c_void),
            100,
            "CallStaticIntMethodA through slot 131 answers the catalog value"
        );

        // The bool method answers through its own accessor.
        let bt = CString::new("isBluetoothA2dpOn").unwrap();
        let bool_sig = CString::new("()Z").unwrap();
        let bool_method = get_static(env, class, bt.as_ptr(), bool_sig.as_ptr());
        assert!(!bool_method.is_null());
        let call_bool: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> u8 =
            std::mem::transmute(load(env_table, abi::env_slot::CallStaticBooleanMethodA));
        let mut zarg: u8 = 0;
        assert_eq!(
            call_bool(env, class, bool_method, &mut zarg as *mut u8 as *mut c_void),
            abi::JNI_FALSE
        );

        // TYPE MISMATCH through the int accessor: zero + pending.
        let exception_check: unsafe extern "C" fn(*mut c_void) -> u8 =
            std::mem::transmute(load(env_table, abi::env_slot::ExceptionCheck));
        assert_eq!(call_int(env, class, bool_method, &mut arg as *mut i32 as *mut c_void), 0);
        assert_eq!(exception_check(env), JNI_TRUE);
        let exception_clear: unsafe extern "C" fn(*mut c_void) =
            std::mem::transmute(load(env_table, abi::env_slot::ExceptionClear));
        exception_clear(env);
        assert_eq!(exception_check(env), abi::JNI_FALSE);

        // Strings: New/Get/Release through the slots.
        let new_string: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void =
            std::mem::transmute(load(env_table, abi::env_slot::NewStringUTF));
        let text = CString::new("rudroid52").unwrap();
        let string = new_string(env, text.as_ptr());
        assert!(!string.is_null());
        let get_chars: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> *const c_char =
            std::mem::transmute(load(env_table, abi::env_slot::GetStringUTFChars));
        let mut is_copy: u8 = 0;
        let chars = get_chars(env, string, &mut is_copy);
        assert_eq!(is_copy, abi::JNI_FALSE);
        assert_eq!(std::ffi::CStr::from_ptr(chars).to_bytes(), b"rudroid52");
        let release: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char) =
            std::mem::transmute(load(env_table, abi::env_slot::ReleaseStringUTFChars));
        release(env, string, chars);

        // GetJavaVM hands back the SAME JavaVM*.
        let get_vm: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> c_int =
            std::mem::transmute(load(env_table, abi::env_slot::GetJavaVM));
        let mut vm2: *mut c_void = std::ptr::null_mut();
        assert_eq!(get_vm(env, &mut vm2), abi::JNI_OK);
        assert_eq!(vm2, abi::java_vm_ptr());

        // An unimplemented slot reached through the raw table: zero +
        // pending, never a fake success.
        let get_object_class: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(load(env_table, abi::env_slot::FromReflectedMethod));
        assert!(get_object_class(env, class).is_null());
        assert_eq!(exception_check(env), JNI_TRUE);
        exception_clear(env);
    }
    test_support::stub_clear();
}
// ---- ticket 88 Phase B: string entity + UTF-chars lifetimes ----------

fn string_entity_count(session: &Session) -> usize {
    session.strings.lock().unwrap().len()
}

fn outstanding_borrows(session: &Session) -> usize {
    session.string_chars.lock().unwrap().len()
}

#[test]
fn string_entity_is_reclaimed_when_its_last_reference_is_deleted() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    let s = session.new_string(b"hello");
    assert_eq!(string_entity_count(&session), 1);
    session.delete_ref(s, false, "DeleteLocalRef");
    assert_eq!(
        string_entity_count(&session),
        0,
        "last reference deleted -> entity reclaimed (no unbounded growth)"
    );
}

#[test]
fn global_reference_keeps_the_string_entity_alive() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    let s = session.new_string(b"shared");
    let target = session.resolve(s).expect("live string ref");
    let g = session.new_ref(target, true);
    session.delete_ref(s, false, "DeleteLocalRef");
    assert_eq!(
        string_entity_count(&session),
        1,
        "a live global reference must keep the entity alive"
    );
    session.delete_ref(g, true, "DeleteGlobalRef");
    assert_eq!(string_entity_count(&session), 0);
}

#[test]
fn utf_chars_borrow_pins_the_entity_and_release_reclaims_both() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    let s = session.new_string(b"borrowed");
    let chars = session.borrow_string_chars(s);
    assert!(!chars.is_null());
    session.delete_ref(s, false, "DeleteLocalRef");
    assert_eq!(
        string_entity_count(&session),
        1,
        "an unreleased chars borrow must NOT let the entity die early"
    );
    assert_eq!(outstanding_borrows(&session), 1);
    session.release_string_chars(chars);
    assert_eq!(outstanding_borrows(&session), 0, "borrow table drained");
    assert_eq!(
        string_entity_count(&session),
        0,
        "release + no references -> entity reclaimed, buffer freed"
    );
}

#[test]
fn repeated_utf_borrows_share_storage_and_require_matching_releases() {
    let session = local_session();
    let string = session.new_string(b"shared bytes");
    let first = session.borrow_string_chars(string);
    let second = session.borrow_string_chars(string);
    assert_eq!(first, second, "borrows share the immutable entity allocation");
    assert!(!first.is_null());
    session.delete_ref(string, false, "DeleteLocalRef");
    session.release_string_chars(first);
    assert_eq!(string_entity_count(&session), 1, "the second borrow still pins data");
    assert_eq!(unsafe { CStr::from_ptr(second).to_bytes() }, b"shared bytes");
    session.release_string_chars(second);
    assert_eq!(string_entity_count(&session), 0);
    assert!(session.liveness.lock().unwrap().is_empty());
    assert_eq!(outstanding_borrows(&session), 0);
}

#[test]
fn mixed_utf_views_release_the_same_owner_only_after_both_encodings_retire() {
    let session=local_session();let string=session.new_string(b"both encodings");
    let bytes=session.borrow_string_chars(string);let units=session.borrow_string_units(string);
    assert!(!bytes.is_null() && !units.is_null());
    assert_eq!(outstanding_borrows(&session),2);
    session.delete_ref(string,false,"test root retirement");
    session.release_string_chars(bytes);
    assert_eq!(string_entity_count(&session),1);
    // SAFETY: the UTF-16 lease still pins the immutable owner's 14 units.
    assert_eq!(unsafe{std::slice::from_raw_parts(units,14)},b"both encodings".iter().map(|&b|u16::from(b)).collect::<Vec<_>>());
    session.release_string_chars(units.cast());
    assert_eq!(string_entity_count(&session),0);assert_eq!(outstanding_borrows(&session),0);
    assert!(session.liveness.lock().unwrap().is_empty());
}

#[test]
fn string_borrow_counter_exhaustion_never_publishes_a_partial_pin() {
    let session=local_session();let string=session.new_string(b"bounded owner");
    let Some(RefTarget::Str(id))=session.resolve(string) else{panic!("string owner required")};
    *session.liveness.lock().unwrap().get_mut(&id).unwrap()=usize::MAX;
    assert!(session.borrow_string_chars(string).is_null());
    assert!(session.string_chars.lock().unwrap().is_empty());
    assert_eq!(session.liveness.lock().unwrap()[&id],usize::MAX);
    session.take_pending();*session.liveness.lock().unwrap().get_mut(&id).unwrap()=1;
    let pointer=session.borrow_string_chars(string);assert!(!pointer.is_null());
    session.string_chars.lock().unwrap().get_mut(&(pointer as usize)).unwrap().1=usize::MAX;
    assert!(session.borrow_string_chars(string).is_null());
    assert_eq!(session.liveness.lock().unwrap()[&id],2);
    session.take_pending();session.string_chars.lock().unwrap().get_mut(&(pointer as usize)).unwrap().1=1;
    session.release_string_chars(pointer);session.delete_ref(string,false,"test cleanup");
    assert!(session.strings.lock().unwrap().is_empty());assert!(session.string_chars.lock().unwrap().is_empty());
    assert!(session.liveness.lock().unwrap().is_empty());
}

#[test]
fn detach_releases_local_pins_without_invalidating_utf_borrows() {
    let session = local_session();
    let string = session.new_string(b"after detach");
    let chars = session.borrow_string_chars(string);
    session.detach_current_thread();
    assert_eq!(string_entity_count(&session), 1);
    assert_eq!(unsafe { CStr::from_ptr(chars).to_bytes() }, b"after detach");
    session.release_string_chars(chars);
    assert_eq!(string_entity_count(&session), 0);
    assert!(session.liveness.lock().unwrap().is_empty());
}

#[test]
fn double_release_and_foreign_pointer_are_misuse_not_crashes() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    let s = session.new_string(b"once");
    let chars = session.borrow_string_chars(s);
    session.release_string_chars(chars);
    let entities = string_entity_count(&session);
    // Double release and an arbitrary pointer are logged misuse: no
    // free of unknown memory, no state change.
    session.release_string_chars(chars);
    let foreign = 0x0000_BAD0_BAD0 as *const std::os::raw::c_char;
    session.release_string_chars(foreign);
    assert_eq!(string_entity_count(&session), entities);
    assert_eq!(outstanding_borrows(&session), 0);
    session.delete_ref(s, false, "DeleteLocalRef");
    assert_eq!(string_entity_count(&session), 0);
}

#[test]
fn repeated_new_get_release_delete_returns_to_steady_state() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    // One keeper string with a global ref survives every cycle.
    let keeper = session.new_string(b"keeper");
    let keeper_global = session.new_ref(session.resolve(keeper).unwrap(), true);
    assert_eq!(string_entity_count(&session), 1);
    for cycle in 0..100 {
        let s = session.new_string(format!("cycle-{cycle}").as_bytes());
        let chars = session.borrow_string_chars(s);
        session.release_string_chars(chars);
        session.delete_ref(s, false, "DeleteLocalRef");
    }
    assert_eq!(
        string_entity_count(&session),
        1,
        "100 full cycles leave exactly the keeper entity"
    );
    assert_eq!(outstanding_borrows(&session), 0);
    session.delete_ref(keeper_global, true, "DeleteGlobalRef");
    // The creating local ref still pins it until its own delete.
    assert_eq!(string_entity_count(&session), 1);
    session.delete_ref(keeper, false, "DeleteLocalRef");
    assert_eq!(string_entity_count(&session), 0);
}

#[test]
fn sparse_reference_tables_release_bursts_while_keeping_live_data() {
    let session = local_session();
    let handles: Vec<_> = (0..16_000).map(|_| session.new_string(b"burst")).collect();
    let borrows: Vec<_> = handles.iter().map(|&handle| session.borrow_string_chars(handle)).collect();
    let globals: Vec<_> = handles.iter().map(|&handle| session.new_ref(session.resolve(handle).unwrap(), true)).collect();
    let before = [session.strings.lock().unwrap().capacity(), session.liveness.lock().unwrap().capacity(),
        session.with_thread(|state| state.locals.capacity()), session.string_chars.lock().unwrap().capacity(),
        session.globals.lock().unwrap().capacity()];
    let (keeper, chars, global) = (handles[0], borrows[0], globals[0]);
    for handle in handles.into_iter().skip(1) { session.delete_ref(handle, false, "DeleteLocalRef"); }
    for chars in borrows.into_iter().skip(1) { session.release_string_chars(chars); }
    for global in globals.into_iter().skip(1) { session.delete_ref(global, true, "DeleteGlobalRef"); }
    let after = [session.strings.lock().unwrap().capacity(), session.liveness.lock().unwrap().capacity(),
        session.with_thread(|state| state.locals.capacity()), session.string_chars.lock().unwrap().capacity(),
        session.globals.lock().unwrap().capacity()];
    eprintln!("JNI sparse burst capacities (strings/pins/locals/UTF/globals): {before:?} -> {after:?}");
    assert_eq!(session.resolve(keeper), session.resolve(global));
    assert_eq!(unsafe { CStr::from_ptr(chars) }.to_bytes(), b"burst", "live UTF borrow stays valid");
    assert!(after.into_iter().all(|capacity| capacity <= 512), "a few surviving references must not retain burst-sized buckets");
    session.delete_ref(keeper, false, "DeleteLocalRef");
    session.delete_ref(global, true, "DeleteGlobalRef");
    assert_eq!(string_entity_count(&session), 1, "UTF borrow still pins the entity");
    session.release_string_chars(chars);
    assert_eq!(string_entity_count(&session), 0);
}

#[test]
fn empty_reference_tables_release_large_bursts_but_reuse_small_buffers() {
    let session = local_session();
    let handles: Vec<_> = (0..16_000).map(|_| session.new_string(b"burst")).collect();
    let strings_before = session.strings.lock().unwrap().capacity();
    let pins_before = session.liveness.lock().unwrap().capacity();
    let locals_before = session.with_thread(|state| state.locals.capacity());
    assert!(strings_before >= 16_000 && pins_before >= 16_000 && locals_before >= 16_000);
    let borrows: Vec<_> = handles.iter().map(|&handle| session.borrow_string_chars(handle)).collect();
    let globals: Vec<_> = handles.iter().map(|&handle| session.new_ref(session.resolve(handle).unwrap(), true)).collect();
    assert!(session.string_chars.lock().unwrap().capacity() >= 16_000);
    assert!(session.globals.lock().unwrap().capacity() >= 16_000);
    for global in globals { session.delete_ref(global, true, "DeleteGlobalRef"); }
    for borrow in borrows { session.release_string_chars(borrow); }
    assert!(session.string_chars.lock().unwrap().capacity() <= 512);
    assert!(session.globals.lock().unwrap().capacity() <= 512);
    for handle in handles { session.delete_ref(handle, false, "DeleteLocalRef"); }
    let strings_after = session.strings.lock().unwrap().capacity();
    let pins_after = session.liveness.lock().unwrap().capacity();
    let locals_after = session.with_thread(|state| state.locals.capacity());
    eprintln!("JNI burst table capacities (strings/pins/locals): {strings_before}/{pins_before}/{locals_before} -> {strings_after}/{pins_after}/{locals_after}");
    assert!(strings_after <= 512 && pins_after <= 512 && locals_after <= 512,
        "empty large tables must not retain their burst capacity for the process lifetime");
    let next = session.new_string(b"reuse");
    session.delete_ref(next, false, "DeleteLocalRef");
    // HashMap capacity counts available slots, including deletion markers;
    // it can change without reallocating buckets. cached_calls measures
    // actual allocations after this burst instead of comparing slot counts.
    assert!(session.strings.lock().unwrap().capacity() <= 512);
    assert!(session.liveness.lock().unwrap().capacity() <= 512);
    assert!(session.with_thread(|state| state.locals.capacity()) <= 512);
}

#[test]
fn detach_reclaims_string_entities_orphaned_by_the_dead_thread() {
    let _serial = test_support::SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let session = local_session();
    let handle = std::thread::spawn(move || {
        let _string = session.new_string(b"thread-local string");
        assert_eq!(string_entity_count(&session), 1);
        session.detach_current_thread();
        assert_eq!(
            string_entity_count(&session),
            0,
            "detach dropped the only reference -> entity reclaimed"
        );
    });
    handle.join().expect("child thread clean");
}

#[test]
fn deleting_the_last_global_on_another_thread_cannot_reclaim_a_temporary_root() {
    let session = std::sync::Arc::new(local_session());
    let local = session.new_string(b"last owner removed");
    let global = session.promote_reference(local, true);
    session.delete_ref(local, false, "test local release");
    let root = session.root_reference(global).unwrap();
    let target = root.target;
    let worker = std::sync::Arc::clone(&session);
    let token = global as usize;
    std::thread::spawn(move || worker.delete_ref(token as *mut c_void, true, "test global release"))
        .join().unwrap();
    assert!(session.resolve(global).is_none());
    assert!(session.target_alive(target), "an in-flight use still owns the payload");
    let retained = session.new_ref(target, false);
    assert_eq!(session.string_bytes(retained, "root test").as_deref(), Some(b"last owner removed".as_slice()));
    drop(root);
    assert!(session.target_alive(target), "the promoted local survives root release");
    session.delete_ref(retained, false, "test retained release");
    assert!(!session.target_alive(target), "the last use reclaims the payload");
}

#[test]
fn owned_direct_buffer_storage_survives_the_last_global_release_during_use() {
    let session = std::sync::Arc::new(local_session());
    let local = session.allocate_direct_buffer(4);
    let global = session.promote_reference(local, true);
    session.delete_ref(local, false, "test local release");
    let root = session.root_reference(global).unwrap();
    let address = session.direct_buffer_address(global).cast::<u8>();
    let worker = std::sync::Arc::clone(&session);
    let token = global as usize;
    std::thread::spawn(move || worker.delete_ref(token as *mut c_void, true, "test global release"))
        .join().unwrap();
    assert!(session.target_alive(root.target));
    // SAFETY: the temporary root retains the VM-owned allocation and no
    // other thread reads or writes these bytes after its owner was deleted.
    unsafe { address.write(42); assert_eq!(address.read(), 42); }
    let target = root.target;
    drop(root);
    assert!(!session.target_alive(target));
}
