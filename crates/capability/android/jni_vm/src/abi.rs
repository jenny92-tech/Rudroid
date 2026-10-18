//! The minimal JavaVM/JNIEnv C ABI of the shared session (ticket 52).
//!
//! Layout authority for the kit's guest-facing JNI tables. The slot order
//! is the fixed JNINativeInterface/JNIInvokeInterface order from the
//! read-only oracle (`libjnivm/include/jni.h`, matching the JNI 1.6 spec):
//! 233 env slots (0..=232, the first four reserved and null) and 8 VM
//! slots. Guest-visible `JavaVM*` and `JNIEnv*` point to stable handles
//! whose first word points to the corresponding static table. The session crosses
//! dynamic-library boundaries through the function tables themselves, never
//! through a static link (ticket 43).
//!
//! Implemented vs explicit failure: exactly [`ENV_IMPLEMENTED`] entries are
//! real (the minimum the ticket's dynamic loop needs: version/env
//! negotiation, class/method lookup, static int/bool calls, UTF-8 strings,
//! local/global references and local frames, exception state, natives
//! registration, monitors, GetJavaVM). EVERY other slot is a named thunk
//! ([`unimplemented`]) that logs its own entry name, sets the pending
//! exception and returns a zero default — never a silent fake success and
//! never a null table posing as a VM. A zero default would read as JNI_OK
//! for a status-returning entry, so every such entry is implemented (a
//! test pins it). The completeness test asserts no slot outside the four
//! reserved ones stays null.
//!
//! Variadic (`...`) and `va_list` (V) variants are implemented only on Linux
//! ARM64 GNU, where `varargs.c` collects the arguments by descriptor and
//! forwards them to the same `A` (jvalue array) dispatcher; elsewhere they
//! stay explicit failures (stable Rust cannot read a `va_list`).

use std::ffi::c_void;
use std::sync::OnceLock;

/// JNI return codes / version negotiated by `GetEnv` (spec values).
pub const JNI_OK: i32 = 0;
pub const JNI_ERR: i32 = -1;
pub const JNI_EDETACHED: i32 = -2;
pub const JNI_EVERSION: i32 = -3;
pub const JNI_VERSION_1_1: i32 = 0x0001_0001;
pub const JNI_VERSION_1_2: i32 = 0x0001_0002;
pub const JNI_VERSION_1_4: i32 = 0x0001_0004;
pub const JNI_VERSION_1_6: i32 = 0x0001_0006;

pub const JNI_TRUE: u8 = 1;
pub const JNI_FALSE: u8 = 0;

/// Total JNINativeInterface slots in the spec layout (indices 0..=232).
pub const ENV_SLOT_COUNT: usize = 233;
/// JNIInvokeInterface slots: reserved0..2 + five functions.
pub const VM_SLOT_COUNT: usize = 8;

/// JNIInvokeInterface slot indices (0..=2 are the reserved slots).
pub mod vm_slot {
    pub const DESTROY_JAVA_VM: usize = 3;
    pub const ATTACH_CURRENT_THREAD: usize = 4;
    pub const DETACH_CURRENT_THREAD: usize = 5;
    pub const GET_ENV: usize = 6;
    pub const ATTACH_CURRENT_THREAD_AS_DAEMON: usize = 7;
}

/// The fixed JNINativeInterface slot order. Names and indices are
/// transcribed from the oracle header; the layout test pins the spec
/// positions (GetVersion=4, FindClass=6, GetStringUTFChars=169,
/// GetJavaVM=219, ExceptionCheck=228, GetObjectRefType=232) so any
/// transcription drift fails instead of silently moving a guest slot.
macro_rules! jni_slots {
    ($( $name:ident = $idx:literal ),* $(,)?) => {
        /// Slot index constants, named after the JNI entry they hold.
        #[allow(non_upper_case_globals)]
        pub mod env_slot {
            $( pub const $name: usize = $idx; )*
        }
        /// Slot index -> entry name, in spec order (diagnostics + tests).
        pub const ENV_SLOT_NAMES: [&str; ENV_SLOT_COUNT] = [$( stringify!($name) ),*];
    };
}

jni_slots!(
    reserved0 = 0, reserved1 = 1, reserved2 = 2, reserved3 = 3, GetVersion = 4, DefineClass = 5,
    FindClass = 6, FromReflectedMethod = 7, FromReflectedField = 8, ToReflectedMethod = 9,
    GetSuperclass = 10, IsAssignableFrom = 11, ToReflectedField = 12, Throw = 13, ThrowNew = 14,
    ExceptionOccurred = 15, ExceptionDescribe = 16, ExceptionClear = 17, FatalError = 18,
    PushLocalFrame = 19, PopLocalFrame = 20, NewGlobalRef = 21, DeleteGlobalRef = 22,
    DeleteLocalRef = 23, IsSameObject = 24, NewLocalRef = 25, EnsureLocalCapacity = 26,
    AllocObject = 27, NewObject = 28, NewObjectV = 29, NewObjectA = 30, GetObjectClass = 31,
    IsInstanceOf = 32, GetMethodID = 33, CallObjectMethod = 34, CallObjectMethodV = 35,
    CallObjectMethodA = 36, CallBooleanMethod = 37, CallBooleanMethodV = 38, CallBooleanMethodA = 39,
    CallByteMethod = 40, CallByteMethodV = 41, CallByteMethodA = 42, CallCharMethod = 43,
    CallCharMethodV = 44, CallCharMethodA = 45, CallShortMethod = 46, CallShortMethodV = 47,
    CallShortMethodA = 48, CallIntMethod = 49, CallIntMethodV = 50, CallIntMethodA = 51,
    CallLongMethod = 52, CallLongMethodV = 53, CallLongMethodA = 54, CallFloatMethod = 55,
    CallFloatMethodV = 56, CallFloatMethodA = 57, CallDoubleMethod = 58, CallDoubleMethodV = 59,
    CallDoubleMethodA = 60, CallVoidMethod = 61, CallVoidMethodV = 62, CallVoidMethodA = 63,
    CallNonvirtualObjectMethod = 64, CallNonvirtualObjectMethodV = 65, CallNonvirtualObjectMethodA = 66,
    CallNonvirtualBooleanMethod = 67, CallNonvirtualBooleanMethodV = 68, CallNonvirtualBooleanMethodA = 69,
    CallNonvirtualByteMethod = 70, CallNonvirtualByteMethodV = 71, CallNonvirtualByteMethodA = 72,
    CallNonvirtualCharMethod = 73, CallNonvirtualCharMethodV = 74, CallNonvirtualCharMethodA = 75,
    CallNonvirtualShortMethod = 76, CallNonvirtualShortMethodV = 77, CallNonvirtualShortMethodA = 78,
    CallNonvirtualIntMethod = 79, CallNonvirtualIntMethodV = 80, CallNonvirtualIntMethodA = 81,
    CallNonvirtualLongMethod = 82, CallNonvirtualLongMethodV = 83, CallNonvirtualLongMethodA = 84,
    CallNonvirtualFloatMethod = 85, CallNonvirtualFloatMethodV = 86, CallNonvirtualFloatMethodA = 87,
    CallNonvirtualDoubleMethod = 88, CallNonvirtualDoubleMethodV = 89, CallNonvirtualDoubleMethodA = 90,
    CallNonvirtualVoidMethod = 91, CallNonvirtualVoidMethodV = 92, CallNonvirtualVoidMethodA = 93,
    GetFieldID = 94, GetObjectField = 95, GetBooleanField = 96, GetByteField = 97, GetCharField = 98,
    GetShortField = 99, GetIntField = 100, GetLongField = 101, GetFloatField = 102,
    GetDoubleField = 103, SetObjectField = 104, SetBooleanField = 105, SetByteField = 106,
    SetCharField = 107, SetShortField = 108, SetIntField = 109, SetLongField = 110,
    SetFloatField = 111, SetDoubleField = 112, GetStaticMethodID = 113, CallStaticObjectMethod = 114,
    CallStaticObjectMethodV = 115, CallStaticObjectMethodA = 116, CallStaticBooleanMethod = 117,
    CallStaticBooleanMethodV = 118, CallStaticBooleanMethodA = 119, CallStaticByteMethod = 120,
    CallStaticByteMethodV = 121, CallStaticByteMethodA = 122, CallStaticCharMethod = 123,
    CallStaticCharMethodV = 124, CallStaticCharMethodA = 125, CallStaticShortMethod = 126,
    CallStaticShortMethodV = 127, CallStaticShortMethodA = 128, CallStaticIntMethod = 129,
    CallStaticIntMethodV = 130, CallStaticIntMethodA = 131, CallStaticLongMethod = 132,
    CallStaticLongMethodV = 133, CallStaticLongMethodA = 134, CallStaticFloatMethod = 135,
    CallStaticFloatMethodV = 136, CallStaticFloatMethodA = 137, CallStaticDoubleMethod = 138,
    CallStaticDoubleMethodV = 139, CallStaticDoubleMethodA = 140, CallStaticVoidMethod = 141,
    CallStaticVoidMethodV = 142, CallStaticVoidMethodA = 143, GetStaticFieldID = 144,
    GetStaticObjectField = 145, GetStaticBooleanField = 146, GetStaticByteField = 147,
    GetStaticCharField = 148, GetStaticShortField = 149, GetStaticIntField = 150,
    GetStaticLongField = 151, GetStaticFloatField = 152, GetStaticDoubleField = 153,
    SetStaticObjectField = 154, SetStaticBooleanField = 155, SetStaticByteField = 156,
    SetStaticCharField = 157, SetStaticShortField = 158, SetStaticIntField = 159,
    SetStaticLongField = 160, SetStaticFloatField = 161, SetStaticDoubleField = 162,
    NewString = 163, GetStringLength = 164, GetStringChars = 165, ReleaseStringChars = 166,
    NewStringUTF = 167, GetStringUTFLength = 168, GetStringUTFChars = 169,
    ReleaseStringUTFChars = 170, GetArrayLength = 171, NewObjectArray = 172,
    GetObjectArrayElement = 173, SetObjectArrayElement = 174, NewBooleanArray = 175,
    NewByteArray = 176, NewCharArray = 177, NewShortArray = 178, NewIntArray = 179,
    NewLongArray = 180, NewFloatArray = 181, NewDoubleArray = 182,
    GetBooleanArrayElements = 183, GetByteArrayElements = 184, GetCharArrayElements = 185,
    GetShortArrayElements = 186, GetIntArrayElements = 187, GetLongArrayElements = 188,
    GetFloatArrayElements = 189, GetDoubleArrayElements = 190, ReleaseBooleanArrayElements = 191,
    ReleaseByteArrayElements = 192, ReleaseCharArrayElements = 193, ReleaseShortArrayElements = 194,
    ReleaseIntArrayElements = 195, ReleaseLongArrayElements = 196, ReleaseFloatArrayElements = 197,
    ReleaseDoubleArrayElements = 198, GetBooleanArrayRegion = 199, GetByteArrayRegion = 200,
    GetCharArrayRegion = 201, GetShortArrayRegion = 202, GetIntArrayRegion = 203,
    GetLongArrayRegion = 204, GetFloatArrayRegion = 205, GetDoubleArrayRegion = 206,
    SetBooleanArrayRegion = 207, SetByteArrayRegion = 208, SetCharArrayRegion = 209,
    SetShortArrayRegion = 210, SetIntArrayRegion = 211, SetLongArrayRegion = 212,
    SetFloatArrayRegion = 213, SetDoubleArrayRegion = 214, RegisterNatives = 215,
    UnregisterNatives = 216, MonitorEnter = 217, MonitorExit = 218, GetJavaVM = 219,
    GetStringRegion = 220, GetStringUTFRegion = 221, GetPrimitiveArrayCritical = 222,
    ReleasePrimitiveArrayCritical = 223, GetStringCritical = 224, ReleaseStringCritical = 225,
    NewWeakGlobalRef = 226, DeleteWeakGlobalRef = 227, ExceptionCheck = 228,
    NewDirectByteBuffer = 229, GetDirectBufferAddress = 230, GetDirectBufferCapacity = 231,
    GetObjectRefType = 232,
);

/// The env entries this minimal session implements. Everything else is a
/// named explicit-failure thunk (the manifest/table in the ticket-52
/// evidence mirrors this list).
pub const ENV_IMPLEMENTED: &[usize] = &[
    env_slot::CallNonvirtualObjectMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualObjectMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualObjectMethodV,
    env_slot::CallNonvirtualBooleanMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualBooleanMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualBooleanMethodV,
    env_slot::CallNonvirtualByteMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualByteMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualByteMethodV,
    env_slot::CallNonvirtualCharMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualCharMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualCharMethodV,
    env_slot::CallNonvirtualShortMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualShortMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualShortMethodV,
    env_slot::CallNonvirtualIntMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualIntMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualIntMethodV,
    env_slot::CallNonvirtualLongMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualLongMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualLongMethodV,
    env_slot::CallNonvirtualFloatMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualFloatMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualFloatMethodV,
    env_slot::CallNonvirtualDoubleMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualDoubleMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualDoubleMethodV,
    env_slot::CallNonvirtualVoidMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualVoidMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallNonvirtualVoidMethodV,

    env_slot::CallByteMethodA, env_slot::CallCharMethodA, env_slot::CallShortMethodA,
    env_slot::CallDoubleMethodA, env_slot::CallStaticByteMethodA, env_slot::CallStaticCharMethodA,
    env_slot::CallStaticShortMethodA, env_slot::CallStaticDoubleMethodA,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallByteMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallByteMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallCharMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallCharMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallShortMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallShortMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticByteMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticByteMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticCharMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticCharMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticShortMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticShortMethodV,
    env_slot::IsAssignableFrom,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallObjectMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallObjectMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallBooleanMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallBooleanMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallIntMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallIntMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallLongMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallLongMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallFloatMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallFloatMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallDoubleMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallDoubleMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticObjectMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticObjectMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticBooleanMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticBooleanMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticIntMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticIntMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticLongMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticLongMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticFloatMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticFloatMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticDoubleMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticDoubleMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::NewObject,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::NewObjectV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallVoidMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallVoidMethodV,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticVoidMethod,
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    env_slot::CallStaticVoidMethodV,

    env_slot::GetFieldID,
    env_slot::GetObjectField,
    env_slot::GetBooleanField,
    env_slot::GetByteField,
    env_slot::GetCharField,
    env_slot::GetShortField,
    env_slot::GetIntField,
    env_slot::GetLongField,
    env_slot::GetFloatField,
    env_slot::GetDoubleField,
    env_slot::SetObjectField,
    env_slot::SetBooleanField,
    env_slot::SetByteField,
    env_slot::SetCharField,
    env_slot::SetShortField,
    env_slot::SetIntField,
    env_slot::SetLongField,
    env_slot::SetFloatField,
    env_slot::SetDoubleField,
    env_slot::GetStaticFieldID,
    env_slot::GetStaticObjectField,
    env_slot::GetStaticBooleanField,
    env_slot::GetStaticByteField,
    env_slot::GetStaticCharField,
    env_slot::GetStaticShortField,
    env_slot::GetStaticIntField,
    env_slot::GetStaticLongField,
    env_slot::GetStaticFloatField,
    env_slot::GetStaticDoubleField,
    env_slot::SetStaticObjectField,
    env_slot::SetStaticBooleanField,
    env_slot::SetStaticByteField,
    env_slot::SetStaticCharField,
    env_slot::SetStaticShortField,
    env_slot::SetStaticIntField,
    env_slot::SetStaticLongField,
    env_slot::SetStaticFloatField,
    env_slot::SetStaticDoubleField,
    env_slot::NewDirectByteBuffer,
    env_slot::GetDirectBufferAddress,
    env_slot::GetDirectBufferCapacity,
    env_slot::NewBooleanArray,
    env_slot::GetBooleanArrayElements,
    env_slot::ReleaseBooleanArrayElements,
    env_slot::GetBooleanArrayRegion,
    env_slot::SetBooleanArrayRegion,
    env_slot::NewCharArray,
    env_slot::GetCharArrayElements,
    env_slot::ReleaseCharArrayElements,
    env_slot::GetCharArrayRegion,
    env_slot::SetCharArrayRegion,
    env_slot::NewShortArray,
    env_slot::GetShortArrayElements,
    env_slot::ReleaseShortArrayElements,
    env_slot::GetShortArrayRegion,
    env_slot::SetShortArrayRegion,
    env_slot::NewLongArray,
    env_slot::GetLongArrayElements,
    env_slot::ReleaseLongArrayElements,
    env_slot::GetLongArrayRegion,
    env_slot::SetLongArrayRegion,
    env_slot::NewFloatArray,
    env_slot::GetFloatArrayElements,
    env_slot::ReleaseFloatArrayElements,
    env_slot::GetFloatArrayRegion,
    env_slot::SetFloatArrayRegion,
    env_slot::NewDoubleArray,
    env_slot::GetDoubleArrayElements,
    env_slot::ReleaseDoubleArrayElements,
    env_slot::GetDoubleArrayRegion,
    env_slot::SetDoubleArrayRegion,
    env_slot::GetPrimitiveArrayCritical,
    env_slot::ReleasePrimitiveArrayCritical,
    env_slot::NewIntArray,
    env_slot::GetIntArrayElements,
    env_slot::ReleaseIntArrayElements,
    env_slot::GetIntArrayRegion,
    env_slot::SetIntArrayRegion,
    env_slot::CallFloatMethodA,
    env_slot::CallStaticFloatMethodA,
    env_slot::FromReflectedMethod,
    env_slot::FromReflectedField,
    env_slot::ToReflectedField,
    env_slot::ToReflectedMethod,
    env_slot::NewWeakGlobalRef,
    env_slot::DeleteWeakGlobalRef,
    env_slot::GetObjectRefType,
    env_slot::NewString,
    env_slot::GetStringLength,
    env_slot::GetStringChars,
    env_slot::ReleaseStringChars,
    env_slot::GetStringCritical,
    env_slot::ReleaseStringCritical,
    env_slot::GetStringRegion,
    env_slot::GetStringUTFRegion,
    env_slot::NewByteArray,
    env_slot::GetByteArrayElements,
    env_slot::ReleaseByteArrayElements,
    env_slot::GetByteArrayRegion,
    env_slot::SetByteArrayRegion,
    env_slot::CallIntMethodA,
    env_slot::CallLongMethodA,
    env_slot::CallStaticLongMethodA,
    env_slot::GetVersion,
    env_slot::FindClass,
    env_slot::Throw,
    env_slot::ThrowNew,
    env_slot::ExceptionOccurred,
    env_slot::ExceptionDescribe,
    env_slot::ExceptionClear,
    env_slot::FatalError,
    env_slot::PushLocalFrame,
    env_slot::PopLocalFrame,
    env_slot::NewGlobalRef,
    env_slot::DeleteGlobalRef,
    env_slot::DeleteLocalRef,
    env_slot::IsSameObject,
    env_slot::NewLocalRef,
    env_slot::EnsureLocalCapacity,
    env_slot::AllocObject,
    env_slot::NewObjectA,
    env_slot::NewObjectArray,
    env_slot::GetArrayLength,
    env_slot::GetObjectArrayElement,
    env_slot::SetObjectArrayElement,
    env_slot::GetObjectClass,
    env_slot::GetSuperclass,
    env_slot::IsInstanceOf,
    env_slot::GetMethodID,
    env_slot::CallVoidMethodA,
    env_slot::CallObjectMethodA,
    env_slot::CallStaticObjectMethodA,
    env_slot::CallBooleanMethodA,
    env_slot::GetStaticMethodID,
    env_slot::CallStaticBooleanMethodA,
    env_slot::CallStaticIntMethodA,
    env_slot::CallStaticVoidMethodA,
    env_slot::NewStringUTF,
    env_slot::GetStringUTFLength,
    env_slot::GetStringUTFChars,
    env_slot::ReleaseStringUTFChars,
    env_slot::RegisterNatives,
    env_slot::UnregisterNatives,
    env_slot::MonitorEnter,
    env_slot::MonitorExit,
    env_slot::GetJavaVM,
    env_slot::ExceptionCheck,
];

// One thunk per unimplemented slot, named after its entry so the failure
// log (and any backtrace) identifies the exact missing JNI surface.
macro_rules! jni_unimplemented_slots {
    ($( $name:ident ),* $(,)?) => {
        /// Named explicit-failure thunks. Each logs its entry name, leaves
        /// the pending exception set (when the calling `env` is valid) and
        /// returns a zero default; float-returning slots additionally leave
        /// the SIMD return register untouched (the pending exception is the
        /// defined failure signal).
        #[allow(non_snake_case)]
        pub mod unimplemented {
            use std::ffi::c_void;
            $( pub unsafe extern "C" fn $name(env: *const c_void) -> usize {
                $crate::session::report_unimplemented_slot(stringify!($name), env)
            } )*
        }

        /// Point every not-implemented slot at its named thunk.
        fn fill_unimplemented(table: &mut [usize; ENV_SLOT_COUNT]) {
            $( table[env_slot::$name] = unimplemented::$name as *const c_void as usize; )*
        }
    };
}

jni_unimplemented_slots!(
    DefineClass, FromReflectedMethod, FromReflectedField, ToReflectedMethod,
    ToReflectedField, NewObject,
    NewObjectV, CallObjectMethod, CallObjectMethodV,
    CallBooleanMethod, CallBooleanMethodV, CallByteMethod,
    CallByteMethodV, CallByteMethodA, CallCharMethod, CallCharMethodV, CallCharMethodA,
    CallShortMethod, CallShortMethodV, CallShortMethodA, CallIntMethod, CallIntMethodV,
    CallIntMethodA, CallLongMethod, CallLongMethodV, CallLongMethodA, CallFloatMethod,
    CallFloatMethodV, CallFloatMethodA, CallDoubleMethod, CallDoubleMethodV, CallDoubleMethodA,
    CallVoidMethod, CallVoidMethodV, CallNonvirtualObjectMethod,
    CallNonvirtualObjectMethodV, CallNonvirtualObjectMethodA, CallNonvirtualBooleanMethod,
    CallNonvirtualBooleanMethodV, CallNonvirtualBooleanMethodA, CallNonvirtualByteMethod,
    CallNonvirtualByteMethodV, CallNonvirtualByteMethodA, CallNonvirtualCharMethod,
    CallNonvirtualCharMethodV, CallNonvirtualCharMethodA, CallNonvirtualShortMethod,
    CallNonvirtualShortMethodV, CallNonvirtualShortMethodA, CallNonvirtualIntMethod,
    CallNonvirtualIntMethodV, CallNonvirtualIntMethodA, CallNonvirtualLongMethod,
    CallNonvirtualLongMethodV, CallNonvirtualLongMethodA, CallNonvirtualFloatMethod,
    CallNonvirtualFloatMethodV, CallNonvirtualFloatMethodA, CallNonvirtualDoubleMethod,
    CallNonvirtualDoubleMethodV, CallNonvirtualDoubleMethodA, CallNonvirtualVoidMethod,
    CallNonvirtualVoidMethodV, CallNonvirtualVoidMethodA, GetFieldID, GetObjectField,
    GetBooleanField, GetByteField, GetCharField, GetShortField, GetIntField, GetLongField,
    GetFloatField, GetDoubleField, SetObjectField, SetBooleanField, SetByteField, SetCharField,
    SetShortField, SetIntField, SetLongField, SetFloatField, SetDoubleField,
    CallStaticObjectMethod, CallStaticObjectMethodV,
    CallStaticBooleanMethod, CallStaticBooleanMethodV, CallStaticByteMethod, CallStaticByteMethodV,
    CallStaticByteMethodA, CallStaticCharMethod, CallStaticCharMethodV, CallStaticCharMethodA,
    CallStaticShortMethod, CallStaticShortMethodV, CallStaticShortMethodA, CallStaticIntMethod,
    CallStaticIntMethodV, CallStaticLongMethod, CallStaticLongMethodV, CallStaticLongMethodA,
    CallStaticFloatMethod, CallStaticFloatMethodV, CallStaticFloatMethodA, CallStaticDoubleMethod,
    CallStaticDoubleMethodV, CallStaticDoubleMethodA, CallStaticVoidMethod, CallStaticVoidMethodV,
    GetStaticFieldID, GetStaticObjectField, GetStaticBooleanField,
    GetStaticByteField, GetStaticCharField, GetStaticShortField, GetStaticIntField,
    GetStaticLongField, GetStaticFloatField, GetStaticDoubleField, SetStaticObjectField,
    SetStaticBooleanField, SetStaticByteField, SetStaticCharField, SetStaticShortField,
    SetStaticIntField, SetStaticLongField, SetStaticFloatField, SetStaticDoubleField, NewString,
    GetStringLength, GetStringChars, ReleaseStringChars,
    NewBooleanArray, NewByteArray, NewCharArray,
    NewShortArray, NewIntArray, NewLongArray, NewFloatArray, NewDoubleArray,
    GetBooleanArrayElements, GetByteArrayElements, GetCharArrayElements, GetShortArrayElements,
    GetIntArrayElements, GetLongArrayElements, GetFloatArrayElements, GetDoubleArrayElements,
    ReleaseBooleanArrayElements, ReleaseByteArrayElements, ReleaseCharArrayElements,
    ReleaseShortArrayElements, ReleaseIntArrayElements, ReleaseLongArrayElements,
    ReleaseFloatArrayElements, ReleaseDoubleArrayElements, GetBooleanArrayRegion,
    GetByteArrayRegion, GetCharArrayRegion, GetShortArrayRegion, GetIntArrayRegion,
    GetLongArrayRegion, GetFloatArrayRegion, GetDoubleArrayRegion, SetBooleanArrayRegion,
    SetByteArrayRegion, SetCharArrayRegion, SetShortArrayRegion, SetIntArrayRegion,
    SetLongArrayRegion, SetFloatArrayRegion, SetDoubleArrayRegion, GetStringRegion,
    GetStringUTFRegion,
    GetPrimitiveArrayCritical, ReleasePrimitiveArrayCritical,
    NewWeakGlobalRef, DeleteWeakGlobalRef, NewDirectByteBuffer,
    GetDirectBufferAddress, GetDirectBufferCapacity, GetObjectRefType,
);

/// The call-accessor kind a static call variant reads — the discriminator
/// behind the explicit type-mismatch failure (Int = jint-family variants,
/// Bool = jboolean-family variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Byte,
    Char,
    Short,
    Double,
    Float,
    Int,
    Long,
    Bool,
    Void,
    Object,
}

/// The guest-visible `JavaVM*` — a handle pointing to the invoke table.
/// Process-lifetime like the session itself (registry ownership: the
/// shared-VM slot is first-write-wins and never torn down).
pub fn java_vm_ptr() -> *mut c_void {
    static VM: OnceLock<&'static [usize; VM_SLOT_COUNT]> = OnceLock::new();
    let handle = VM.get_or_init(invoke_table);
    (handle as *const &'static [usize; VM_SLOT_COUNT]).cast_mut().cast()
}

/// The guest-visible `JNIEnv*` (a handle pointing to the native interface
/// table). The same main-thread env answers every thread, mirroring the
/// oracle's non-GC attach behavior (documented on the session).
pub fn env_ptr() -> *mut c_void {
    static ENV: OnceLock<&'static [usize; ENV_SLOT_COUNT]> = OnceLock::new();
    let handle = ENV.get_or_init(native_interface);
    (handle as *const &'static [usize; ENV_SLOT_COUNT]).cast_mut().cast()
}

/// The static JNIInvokeInterface: reserved slots stay null (spec), the five
/// functions are real. Assembled once on first use.
fn invoke_table() -> &'static [usize; VM_SLOT_COUNT] {
    static TABLE: OnceLock<[usize; VM_SLOT_COUNT]> = OnceLock::new();
    TABLE.get_or_init(|| {
        use super::session::shims as s;
        let mut table = [0usize; VM_SLOT_COUNT];
        table[vm_slot::DESTROY_JAVA_VM] = s::destroy_java_vm as *const c_void as usize;
        table[vm_slot::ATTACH_CURRENT_THREAD] = s::attach_current_thread as *const c_void as usize;
        table[vm_slot::DETACH_CURRENT_THREAD] = s::detach_current_thread as *const c_void as usize;
        table[vm_slot::GET_ENV] = s::get_env as *const c_void as usize;
        table[vm_slot::ATTACH_CURRENT_THREAD_AS_DAEMON] =
            s::attach_current_thread_as_daemon as *const c_void as usize;
        table
    })
}

/// The static JNINativeInterface: implemented entries call the session
/// shims, every other slot its named thunk. Panics if any non-reserved slot
/// is left null — the "no null table posing as a VM" invariant, enforced at
/// first use instead of being discovered by a guest segfault.
fn native_interface() -> &'static [usize; ENV_SLOT_COUNT] {
    static TABLE: OnceLock<[usize; ENV_SLOT_COUNT]> = OnceLock::new();
    TABLE.get_or_init(|| {
        use super::session::shims as s;
        let mut table = [0usize; ENV_SLOT_COUNT];
        fill_unimplemented(&mut table);
        s::bind_nonvirtual(&mut table);
        table[env_slot::CallByteMethodA] = s::call_byte_method_a as *const c_void as usize;
        table[env_slot::CallCharMethodA] = s::call_char_method_a as *const c_void as usize;
        table[env_slot::CallShortMethodA] = s::call_short_method_a as *const c_void as usize;
        table[env_slot::CallDoubleMethodA] = s::call_double_method_a as *const c_void as usize;
        table[env_slot::CallStaticByteMethodA] = s::call_static_byte_method_a as *const c_void as usize;
        table[env_slot::CallStaticCharMethodA] = s::call_static_char_method_a as *const c_void as usize;
        table[env_slot::CallStaticShortMethodA] = s::call_static_short_method_a as *const c_void as usize;
        table[env_slot::CallStaticDoubleMethodA] = s::call_static_double_method_a as *const c_void as usize;
        table[env_slot::IsAssignableFrom] = s::is_assignable_from as *const c_void as usize;
        table[env_slot::NewDirectByteBuffer] = s::new_direct_byte_buffer as *const c_void as usize;
        table[env_slot::GetDirectBufferAddress] = s::get_direct_buffer_address as *const c_void as usize;
        table[env_slot::GetDirectBufferCapacity] = s::get_direct_buffer_capacity as *const c_void as usize;
        table[env_slot::NewIntArray] = s::new_int_array as *const c_void as usize;
        table[env_slot::GetIntArrayElements] = s::get_int_array_elements as *const c_void as usize;
        table[env_slot::ReleaseIntArrayElements] = s::release_int_array_elements as *const c_void as usize;
        table[env_slot::GetIntArrayRegion] = s::get_int_array_region as *const c_void as usize;
        table[env_slot::SetIntArrayRegion] = s::set_int_array_region as *const c_void as usize;
        table[env_slot::CallFloatMethodA] = s::call_float_method_a as *const c_void as usize;
        table[env_slot::CallStaticFloatMethodA] = s::call_static_float_method_a as *const c_void as usize;
        table[env_slot::GetVersion] = s::get_version as *const c_void as usize;
        table[env_slot::GetFieldID] = s::get_field_id as *const c_void as usize;
        table[env_slot::GetObjectField] = s::get_object_field as *const c_void as usize;
        table[env_slot::GetBooleanField] = s::get_boolean_field as *const c_void as usize;
        table[env_slot::GetByteField] = s::get_byte_field as *const c_void as usize;
        table[env_slot::GetCharField] = s::get_char_field as *const c_void as usize;
        table[env_slot::GetShortField] = s::get_short_field as *const c_void as usize;
        table[env_slot::GetIntField] = s::get_int_field as *const c_void as usize;
        table[env_slot::GetLongField] = s::get_long_field as *const c_void as usize;
        table[env_slot::GetFloatField] = s::get_float_field as *const c_void as usize;
        table[env_slot::GetDoubleField] = s::get_double_field as *const c_void as usize;
        table[env_slot::SetObjectField] = s::set_object_field as *const c_void as usize;
        table[env_slot::SetBooleanField] = s::set_boolean_field as *const c_void as usize;
        table[env_slot::SetByteField] = s::set_byte_field as *const c_void as usize;
        table[env_slot::SetCharField] = s::set_char_field as *const c_void as usize;
        table[env_slot::SetShortField] = s::set_short_field as *const c_void as usize;
        table[env_slot::SetIntField] = s::set_int_field as *const c_void as usize;
        table[env_slot::SetLongField] = s::set_long_field as *const c_void as usize;
        table[env_slot::SetFloatField] = s::set_float_field as *const c_void as usize;
        table[env_slot::SetDoubleField] = s::set_double_field as *const c_void as usize;
        table[env_slot::GetStaticFieldID] = s::get_static_field_id as *const c_void as usize;
        table[env_slot::GetStaticObjectField] = s::get_static_object_field as *const c_void as usize;
        table[env_slot::GetStaticBooleanField] = s::get_static_boolean_field as *const c_void as usize;
        table[env_slot::GetStaticByteField] = s::get_static_byte_field as *const c_void as usize;
        table[env_slot::GetStaticCharField] = s::get_static_char_field as *const c_void as usize;
        table[env_slot::GetStaticShortField] = s::get_static_short_field as *const c_void as usize;
        table[env_slot::GetStaticIntField] = s::get_static_int_field as *const c_void as usize;
        table[env_slot::GetStaticLongField] = s::get_static_long_field as *const c_void as usize;
        table[env_slot::GetStaticFloatField] = s::get_static_float_field as *const c_void as usize;
        table[env_slot::GetStaticDoubleField] = s::get_static_double_field as *const c_void as usize;
        table[env_slot::SetStaticObjectField] = s::set_static_object_field as *const c_void as usize;
        table[env_slot::SetStaticBooleanField] = s::set_static_boolean_field as *const c_void as usize;
        table[env_slot::SetStaticByteField] = s::set_static_byte_field as *const c_void as usize;
        table[env_slot::SetStaticCharField] = s::set_static_char_field as *const c_void as usize;
        table[env_slot::SetStaticShortField] = s::set_static_short_field as *const c_void as usize;
        table[env_slot::SetStaticIntField] = s::set_static_int_field as *const c_void as usize;
        table[env_slot::SetStaticLongField] = s::set_static_long_field as *const c_void as usize;
        table[env_slot::SetStaticFloatField] = s::set_static_float_field as *const c_void as usize;
        table[env_slot::SetStaticDoubleField] = s::set_static_double_field as *const c_void as usize;
        table[env_slot::FromReflectedField]=s::from_reflected_field as *const c_void as usize;
        table[env_slot::ToReflectedField]=s::to_reflected_field as *const c_void as usize;
        table[env_slot::FromReflectedMethod] = s::from_reflected_method as *const c_void as usize;
        table[env_slot::ToReflectedMethod] = s::to_reflected_method as *const c_void as usize;
        table[env_slot::NewWeakGlobalRef] = s::new_weak_global_ref as *const c_void as usize;
        table[env_slot::DeleteWeakGlobalRef] = s::delete_weak_global_ref as *const c_void as usize;
        table[env_slot::GetObjectRefType] = s::get_object_ref_type as *const c_void as usize;
        table[env_slot::NewString]=s::new_string as *const c_void as usize;
        table[env_slot::GetStringLength]=s::get_string_length as *const c_void as usize;
        table[env_slot::GetStringChars]=s::get_string_chars as *const c_void as usize;
        table[env_slot::ReleaseStringChars]=s::release_string_chars as *const c_void as usize;
        // The VM owns immutable, non-moving UTF-16 storage. Both exact JNI
        // ABIs use the same pin/release ledger; critical callers still obey
        // JNI's restrictions on other calls and blocking between the pair.
        table[env_slot::GetStringCritical]=s::get_string_chars as *const c_void as usize;
        table[env_slot::ReleaseStringCritical]=s::release_string_chars as *const c_void as usize;
        table[env_slot::GetStringRegion]=s::get_string_region as *const c_void as usize;
        table[env_slot::GetStringUTFRegion]=s::get_string_utf_region as *const c_void as usize;
        table[env_slot::NewByteArray]=s::new_byte_array as *const c_void as usize;
        table[env_slot::GetByteArrayElements]=s::get_byte_array_elements as *const c_void as usize;
        table[env_slot::ReleaseByteArrayElements]=s::release_byte_array_elements as *const c_void as usize;
        table[env_slot::GetByteArrayRegion]=s::get_byte_array_region as *const c_void as usize;
        table[env_slot::NewBooleanArray]=s::new_boolean_array as *const c_void as usize;
        table[env_slot::GetBooleanArrayElements]=s::get_boolean_array_elements as *const c_void as usize;
        table[env_slot::ReleaseBooleanArrayElements]=s::release_boolean_array_elements as *const c_void as usize;
        table[env_slot::GetBooleanArrayRegion]=s::get_boolean_array_region as *const c_void as usize;
        table[env_slot::SetBooleanArrayRegion]=s::set_boolean_array_region as *const c_void as usize;
        table[env_slot::NewCharArray]=s::new_char_array as *const c_void as usize;
        table[env_slot::GetCharArrayElements]=s::get_char_array_elements as *const c_void as usize;
        table[env_slot::ReleaseCharArrayElements]=s::release_char_array_elements as *const c_void as usize;
        table[env_slot::GetCharArrayRegion]=s::get_char_array_region as *const c_void as usize;
        table[env_slot::SetCharArrayRegion]=s::set_char_array_region as *const c_void as usize;
        table[env_slot::NewShortArray]=s::new_short_array as *const c_void as usize;
        table[env_slot::GetShortArrayElements]=s::get_short_array_elements as *const c_void as usize;
        table[env_slot::ReleaseShortArrayElements]=s::release_short_array_elements as *const c_void as usize;
        table[env_slot::GetShortArrayRegion]=s::get_short_array_region as *const c_void as usize;
        table[env_slot::SetShortArrayRegion]=s::set_short_array_region as *const c_void as usize;
        table[env_slot::NewLongArray]=s::new_long_array as *const c_void as usize;
        table[env_slot::GetLongArrayElements]=s::get_long_array_elements as *const c_void as usize;
        table[env_slot::ReleaseLongArrayElements]=s::release_long_array_elements as *const c_void as usize;
        table[env_slot::GetLongArrayRegion]=s::get_long_array_region as *const c_void as usize;
        table[env_slot::SetLongArrayRegion]=s::set_long_array_region as *const c_void as usize;
        table[env_slot::NewFloatArray]=s::new_float_array as *const c_void as usize;
        table[env_slot::GetFloatArrayElements]=s::get_float_array_elements as *const c_void as usize;
        table[env_slot::ReleaseFloatArrayElements]=s::release_float_array_elements as *const c_void as usize;
        table[env_slot::GetFloatArrayRegion]=s::get_float_array_region as *const c_void as usize;
        table[env_slot::SetFloatArrayRegion]=s::set_float_array_region as *const c_void as usize;
        table[env_slot::NewDoubleArray]=s::new_double_array as *const c_void as usize;
        table[env_slot::GetDoubleArrayElements]=s::get_double_array_elements as *const c_void as usize;
        table[env_slot::ReleaseDoubleArrayElements]=s::release_double_array_elements as *const c_void as usize;
        table[env_slot::GetDoubleArrayRegion]=s::get_double_array_region as *const c_void as usize;
        table[env_slot::SetDoubleArrayRegion]=s::set_double_array_region as *const c_void as usize;
        table[env_slot::GetPrimitiveArrayCritical]=s::get_primitive_array_critical as *const c_void as usize;
        table[env_slot::ReleasePrimitiveArrayCritical]=s::release_primitive_array_critical as *const c_void as usize;
        table[env_slot::SetByteArrayRegion]=s::set_byte_array_region as *const c_void as usize;
        table[env_slot::CallLongMethodA]=s::call_long_method_a as *const c_void as usize;
        table[env_slot::CallStaticLongMethodA]=s::call_static_long_method_a as *const c_void as usize;
        table[env_slot::CallIntMethodA]=s::call_int_method_a as *const c_void as usize;
        table[env_slot::FindClass] = s::find_class as *const c_void as usize;
        table[env_slot::Throw] = s::throw as *const c_void as usize;
        table[env_slot::ThrowNew] = s::throw_new as *const c_void as usize;
        table[env_slot::ExceptionOccurred] = s::exception_occurred as *const c_void as usize;
        table[env_slot::ExceptionDescribe] = s::exception_describe as *const c_void as usize;
        table[env_slot::ExceptionClear] = s::exception_clear as *const c_void as usize;
        table[env_slot::FatalError] = s::fatal_error as *const c_void as usize;
        table[env_slot::PushLocalFrame] = s::push_local_frame as *const c_void as usize;
        table[env_slot::PopLocalFrame] = s::pop_local_frame as *const c_void as usize;
        table[env_slot::NewGlobalRef] = s::new_global_ref as *const c_void as usize;
        table[env_slot::DeleteGlobalRef] = s::delete_global_ref as *const c_void as usize;
        table[env_slot::DeleteLocalRef] = s::delete_local_ref as *const c_void as usize;
        table[env_slot::IsSameObject] = s::is_same_object as *const c_void as usize;
        table[env_slot::NewLocalRef] = s::new_local_ref as *const c_void as usize;
        table[env_slot::EnsureLocalCapacity] = s::ensure_local_capacity as *const c_void as usize;
        table[env_slot::GetMethodID] = s::get_method_id as *const c_void as usize;
        table[env_slot::AllocObject] = s::alloc_object as *const c_void as usize;
        table[env_slot::NewObjectA] = s::new_object_a as *const c_void as usize;
        table[env_slot::NewObjectArray] = s::new_object_array as *const c_void as usize;
        table[env_slot::GetArrayLength] = s::get_array_length as *const c_void as usize;
        table[env_slot::GetObjectArrayElement] = s::get_object_array_element as *const c_void as usize;
        table[env_slot::SetObjectArrayElement] = s::set_object_array_element as *const c_void as usize;
        table[env_slot::GetObjectClass] = s::get_object_class as *const c_void as usize;
        table[env_slot::GetSuperclass] = s::get_superclass as *const c_void as usize;
        table[env_slot::IsInstanceOf] = s::is_instance_of as *const c_void as usize;
        table[env_slot::CallVoidMethodA] = s::call_void_method_a as *const c_void as usize;
        table[env_slot::CallObjectMethodA] = s::call_object_method_a as *const c_void as usize;
        table[env_slot::CallStaticObjectMethodA] = s::call_static_object_method_a as *const c_void as usize;
        table[env_slot::CallStaticVoidMethodA] = s::call_static_void_method_a as *const c_void as usize;
        table[env_slot::CallBooleanMethodA] = s::call_boolean_method_a as *const c_void as usize;
        table[env_slot::GetStaticMethodID] = s::get_static_method_id as *const c_void as usize;
        table[env_slot::CallStaticBooleanMethodA] =
            s::call_static_boolean_method_a as *const c_void as usize;
        table[env_slot::CallStaticIntMethodA] =
            s::call_static_int_method_a as *const c_void as usize;
        table[env_slot::NewStringUTF] = s::new_string_utf as *const c_void as usize;
        table[env_slot::GetStringUTFLength] = s::get_string_utf_length as *const c_void as usize;
        table[env_slot::GetStringUTFChars] = s::get_string_utf_chars as *const c_void as usize;
        table[env_slot::ReleaseStringUTFChars] =
            s::release_string_utf_chars as *const c_void as usize;
        table[env_slot::RegisterNatives] = s::register_natives as *const c_void as usize;
        table[env_slot::UnregisterNatives] = s::unregister_natives as *const c_void as usize;
        table[env_slot::MonitorEnter] = s::monitor_enter as *const c_void as usize;
        table[env_slot::MonitorExit] = s::monitor_exit as *const c_void as usize;
        table[env_slot::GetJavaVM] = s::get_java_vm as *const c_void as usize;
        table[env_slot::ExceptionCheck] = s::exception_check as *const c_void as usize;
        #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
        varargs::bind(&mut table);
        for (index, slot) in table.iter().enumerate() {
            assert!(
                index < 4 || *slot != 0,
                "JNI env slot {} ({}) was left null — the table must fail \
                 explicitly, never with a null entry",
                index,
                ENV_SLOT_NAMES[index]
            );
        }
        table
    })
}

/// Test-only access to the assembled tables (layout/coverage assertions).
#[cfg(test)]
pub(crate) fn tables_for_tests() -> (&'static [usize; VM_SLOT_COUNT], &'static [usize; ENV_SLOT_COUNT]) {
    (invoke_table(), native_interface())
}

#[cfg(test)]
#[path = "../tests/unit/abi.rs"]
mod tests;

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[path = "varargs.rs"]
mod varargs;
