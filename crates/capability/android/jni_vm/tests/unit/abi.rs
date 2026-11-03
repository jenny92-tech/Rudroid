//! INPUT:  private items of `src/abi.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/abi.rs`; declared there with `#[path]`
use super::*;

#[test]
fn jni_handles_point_to_tables_instead_of_reserved_slots() {
    unsafe {
        assert_eq!(*(java_vm_ptr() as *const *const usize), invoke_table().as_ptr());
        assert_eq!(*(env_ptr() as *const *const usize), native_interface().as_ptr());
    }
}

/// The transcription against the spec positions: a drifted slot index
/// moves a guest's call to the wrong function, so the well-known JNI
/// indices are pinned here.
#[test]
fn slot_indices_match_the_jni_spec_positions() {
    assert_eq!(env_slot::GetVersion, 4);
    assert_eq!(env_slot::DefineClass, 5);
    assert_eq!(env_slot::FindClass, 6);
    assert_eq!(env_slot::ThrowNew, 14);
    assert_eq!(env_slot::ExceptionClear, 17);
    assert_eq!(env_slot::NewGlobalRef, 21);
    assert_eq!(env_slot::GetMethodID, 33);
    assert_eq!(env_slot::GetStaticMethodID, 113);
    assert_eq!(env_slot::CallStaticBooleanMethodA, 119);
    assert_eq!(env_slot::CallStaticIntMethodA, 131);
    assert_eq!(env_slot::NewStringUTF, 167);
    assert_eq!(env_slot::GetStringUTFLength, 168);
    assert_eq!(env_slot::GetStringUTFChars, 169);
    assert_eq!(env_slot::ReleaseStringUTFChars, 170);
    assert_eq!(env_slot::GetJavaVM, 219);
    assert_eq!(env_slot::ExceptionCheck, 228);
    assert_eq!(env_slot::GetObjectRefType, 232);
    assert_eq!(ENV_SLOT_COUNT, 233);
    assert_eq!(ENV_SLOT_NAMES.len(), ENV_SLOT_COUNT);
    assert_eq!(ENV_SLOT_NAMES[env_slot::GetStringUTFChars], "GetStringUTFChars");
    assert_eq!(ENV_SLOT_NAMES[env_slot::ExceptionCheck], "ExceptionCheck");
}

#[test]
fn the_assembled_tables_are_complete_and_never_null() {
    let (vm_table, env_table) = tables_for_tests();
    assert_eq!(vm_table.len(), VM_SLOT_COUNT);
    // Reserved slots stay null; the five functions are real.
    assert_eq!(&vm_table[..3], &[0; 3]);
    for slot in &vm_table[3..] {
        assert_ne!(*slot, 0, "VM table function slots must all be implemented");
    }
    // The four reserved env slots stay null; EVERY other slot holds a
    // real entry (implemented or named explicit-failure thunk).
    assert_eq!(&env_table[..4], &[0; 4]);
    for (index, slot) in env_table.iter().enumerate().skip(4) {
        assert_ne!(*slot, 0, "slot {index} ({}) left null", ENV_SLOT_NAMES[index]);
    }
}

#[test]
fn the_implemented_set_is_the_documented_minimum() {
    let native_varargs = cfg!(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"));
    assert_eq!(ENV_IMPLEMENTED.len(), if native_varargs { 228 } else { 166 });
    for (expected, index) in [
        ("GetVersion", 4usize),
        ("FindClass", 6),
        ("ExceptionCheck", 228),
        ("GetJavaVM", 219),
    ] {
        assert_eq!(ENV_SLOT_NAMES[*ENV_IMPLEMENTED.iter().find(|i| **i == index).unwrap()], expected);
    }
    // No duplicates: every implemented slot listed exactly once.
    let mut sorted = ENV_IMPLEMENTED.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ENV_IMPLEMENTED.len());
}

/// common's `jni_env::slot` table is what every other capability calls
/// through; it must name the same entries as this authoritative table.
#[test]
fn common_jni_env_slots_match_this_table() {
    use rudroid_common::jni_env::slot::env as common;
    for (common_index, own_index) in [
        (common::GET_VERSION, env_slot::GetVersion),
        (common::DEFINE_CLASS, env_slot::DefineClass),
        (common::FIND_CLASS, env_slot::FindClass),
        (common::IS_ASSIGNABLE_FROM, env_slot::IsAssignableFrom),
        (common::TO_REFLECTED_METHOD, env_slot::ToReflectedMethod),
        (common::THROW, env_slot::Throw),
        (common::THROW_NEW, env_slot::ThrowNew),
        (common::EXCEPTION_OCCURRED, env_slot::ExceptionOccurred),
        (common::EXCEPTION_DESCRIBE, env_slot::ExceptionDescribe),
        (common::EXCEPTION_CLEAR, env_slot::ExceptionClear),
        (common::PUSH_LOCAL_FRAME, env_slot::PushLocalFrame),
        (common::POP_LOCAL_FRAME, env_slot::PopLocalFrame),
        (common::NEW_GLOBAL_REF, env_slot::NewGlobalRef),
        (common::DELETE_GLOBAL_REF, env_slot::DeleteGlobalRef),
        (common::DELETE_LOCAL_REF, env_slot::DeleteLocalRef),
        (common::IS_SAME_OBJECT, env_slot::IsSameObject),
        (common::NEW_LOCAL_REF, env_slot::NewLocalRef),
        (common::ALLOC_OBJECT, env_slot::AllocObject),
        (common::NEW_OBJECT_A, env_slot::NewObjectA),
        (common::GET_OBJECT_CLASS, env_slot::GetObjectClass),
        (common::IS_INSTANCE_OF, env_slot::IsInstanceOf),
        (common::GET_METHOD_ID, env_slot::GetMethodID),
        (common::CALL_OBJECT_METHOD_A, env_slot::CallObjectMethodA),
        (common::CALL_BOOLEAN_METHOD_A, env_slot::CallBooleanMethodA),
        (common::CALL_INT_METHOD_A, env_slot::CallIntMethodA),
        (common::CALL_VOID_METHOD_A, env_slot::CallVoidMethodA),
        (common::GET_FIELD_ID, env_slot::GetFieldID),
        (common::GET_OBJECT_FIELD, env_slot::GetObjectField),
        (common::GET_BOOLEAN_FIELD, env_slot::GetBooleanField),
        (common::GET_INT_FIELD, env_slot::GetIntField),
        (common::GET_LONG_FIELD, env_slot::GetLongField),
        (common::GET_FLOAT_FIELD, env_slot::GetFloatField),
        (common::SET_OBJECT_FIELD, env_slot::SetObjectField),
        (common::SET_BOOLEAN_FIELD, env_slot::SetBooleanField),
        (common::SET_INT_FIELD, env_slot::SetIntField),
        (common::SET_LONG_FIELD, env_slot::SetLongField),
        (common::SET_FLOAT_FIELD, env_slot::SetFloatField),
        (common::GET_STATIC_METHOD_ID, env_slot::GetStaticMethodID),
        (common::CALL_STATIC_OBJECT_METHOD_A, env_slot::CallStaticObjectMethodA),
        (common::CALL_STATIC_BOOLEAN_METHOD_A, env_slot::CallStaticBooleanMethodA),
        (common::CALL_STATIC_INT_METHOD_A, env_slot::CallStaticIntMethodA),
        (common::CALL_STATIC_VOID_METHOD_A, env_slot::CallStaticVoidMethodA),
        (common::GET_STATIC_FIELD_ID, env_slot::GetStaticFieldID),
        (common::GET_STATIC_OBJECT_FIELD, env_slot::GetStaticObjectField),
        (common::SET_STATIC_OBJECT_FIELD, env_slot::SetStaticObjectField),
        (common::SET_STATIC_INT_FIELD, env_slot::SetStaticIntField),
        (common::NEW_STRING, env_slot::NewString),
        (common::GET_STRING_LENGTH, env_slot::GetStringLength),
        (common::GET_STRING_CHARS, env_slot::GetStringChars),
        (common::RELEASE_STRING_CHARS, env_slot::ReleaseStringChars),
        (common::NEW_STRING_UTF, env_slot::NewStringUTF),
        (common::GET_STRING_UTF_CHARS, env_slot::GetStringUTFChars),
        (common::RELEASE_STRING_UTF_CHARS, env_slot::ReleaseStringUTFChars),
        (common::GET_ARRAY_LENGTH, env_slot::GetArrayLength),
        (common::NEW_OBJECT_ARRAY, env_slot::NewObjectArray),
        (common::GET_OBJECT_ARRAY_ELEMENT, env_slot::GetObjectArrayElement),
        (common::SET_OBJECT_ARRAY_ELEMENT, env_slot::SetObjectArrayElement),
        (common::NEW_INT_ARRAY, env_slot::NewIntArray),
        (common::GET_INT_ARRAY_REGION, env_slot::GetIntArrayRegion),
        (common::SET_INT_ARRAY_REGION, env_slot::SetIntArrayRegion),
        (common::REGISTER_NATIVES, env_slot::RegisterNatives),
        (common::NEW_WEAK_GLOBAL_REF, env_slot::NewWeakGlobalRef),
        (common::DELETE_WEAK_GLOBAL_REF, env_slot::DeleteWeakGlobalRef),
        (common::EXCEPTION_CHECK, env_slot::ExceptionCheck),
        (common::GET_DIRECT_BUFFER_ADDRESS, env_slot::GetDirectBufferAddress),
        (common::GET_DIRECT_BUFFER_CAPACITY, env_slot::GetDirectBufferCapacity),
    ] {
        assert_eq!(common_index, own_index);
    }
    use rudroid_common::jni_env::slot::vm;
    assert_eq!(
        (vm::ATTACH_CURRENT_THREAD, vm::DETACH_CURRENT_THREAD, vm::GET_ENV),
        (vm_slot::ATTACH_CURRENT_THREAD, vm_slot::DETACH_CURRENT_THREAD, vm_slot::GET_ENV)
    );
}
