//! INPUT:  a live `JavaVM*` / `JNIEnv*` of the in-kit shared JNI session
//! OUTPUT:  typed `JavaVm` / `JniEnv` wrappers, checked calls and natives, the named JNI 1.6 slot table
//! POS:    the single place capabilities read JNI function-table slots; call sites hold no slot numbers, transmutes or `unsafe`
//!
//! Every JNI call a capability makes goes through these methods, so the slot
//! index and the C signature of each entry are written exactly once.
//!
//! Safety model: the in-kit `android_jni_vm` encodes every reference, method
//! ID and field ID as a tagged integer and validates it by table lookup — an
//! unknown handle becomes a pending exception and a zero result, never a
//! memory access. So once a wrapper exists, methods passing handles, IDs,
//! scalars, slices or `&CStr` are safe. A wrapper comes from
//! [`JniEnv::from_raw`] / [`JavaVm::from_raw`] (unsafe) or, for natives, from
//! the VM itself as the first parameter.
//!
//! The standard tools on top:
//! - [`Method`] + [`Arg`]: `env.method(..)` keeps the descriptor's parameter
//!   count, and `call_*` check `args.len()` against it; [`Arg`] constructors
//!   initialise all eight bytes. No `jvalue` pointer reaches a call site.
//! - [`Native`]: `Native::new0..new10` compare the Rust prototype with the JNI
//!   descriptor at registration; [`JniEnv::register`] is then safe.
//! - [`JniEnv::string`] / [`JniEnv::string_utf16`] / [`JniEnv::string_utf`]
//!   copy a Java string and release the VM buffer in one call.
//!
//! The raw `*_a`, `release_*`, `register_natives` and `define_class` methods
//! stay `unsafe fn` for the VM's own tests and genuinely raw callers.

use std::ffi::{c_char, c_void, CStr, CString};

use crate::jni_abi::{function_slot, JValue, NativeMethod};

/// `jobject`, `jclass`, `jstring`, `jarray`, `jthrowable`, `jweak`.
pub type JObject = *mut c_void;
/// `jmethodID`.
pub type JMethodId = *mut c_void;
/// `jfieldID`.
pub type JFieldId = *mut c_void;

pub const JNI_VERSION_1_6: i32 = 0x0001_0006;
pub const JNI_OK: i32 = 0;

/// JNI 1.6 slot indices (spec order). `android_jni_vm` checks these against
/// its own complete table, so the two can never drift apart.
pub mod slot {
    /// `JNIInvokeInterface` (three reserved slots first).
    pub mod vm {
        pub const ATTACH_CURRENT_THREAD: usize = 4;
        pub const DETACH_CURRENT_THREAD: usize = 5;
        pub const GET_ENV: usize = 6;
    }
    /// `JNINativeInterface` (four reserved slots first).
    pub mod env {
        pub const GET_VERSION: usize = 4;
        pub const DEFINE_CLASS: usize = 5;
        pub const FIND_CLASS: usize = 6;
        pub const IS_ASSIGNABLE_FROM: usize = 11;
        pub const TO_REFLECTED_METHOD: usize = 9;
        pub const THROW: usize = 13;
        pub const THROW_NEW: usize = 14;
        pub const EXCEPTION_OCCURRED: usize = 15;
        pub const EXCEPTION_DESCRIBE: usize = 16;
        pub const EXCEPTION_CLEAR: usize = 17;
        pub const PUSH_LOCAL_FRAME: usize = 19;
        pub const POP_LOCAL_FRAME: usize = 20;
        pub const NEW_GLOBAL_REF: usize = 21;
        pub const DELETE_GLOBAL_REF: usize = 22;
        pub const DELETE_LOCAL_REF: usize = 23;
        pub const IS_SAME_OBJECT: usize = 24;
        pub const NEW_LOCAL_REF: usize = 25;
        pub const ALLOC_OBJECT: usize = 27;
        pub const NEW_OBJECT_A: usize = 30;
        pub const GET_OBJECT_CLASS: usize = 31;
        pub const IS_INSTANCE_OF: usize = 32;
        pub const GET_METHOD_ID: usize = 33;
        pub const CALL_OBJECT_METHOD_A: usize = 36;
        pub const CALL_BOOLEAN_METHOD_A: usize = 39;
        pub const CALL_INT_METHOD_A: usize = 51;
        pub const CALL_LONG_METHOD_A: usize = 54;
        pub const CALL_FLOAT_METHOD_A: usize = 57;
        pub const CALL_VOID_METHOD_A: usize = 63;
        pub const GET_FIELD_ID: usize = 94;
        pub const GET_OBJECT_FIELD: usize = 95;
        pub const GET_BOOLEAN_FIELD: usize = 96;
        pub const GET_INT_FIELD: usize = 100;
        pub const GET_LONG_FIELD: usize = 101;
        pub const GET_FLOAT_FIELD: usize = 102;
        pub const SET_OBJECT_FIELD: usize = 104;
        pub const SET_BOOLEAN_FIELD: usize = 105;
        pub const SET_INT_FIELD: usize = 109;
        pub const SET_LONG_FIELD: usize = 110;
        pub const SET_FLOAT_FIELD: usize = 111;
        pub const GET_STATIC_METHOD_ID: usize = 113;
        pub const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
        pub const CALL_STATIC_BOOLEAN_METHOD_A: usize = 119;
        pub const CALL_STATIC_INT_METHOD_A: usize = 131;
        pub const CALL_STATIC_LONG_METHOD_A: usize = 134;
        pub const CALL_STATIC_VOID_METHOD_A: usize = 143;
        pub const GET_STATIC_FIELD_ID: usize = 144;
        pub const GET_STATIC_OBJECT_FIELD: usize = 145;
        pub const GET_STATIC_INT_FIELD: usize = 150;
        pub const SET_STATIC_OBJECT_FIELD: usize = 154;
        pub const SET_STATIC_INT_FIELD: usize = 159;
        pub const NEW_STRING: usize = 163;
        pub const GET_STRING_LENGTH: usize = 164;
        pub const GET_STRING_CHARS: usize = 165;
        pub const RELEASE_STRING_CHARS: usize = 166;
        pub const NEW_STRING_UTF: usize = 167;
        pub const GET_STRING_UTF_CHARS: usize = 169;
        pub const RELEASE_STRING_UTF_CHARS: usize = 170;
        pub const GET_ARRAY_LENGTH: usize = 171;
        pub const NEW_OBJECT_ARRAY: usize = 172;
        pub const GET_OBJECT_ARRAY_ELEMENT: usize = 173;
        pub const SET_OBJECT_ARRAY_ELEMENT: usize = 174;
        pub const NEW_BOOLEAN_ARRAY: usize = 175;
        pub const GET_BOOLEAN_ARRAY_REGION: usize = 199;
        pub const SET_BOOLEAN_ARRAY_REGION: usize = 207;
        pub const NEW_CHAR_ARRAY: usize = 177;
        pub const GET_CHAR_ARRAY_REGION: usize = 201;
        pub const SET_CHAR_ARRAY_REGION: usize = 209;
        pub const NEW_SHORT_ARRAY: usize = 178;
        pub const GET_SHORT_ARRAY_REGION: usize = 202;
        pub const SET_SHORT_ARRAY_REGION: usize = 210;
        pub const NEW_LONG_ARRAY: usize = 180;
        pub const GET_LONG_ARRAY_REGION: usize = 204;
        pub const SET_LONG_ARRAY_REGION: usize = 212;
        pub const NEW_FLOAT_ARRAY: usize = 181;
        pub const GET_FLOAT_ARRAY_REGION: usize = 205;
        pub const SET_FLOAT_ARRAY_REGION: usize = 213;
        pub const NEW_DOUBLE_ARRAY: usize = 182;
        pub const GET_DOUBLE_ARRAY_REGION: usize = 206;
        pub const SET_DOUBLE_ARRAY_REGION: usize = 214;
        pub const NEW_BYTE_ARRAY: usize = 176;
        pub const NEW_INT_ARRAY: usize = 179;
        pub const GET_BYTE_ARRAY_REGION: usize = 200;
        pub const GET_INT_ARRAY_REGION: usize = 203;
        pub const SET_BYTE_ARRAY_REGION: usize = 208;
        pub const SET_INT_ARRAY_REGION: usize = 211;
        pub const REGISTER_NATIVES: usize = 215;
        pub const NEW_WEAK_GLOBAL_REF: usize = 226;
        pub const DELETE_WEAK_GLOBAL_REF: usize = 227;
        pub const EXCEPTION_CHECK: usize = 228;
        pub const GET_DIRECT_BUFFER_ADDRESS: usize = 230;
        pub const GET_DIRECT_BUFFER_CAPACITY: usize = 231;
    }
}

/// A `JavaVM*` of the shared session.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JavaVm(std::ptr::NonNull<c_void>);

// SAFETY: a `JavaVM*` is process-wide and valid on every thread (JNI spec).
unsafe impl Send for JavaVm {}
// SAFETY: as above; every entry is callable concurrently.
unsafe impl Sync for JavaVm {}

impl JavaVm {
    /// `None` for a null pointer.
    ///
    /// # Safety
    /// `vm` must be null or the in-kit shared session's `JavaVM*`, with its
    /// JNI tables and session pinned for the process. Safe code can copy
    /// and publish this wrapper and retain environments obtained from it.
    pub unsafe fn from_raw(vm: *mut c_void) -> Option<Self> {
        std::ptr::NonNull::new(vm).map(Self)
    }

    pub fn as_raw(self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// `GetEnv` for the calling thread; `None` unless it returns `JNI_OK`
    /// with a non-null env.
    pub fn get_env(self, version: i32) -> Option<JniEnv> {
        let mut env = std::ptr::null_mut();
        // SAFETY: `from_raw` contract; the out-param is a valid local.
        let rc = unsafe {
            function_slot::<unsafe extern "C" fn(*mut c_void, *mut *mut c_void, i32) -> i32>(
                self.as_raw(),
                slot::vm::GET_ENV,
            )(self.as_raw(), &mut env, version)
        };
        (rc == JNI_OK && !env.is_null()).then_some(JniEnv(env))
    }

    /// `AttachCurrentThread` with no attach arguments.
    pub fn attach_current_thread(self) -> Option<JniEnv> {
        let mut env = std::ptr::null_mut();
        // SAFETY: `from_raw` contract; null attach args are allowed by JNI.
        let rc = unsafe {
            function_slot::<unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut c_void) -> i32>(
                self.as_raw(),
                slot::vm::ATTACH_CURRENT_THREAD,
            )(self.as_raw(), &mut env, std::ptr::null_mut())
        };
        (rc == JNI_OK && !env.is_null()).then_some(JniEnv(env))
    }

    /// `DetachCurrentThread`; returns the JNI status code.
    pub fn detach_current_thread(self) -> i32 {
        // SAFETY: `from_raw` contract.
        unsafe {
            function_slot::<unsafe extern "C" fn(*mut c_void) -> i32>(self.as_raw(), slot::vm::DETACH_CURRENT_THREAD)(
                self.as_raw(),
            )
        }
    }
}

/// A `JNIEnv*` of the shared session for the calling thread.
///
/// `#[repr(transparent)]`, so a registered native takes it directly as its
/// first parameter (`extern "C" fn(env: JniEnv, this: JObject, ...)`): the VM
/// is the only caller and always passes its live env, which is what
/// [`JniEnv::from_raw`] would otherwise assert. Not `Send`: an env belongs to
/// one thread.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JniEnv(*mut c_void);

/// One call through `slot::env::$slot` with the given C signature.
macro_rules! call {
    ($self:ident, $slot:ident, fn($($ty:ty),*) $(-> $ret:ty)?, $($arg:expr),*) => {
        // SAFETY: `JniEnv::from_raw` contract (live in-kit session env); the
        // signature matches the JNI 1.6 entry at this slot.
        unsafe {
            function_slot::<unsafe extern "C" fn(*mut c_void $(, $ty)*) $(-> $ret)?>(
                $self.0,
                slot::env::$slot,
            )($self.0 $(, $arg)*)
        }
    };
}

/// All primitive region methods use initialized Rust POD slices and the shared
/// VM's exact-kind/bounds checks; no element pointer escapes a synchronous call.
macro_rules! primitive_array_api {
    ($new:ident,$get:ident,$set:ident,$ty:ty,$new_slot:ident,$get_slot:ident,$set_slot:ident)=> {
        pub fn $new(self,length:i32)->JObject {call!(self,$new_slot,fn(i32)->JObject,length)}
        pub fn $get(self,array:JObject,start:i32,out:&mut [$ty]) {
            let length=i32::try_from(out.len()).expect("JNI primitive region exceeds jsize");
            call!(self,$get_slot,fn(JObject,i32,i32,*mut $ty),array,start,length,out.as_mut_ptr())
        }
        pub fn $set(self,array:JObject,start:i32,values:&[$ty]) {
            let length=i32::try_from(values.len()).expect("JNI primitive region exceeds jsize");
            call!(self,$set_slot,fn(JObject,i32,i32,*const $ty),array,start,length,values.as_ptr())
        }
    };
}

impl JniEnv {
    /// # Safety
    /// `env` must be a non-null `JNIEnv*` of the in-kit shared session,
    /// attached to the calling thread, for as long as the wrapper is used.
    pub unsafe fn from_raw(env: *mut c_void) -> Self {
        debug_assert!(!env.is_null(), "JNIEnv is null");
        Self(env)
    }

    pub fn as_raw(self) -> *mut c_void {
        self.0
    }

    pub fn get_version(self) -> i32 {
        call!(self, GET_VERSION, fn() -> i32,)
    }

    /// # Safety
    /// `bytes` must hold a class image the session accepts; `loader` is a handle.
    pub unsafe fn define_class(self, name: &CStr, loader: JObject, bytes: &[u8]) -> JObject {
        call!(self, DEFINE_CLASS, fn(*const c_char, JObject, *const u8, i32) -> JObject,
            name.as_ptr(), loader, bytes.as_ptr(), bytes.len() as i32)
    }

    pub fn find_class(self, name: &CStr) -> JObject {
        call!(self, FIND_CLASS, fn(*const c_char) -> JObject, name.as_ptr())
    }

    pub fn to_reflected_method(self, class: JObject, method: JMethodId, is_static: bool) -> JObject {
        call!(self, TO_REFLECTED_METHOD, fn(JObject, JMethodId, u8) -> JObject, class, method, u8::from(is_static))
    }

    pub fn throw(self, throwable: JObject) -> i32 {
        call!(self, THROW, fn(JObject) -> i32, throwable)
    }

    pub fn throw_new(self, class: JObject, message: &CStr) -> i32 {
        call!(self, THROW_NEW, fn(JObject, *const c_char) -> i32, class, message.as_ptr())
    }

    pub fn exception_occurred(self) -> JObject {
        call!(self, EXCEPTION_OCCURRED, fn() -> JObject,)
    }

    pub fn exception_describe(self) {
        call!(self, EXCEPTION_DESCRIBE, fn(),)
    }

    pub fn exception_clear(self) {
        call!(self, EXCEPTION_CLEAR, fn(),)
    }

    /// `ExceptionCheck`: true while an exception is pending.
    pub fn exception_check(self) -> bool {
        call!(self, EXCEPTION_CHECK, fn() -> u8,) != 0
    }

    pub fn push_local_frame(self, capacity: i32) -> i32 {
        call!(self, PUSH_LOCAL_FRAME, fn(i32) -> i32, capacity)
    }

    pub fn pop_local_frame(self, result: JObject) -> JObject {
        call!(self, POP_LOCAL_FRAME, fn(JObject) -> JObject, result)
    }

    pub fn new_global_ref(self, object: JObject) -> JObject {
        call!(self, NEW_GLOBAL_REF, fn(JObject) -> JObject, object)
    }

    pub fn delete_global_ref(self, object: JObject) {
        call!(self, DELETE_GLOBAL_REF, fn(JObject), object)
    }

    pub fn delete_local_ref(self, object: JObject) {
        call!(self, DELETE_LOCAL_REF, fn(JObject), object)
    }

    pub fn is_same_object(self, a: JObject, b: JObject) -> bool {
        call!(self, IS_SAME_OBJECT, fn(JObject, JObject) -> u8, a, b) != 0
    }

    /// Whether an instance of `source` can be cast to `target` (JNI argument order).
    pub fn is_assignable_from(self, source: JObject, target: JObject) -> bool {
        call!(self, IS_ASSIGNABLE_FROM, fn(JObject, JObject) -> u8, source, target) != 0
    }

    pub fn new_local_ref(self, object: JObject) -> JObject {
        call!(self, NEW_LOCAL_REF, fn(JObject) -> JObject, object)
    }

    pub fn new_weak_global_ref(self, object: JObject) -> JObject {
        call!(self, NEW_WEAK_GLOBAL_REF, fn(JObject) -> JObject, object)
    }

    pub fn delete_weak_global_ref(self, weak: JObject) {
        call!(self, DELETE_WEAK_GLOBAL_REF, fn(JObject), weak)
    }

    pub fn alloc_object(self, class: JObject) -> JObject {
        call!(self, ALLOC_OBJECT, fn(JObject) -> JObject, class)
    }

    /// # Safety
    /// `args` must hold one correctly typed `jvalue` per parameter of `ctor`.
    pub unsafe fn new_object_a(self, class: JObject, ctor: JMethodId, args: *const JValue) -> JObject {
        call!(self, NEW_OBJECT_A, fn(JObject, JMethodId, *const JValue) -> JObject, class, ctor, args)
    }

    pub fn get_object_class(self, object: JObject) -> JObject {
        call!(self, GET_OBJECT_CLASS, fn(JObject) -> JObject, object)
    }

    pub fn is_instance_of(self, object: JObject, class: JObject) -> bool {
        call!(self, IS_INSTANCE_OF, fn(JObject, JObject) -> u8, object, class) != 0
    }

    pub fn get_method_id(self, class: JObject, name: &CStr, signature: &CStr) -> JMethodId {
        call!(self, GET_METHOD_ID, fn(JObject, *const c_char, *const c_char) -> JMethodId,
            class, name.as_ptr(), signature.as_ptr())
    }

    pub fn get_static_method_id(self, class: JObject, name: &CStr, signature: &CStr) -> JMethodId {
        call!(self, GET_STATIC_METHOD_ID, fn(JObject, *const c_char, *const c_char) -> JMethodId,
            class, name.as_ptr(), signature.as_ptr())
    }

    pub fn get_field_id(self, class: JObject, name: &CStr, signature: &CStr) -> JFieldId {
        call!(self, GET_FIELD_ID, fn(JObject, *const c_char, *const c_char) -> JFieldId,
            class, name.as_ptr(), signature.as_ptr())
    }

    pub fn get_static_field_id(self, class: JObject, name: &CStr, signature: &CStr) -> JFieldId {
        call!(self, GET_STATIC_FIELD_ID, fn(JObject, *const c_char, *const c_char) -> JFieldId,
            class, name.as_ptr(), signature.as_ptr())
    }

    /// # Safety
    /// `args` must hold one correctly typed `jvalue` per parameter of `method`.
    pub unsafe fn call_object_method_a(self, object: JObject, method: JMethodId, args: *const JValue) -> JObject {
        call!(self, CALL_OBJECT_METHOD_A, fn(JObject, JMethodId, *const JValue) -> JObject, object, method, args)
    }

    /// # Safety
    /// As [`Self::call_object_method_a`].
    pub unsafe fn call_boolean_method_a(self, object: JObject, method: JMethodId, args: *const JValue) -> u8 {
        call!(self, CALL_BOOLEAN_METHOD_A, fn(JObject, JMethodId, *const JValue) -> u8, object, method, args)
    }

    /// # Safety
    /// As [`Self::call_object_method_a`].
    pub unsafe fn call_int_method_a(self, object: JObject, method: JMethodId, args: *const JValue) -> i32 {
        call!(self, CALL_INT_METHOD_A, fn(JObject, JMethodId, *const JValue) -> i32, object, method, args)
    }
    /// # Safety
    /// `args` holds the method descriptor's initialized jvalues; null only for zero parameters.
    pub unsafe fn call_long_method_a(self, object: JObject, method: JMethodId, args: *const JValue) -> i64 {
        call!(self, CALL_LONG_METHOD_A, fn(JObject, JMethodId, *const JValue) -> i64, object, method, args)
    }
    /// # Safety
    /// As `call_long_method_a`, using the exact JNI float return ABI.
    pub unsafe fn call_float_method_a(self, object: JObject, method: JMethodId, args: *const JValue) -> f32 {
        call!(self, CALL_FLOAT_METHOD_A, fn(JObject, JMethodId, *const JValue) -> f32, object, method, args)
    }

    /// # Safety
    /// As [`Self::call_object_method_a`].
    pub unsafe fn call_void_method_a(self, object: JObject, method: JMethodId, args: *const JValue) {
        call!(self, CALL_VOID_METHOD_A, fn(JObject, JMethodId, *const JValue), object, method, args)
    }

    /// # Safety
    /// As [`Self::call_object_method_a`].
    pub unsafe fn call_static_object_method_a(self, class: JObject, method: JMethodId, args: *const JValue) -> JObject {
        call!(self, CALL_STATIC_OBJECT_METHOD_A, fn(JObject, JMethodId, *const JValue) -> JObject, class, method, args)
    }

    /// # Safety
    /// As [`Self::call_object_method_a`].
    pub unsafe fn call_static_boolean_method_a(self, class: JObject, method: JMethodId, args: *const JValue) -> u8 {
        call!(self, CALL_STATIC_BOOLEAN_METHOD_A, fn(JObject, JMethodId, *const JValue) -> u8, class, method, args)
    }

    /// # Safety
    /// As [`Self::call_object_method_a`].
    pub unsafe fn call_static_int_method_a(self, class: JObject, method: JMethodId, args: *const JValue) -> i32 {
        call!(self, CALL_STATIC_INT_METHOD_A, fn(JObject, JMethodId, *const JValue) -> i32, class, method, args)
    }

    /// # Safety
    /// As [`Self::call_object_method_a`].
    pub unsafe fn call_static_void_method_a(self, class: JObject, method: JMethodId, args: *const JValue) {
        call!(self, CALL_STATIC_VOID_METHOD_A, fn(JObject, JMethodId, *const JValue), class, method, args)
    }

    pub fn get_object_field(self, object: JObject, field: JFieldId) -> JObject {
        call!(self, GET_OBJECT_FIELD, fn(JObject, JFieldId) -> JObject, object, field)
    }

    pub fn get_boolean_field(self, object: JObject, field: JFieldId) -> u8 {
        call!(self, GET_BOOLEAN_FIELD, fn(JObject, JFieldId) -> u8, object, field)
    }

    pub fn get_int_field(self, object: JObject, field: JFieldId) -> i32 {
        call!(self, GET_INT_FIELD, fn(JObject, JFieldId) -> i32, object, field)
    }

    pub fn get_long_field(self, object: JObject, field: JFieldId) -> i64 {
        call!(self, GET_LONG_FIELD, fn(JObject, JFieldId) -> i64, object, field)
    }

    pub fn get_float_field(self, object: JObject, field: JFieldId) -> f32 {
        call!(self, GET_FLOAT_FIELD, fn(JObject, JFieldId) -> f32, object, field)
    }

    pub fn set_object_field(self, object: JObject, field: JFieldId, value: JObject) {
        call!(self, SET_OBJECT_FIELD, fn(JObject, JFieldId, JObject), object, field, value)
    }

    pub fn set_boolean_field(self, object: JObject, field: JFieldId, value: u8) {
        call!(self, SET_BOOLEAN_FIELD, fn(JObject, JFieldId, u8), object, field, value)
    }

    pub fn set_int_field(self, object: JObject, field: JFieldId, value: i32) {
        call!(self, SET_INT_FIELD, fn(JObject, JFieldId, i32), object, field, value)
    }

    pub fn set_long_field(self, object: JObject, field: JFieldId, value: i64) {
        call!(self, SET_LONG_FIELD, fn(JObject, JFieldId, i64), object, field, value)
    }

    pub fn set_float_field(self, object: JObject, field: JFieldId, value: f32) {
        call!(self, SET_FLOAT_FIELD, fn(JObject, JFieldId, f32), object, field, value)
    }

    pub fn get_static_object_field(self, class: JObject, field: JFieldId) -> JObject {
        call!(self, GET_STATIC_OBJECT_FIELD, fn(JObject, JFieldId) -> JObject, class, field)
    }

    pub fn get_static_int_field(self, class: JObject, field: JFieldId) -> i32 {
        call!(self, GET_STATIC_INT_FIELD, fn(JObject, JFieldId) -> i32, class, field)
    }

    pub fn set_static_object_field(self, class: JObject, field: JFieldId, value: JObject) {
        call!(self, SET_STATIC_OBJECT_FIELD, fn(JObject, JFieldId, JObject), class, field, value)
    }

    pub fn set_static_int_field(self, class: JObject, field: JFieldId, value: i32) {
        call!(self, SET_STATIC_INT_FIELD, fn(JObject, JFieldId, i32), class, field, value)
    }

    pub fn new_string(self, chars: &[u16]) -> JObject {
        call!(self, NEW_STRING, fn(*const u16, i32) -> JObject, chars.as_ptr(), chars.len() as i32)
    }

    pub fn get_string_length(self, string: JObject) -> i32 {
        call!(self, GET_STRING_LENGTH, fn(JObject) -> i32, string)
    }

    /// UTF-16 chars of `string` (null on failure); release with
    /// [`Self::release_string_chars`].
    pub fn get_string_chars(self, string: JObject) -> *const u16 {
        call!(self, GET_STRING_CHARS, fn(JObject, *mut u8) -> *const u16, string, std::ptr::null_mut())
    }

    /// # Safety
    /// `chars` must come from [`Self::get_string_chars`] for `string` and not be used afterwards.
    pub unsafe fn release_string_chars(self, string: JObject, chars: *const u16) {
        call!(self, RELEASE_STRING_CHARS, fn(JObject, *const u16), string, chars)
    }

    pub fn new_string_utf(self, text: &CStr) -> JObject {
        call!(self, NEW_STRING_UTF, fn(*const c_char) -> JObject, text.as_ptr())
    }

    /// Modified UTF-8 chars of `string` (null on failure); release with
    /// [`Self::release_string_utf_chars`].
    pub fn get_string_utf_chars(self, string: JObject) -> *const c_char {
        call!(self, GET_STRING_UTF_CHARS, fn(JObject, *mut u8) -> *const c_char, string, std::ptr::null_mut())
    }

    /// # Safety
    /// `chars` must come from [`Self::get_string_utf_chars`] for `string` and not be used afterwards.
    pub unsafe fn release_string_utf_chars(self, string: JObject, chars: *const c_char) {
        call!(self, RELEASE_STRING_UTF_CHARS, fn(JObject, *const c_char), string, chars)
    }

    pub fn get_array_length(self, array: JObject) -> i32 {
        call!(self, GET_ARRAY_LENGTH, fn(JObject) -> i32, array)
    }

    pub fn new_object_array(self, length: i32, class: JObject, initial: JObject) -> JObject {
        call!(self, NEW_OBJECT_ARRAY, fn(i32, JObject, JObject) -> JObject, length, class, initial)
    }

    pub fn get_object_array_element(self, array: JObject, index: i32) -> JObject {
        call!(self, GET_OBJECT_ARRAY_ELEMENT, fn(JObject, i32) -> JObject, array, index)
    }

    pub fn set_object_array_element(self, array: JObject, index: i32, value: JObject) {
        call!(self, SET_OBJECT_ARRAY_ELEMENT, fn(JObject, i32, JObject), array, index, value)
    }

    primitive_array_api!(new_boolean_array,get_boolean_array_region,set_boolean_array_region,u8,NEW_BOOLEAN_ARRAY,GET_BOOLEAN_ARRAY_REGION,SET_BOOLEAN_ARRAY_REGION);
    primitive_array_api!(new_byte_array,get_byte_array_region,set_byte_array_region,i8,NEW_BYTE_ARRAY,GET_BYTE_ARRAY_REGION,SET_BYTE_ARRAY_REGION);
    primitive_array_api!(new_char_array,get_char_array_region,set_char_array_region,u16,NEW_CHAR_ARRAY,GET_CHAR_ARRAY_REGION,SET_CHAR_ARRAY_REGION);
    primitive_array_api!(new_short_array,get_short_array_region,set_short_array_region,i16,NEW_SHORT_ARRAY,GET_SHORT_ARRAY_REGION,SET_SHORT_ARRAY_REGION);
    primitive_array_api!(new_int_array,get_int_array_region,set_int_array_region,i32,NEW_INT_ARRAY,GET_INT_ARRAY_REGION,SET_INT_ARRAY_REGION);
    primitive_array_api!(new_long_array,get_long_array_region,set_long_array_region,i64,NEW_LONG_ARRAY,GET_LONG_ARRAY_REGION,SET_LONG_ARRAY_REGION);
    primitive_array_api!(new_float_array,get_float_array_region,set_float_array_region,f32,NEW_FLOAT_ARRAY,GET_FLOAT_ARRAY_REGION,SET_FLOAT_ARRAY_REGION);
    primitive_array_api!(new_double_array,get_double_array_region,set_double_array_region,f64,NEW_DOUBLE_ARRAY,GET_DOUBLE_ARRAY_REGION,SET_DOUBLE_ARRAY_REGION);

    /// # Safety
    /// Every entry must carry valid C strings and a function with exactly the
    /// native prototype its JNI signature describes, alive while registered.
    pub unsafe fn register_natives(self, class: JObject, methods: &[NativeMethod]) -> i32 {
        call!(self, REGISTER_NATIVES, fn(JObject, *const NativeMethod, i32) -> i32,
            class, methods.as_ptr(), methods.len() as i32)
    }

    /// Start of a direct `ByteBuffer` (null when not direct).
    pub fn get_direct_buffer_address(self, buffer: JObject) -> *mut c_void {
        call!(self, GET_DIRECT_BUFFER_ADDRESS, fn(JObject) -> *mut c_void, buffer)
    }

    pub fn get_direct_buffer_capacity(self, buffer: JObject) -> i64 {
        call!(self, GET_DIRECT_BUFFER_CAPACITY, fn(JObject) -> i64, buffer)
    }
}


/// One JNI descriptor type; arrays and every `L...;` class are `Object`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JniType {
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Object,
    Void,
}

/// Parameter types and return type of a method descriptor such as
/// `(I[BLjava/lang/String;)V`; `None` when malformed.
pub fn parse_signature(signature: &CStr) -> Option<(Vec<JniType>, JniType)> {
    let bytes = signature.to_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let mut params = Vec::new();
    let mut at = 1;
    while *bytes.get(at)? != b')' {
        let (ty, next) = parse_type(bytes, at)?;
        if ty == JniType::Void {
            return None;
        }
        params.push(ty);
        at = next;
    }
    let (ret, end) = parse_type(bytes, at + 1)?;
    (end == bytes.len()).then_some((params, ret))
}

fn parse_type(bytes: &[u8], at: usize) -> Option<(JniType, usize)> {
    let mut end = at;
    while bytes.get(end) == Some(&b'[') {
        end += 1;
    }
    let array = end > at;
    let ty = match *bytes.get(end)? {
        b'Z' => JniType::Boolean,
        b'B' => JniType::Byte,
        b'C' => JniType::Char,
        b'S' => JniType::Short,
        b'I' => JniType::Int,
        b'J' => JniType::Long,
        b'F' => JniType::Float,
        b'D' => JniType::Double,
        b'V' if !array => JniType::Void,
        b'L' => {
            let len = bytes[end..].iter().position(|&b| b == b';')?;
            if len < 2 {
                return None;
            }
            end += len;
            JniType::Object
        }
        _ => return None,
    };
    Some((if array { JniType::Object } else { ty }, end + 1))
}

/// A method ID with the parameter count of the descriptor it was resolved
/// with. The in-kit VM resolves an ID only when the declared and requested
/// descriptors have the same parameter count, so `arity` is exactly how many
/// `jvalue`s a call reads; the `call_*` methods check `args.len()` against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    id: JMethodId,
    arity: usize,
}

// SAFETY: in-kit method IDs are tagged integers, valid on every thread.
unsafe impl Send for Method {}
// SAFETY: as above; a `Method` is immutable.
unsafe impl Sync for Method {}

impl Method {
    pub fn id(self) -> JMethodId {
        self.id
    }

    pub fn arity(self) -> usize {
        self.arity
    }
}

/// One fully initialised `jvalue` for a checked call. All eight bytes are
/// written whatever the member, so a type mismatch with the descriptor reads
/// a defined (if meaningless) value, never uninitialised memory.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Arg(JValue);

impl Arg {
    fn with(fill: impl FnOnce(&mut JValue)) -> Self {
        let mut value = JValue { j: 0 };
        fill(&mut value);
        Self(value)
    }

    pub fn object(value: JObject) -> Self {
        Self(JValue { l: value })
    }

    pub fn bool(value: bool) -> Self {
        Self::with(|v| v.z = u8::from(value))
    }

    pub fn byte(value: i8) -> Self {
        Self::with(|v| v.b = value)
    }

    pub fn char(value: u16) -> Self {
        Self::with(|v| v.c = value)
    }

    pub fn short(value: i16) -> Self {
        Self::with(|v| v.s = value)
    }

    pub fn int(value: i32) -> Self {
        Self::with(|v| v.i = value)
    }

    pub fn long(value: i64) -> Self {
        Self(JValue { j: value })
    }

    pub fn float(value: f32) -> Self {
        Self::with(|v| v.f = value)
    }

    pub fn double(value: f64) -> Self {
        Self(JValue { d: value })
    }
}

mod native_type {
    pub trait Sealed {}
}

/// Rust types a native may take for a JNI parameter. Sealed: an arbitrary
/// Rust type cannot claim a JNI scalar's ABI merely by choosing `TYPE`.
///
/// ```compile_fail
/// use rudroid_common::jni_env::{JniArgType, JniType};
/// #[derive(Clone, Copy)]
/// struct Forged([u64; 2]);
/// impl JniArgType for Forged { const TYPE: JniType = JniType::Int; }
/// ```
pub trait JniArgType: Copy + native_type::Sealed {
    const TYPE: JniType;
}

/// Rust types a native may return for a JNI return type. Sealed for the
/// same ABI guarantee as [`JniArgType`].
///
/// ```compile_fail
/// use rudroid_common::jni_env::{JniReturnType, JniType};
/// struct Forged([u64; 2]);
/// impl JniReturnType for Forged { const TYPE: JniType = JniType::Int; }
/// ```
pub trait JniReturnType: native_type::Sealed {
    const TYPE: JniType;
}

macro_rules! jni_types {
    ($($rust:ty => $jni:ident),* $(,)?) => {$(
        impl native_type::Sealed for $rust {}
        impl JniArgType for $rust {
            const TYPE: JniType = JniType::$jni;
        }
        impl JniReturnType for $rust {
            const TYPE: JniType = JniType::$jni;
        }
    )*};
}

jni_types! {
    u8 => Boolean, i8 => Byte, u16 => Char, i16 => Short, i32 => Int,
    i64 => Long, f32 => Float, f64 => Double, JObject => Object,
}

impl native_type::Sealed for () {}
impl JniReturnType for () {
    const TYPE: JniType = JniType::Void;
}

/// A checked `RegisterNatives` entry. The constructors take the native as an
/// `extern "C" fn(JniEnv, JObject, ...)` and compare its Rust parameter and
/// return types with the descriptor, so the VM always calls it through its
/// real prototype; a mismatch panics at registration. Names, descriptors and
/// functions are `'static` (kit libraries are never unloaded).
#[repr(transparent)]
pub struct Native(NativeMethod);

macro_rules! native_constructors {
    ($($name:ident($($arg:ident),*);)*) => {$(
        /// Checked entry for a native taking `env`, `this`/`class` and the
        /// listed JNI parameters.
        pub fn $name<$($arg: JniArgType,)* R: JniReturnType>(
            name: &'static CStr,
            signature: &'static CStr,
            function: extern "C" fn(JniEnv, JObject $(, $arg)*) -> R,
        ) -> Self {
            Self::checked(name, signature, &[$(<$arg as JniArgType>::TYPE),*], R::TYPE, function as *mut c_void)
        }
    )*};
}

impl Native {
    native_constructors! {
        new0();
        new1(A);
        new2(A, B);
        new3(A, B, C);
        new4(A, B, C, D);
        new5(A, B, C, D, E);
        new6(A, B, C, D, E, F);
        new7(A, B, C, D, E, F, G);
        new8(A, B, C, D, E, F, G, H);
        new9(A, B, C, D, E, F, G, H, I);
        new10(A, B, C, D, E, F, G, H, I, J);
    }

    fn checked(
        name: &'static CStr,
        signature: &'static CStr,
        params: &[JniType],
        ret: JniType,
        function: *mut c_void,
    ) -> Self {
        let parsed = parse_signature(signature);
        assert!(
            parsed.as_ref().is_some_and(|(p, r)| p.as_slice() == params && *r == ret),
            "native {name:?} {signature:?} does not match its Rust prototype {params:?} -> {ret:?}"
        );
        Self(NativeMethod { name: name.as_ptr(), signature: signature.as_ptr(), fn_ptr: function })
    }

    pub fn name(&self) -> &'static CStr {
        // SAFETY: built only from a `&'static CStr` in `checked`.
        unsafe { CStr::from_ptr(self.0.name) }
    }
}

impl JniEnv {
    /// `GetMethodID` keeping the descriptor's parameter count; `None` when
    /// the descriptor is malformed or the method is unknown (the VM then has
    /// `NoSuchMethodError` pending).
    pub fn method(self, class: JObject, name: &CStr, signature: &CStr) -> Option<Method> {
        let arity = parse_signature(signature)?.0.len();
        let id = self.get_method_id(class, name, signature);
        (!id.is_null()).then_some(Method { id, arity })
    }

    /// `GetStaticMethodID`, as [`Self::method`].
    pub fn static_method(self, class: JObject, name: &CStr, signature: &CStr) -> Option<Method> {
        let arity = parse_signature(signature)?.0.len();
        let id = self.get_static_method_id(class, name, signature);
        (!id.is_null()).then_some(Method { id, arity })
    }

    pub fn call_object(self, object: JObject, method: Method, args: &[Arg]) -> JObject {
        let args = checked_args(method, args);
        // SAFETY: `args` holds exactly the method's parameter count of initialised jvalues.
        unsafe { self.call_object_method_a(object, method.id, args) }
    }

    pub fn call_boolean(self, object: JObject, method: Method, args: &[Arg]) -> bool {
        let args = checked_args(method, args);
        // SAFETY: as `call_object`.
        unsafe { self.call_boolean_method_a(object, method.id, args) != 0 }
    }

    pub fn call_int(self, object: JObject, method: Method, args: &[Arg]) -> i32 {
        let args = checked_args(method, args);
        // SAFETY: as `call_object`.
        unsafe { self.call_int_method_a(object, method.id, args) }
    }
    pub fn call_long(self, object: JObject, method: Method, args: &[Arg]) -> i64 {
        let args=checked_args(method,args);
        // SAFETY: checked_args supplies precisely the initialized descriptor argument count.
        unsafe {self.call_long_method_a(object,method.id,args)}
    }
    pub fn call_float(self, object: JObject, method: Method, args: &[Arg]) -> f32 {
        let args=checked_args(method,args);
        // SAFETY: as call_long; no caller pointer survives this call.
        unsafe {self.call_float_method_a(object,method.id,args)}
    }

    pub fn call_void(self, object: JObject, method: Method, args: &[Arg]) {
        let args = checked_args(method, args);
        // SAFETY: as `call_object`.
        unsafe { self.call_void_method_a(object, method.id, args) }
    }

    pub fn call_static_object(self, class: JObject, method: Method, args: &[Arg]) -> JObject {
        let args = checked_args(method, args);
        // SAFETY: as `call_object`.
        unsafe { self.call_static_object_method_a(class, method.id, args) }
    }

    pub fn call_static_boolean(self, class: JObject, method: Method, args: &[Arg]) -> bool {
        let args = checked_args(method, args);
        // SAFETY: as `call_object`.
        unsafe { self.call_static_boolean_method_a(class, method.id, args) != 0 }
    }

    pub fn call_static_int(self, class: JObject, method: Method, args: &[Arg]) -> i32 {
        let args = checked_args(method, args);
        // SAFETY: as `call_object`.
        unsafe { self.call_static_int_method_a(class, method.id, args) }
    }

    pub fn call_static_long(self, class: JObject, method: Method, args: &[Arg]) -> i64 {
        let args = checked_args(method, args);
        call!(self, CALL_STATIC_LONG_METHOD_A, fn(JObject, JMethodId, *const JValue) -> i64, class, method.id, args)
    }

    pub fn call_static_void(self, class: JObject, method: Method, args: &[Arg]) {
        let args = checked_args(method, args);
        // SAFETY: as `call_object`.
        unsafe { self.call_static_void_method_a(class, method.id, args) }
    }

    /// `NewObjectA` with a constructor from [`Self::method`] (`<init>`).
    pub fn new_object(self, class: JObject, ctor: Method, args: &[Arg]) -> JObject {
        let args = checked_args(ctor, args);
        // SAFETY: as `call_object`.
        unsafe { self.new_object_a(class, ctor.id, args) }
    }

    /// Copy of `string` as UTF-16 units; `None` for null or on failure.
    pub fn string_utf16(self, string: JObject) -> Option<Vec<u16>> {
        self.with_string_utf16(string, <[u16]>::to_vec)
    }

    /// Read UTF-16 without allocating an owned copy. The VM borrow is released
    /// on return or unwind; the callback cannot return a borrowed Rust view.
    pub fn with_string_utf16<R>(self, string: JObject, read: impl FnOnce(&[u16]) -> R) -> Option<R> {
        if string.is_null() {
            return None;
        }
        let string = self.new_local_ref(string);
        if string.is_null() { return None; }
        let root = LocalRef { env: self, object: string };
        let len = usize::try_from(self.get_string_length(string)).ok()?;
        let chars = self.get_string_chars(string);
        if chars.is_null() {
            return None;
        }
        let _borrow = StringBorrow { root, chars: StringView::Utf16(chars) };
        // SAFETY: immutable `len` units remain pinned until StringBorrow drops,
        // including callback unwind; R is independent of the view's lifetime.
        Some(read(unsafe { std::slice::from_raw_parts(chars, len) }))
    }

    /// Copy of `string` as a Rust string (unpaired surrogates become U+FFFD).
    pub fn string(self, string: JObject) -> Option<String> {
        self.string_utf16(string).map(|units| String::from_utf16_lossy(&units))
    }

    /// Copy of `string`'s modified UTF-8 (`GetStringUTFChars`).
    pub fn string_utf(self, string: JObject) -> Option<CString> {
        self.with_string_utf(string, CStr::to_owned)
    }

    /// Read modified UTF-8 without copying; release is automatic on every exit.
    pub fn with_string_utf<R>(self, string: JObject, read: impl FnOnce(&CStr) -> R) -> Option<R> {
        if string.is_null() {
            return None;
        }
        let string = self.new_local_ref(string);
        if string.is_null() { return None; }
        let root = LocalRef { env: self, object: string };
        let chars = self.get_string_utf_chars(string);
        if chars.is_null() {
            return None;
        }
        let _borrow = StringBorrow { root, chars: StringView::Utf8(chars) };
        // SAFETY: immutable NUL-terminated VM storage stays pinned by this
        // scoped borrow; release runs after the callback or during unwind.
        Some(read(unsafe { CStr::from_ptr(chars) }))
    }

    /// Copy the first `out.len()` bytes of a direct `ByteBuffer`; false when
    /// `buffer` is not direct or holds fewer bytes.
    pub fn read_direct_buffer(self, buffer: JObject, out: &mut [u8]) -> bool {
        // Promotion retains under the source-handle lock. A weak reference
        // or concurrent DeleteGlobalRef cannot free the storage mid-copy.
        let buffer = self.new_local_ref(buffer);
        if buffer.is_null() { return false; }
        let _root = LocalRef { env: self, object: buffer };
        let address = self.get_direct_buffer_address(buffer).cast::<u8>();
        let capacity = self.get_direct_buffer_capacity(buffer);
        if address.is_null() || usize::try_from(capacity).map_or(true, |capacity| capacity < out.len()) {
            return false;
        }
        // SAFETY: the VM reports `capacity` bytes at `address`, kept alive by
        // the retained local reference to `buffer` for this call; no Rust reference
        // to the VM-owned storage is created.
        unsafe { std::ptr::copy_nonoverlapping(address, out.as_mut_ptr(), out.len()) };
        true
    }

    /// `RegisterNatives` with checked entries.
    pub fn register(self, class: JObject, natives: &[Native]) -> i32 {
        let count = i32::try_from(natives.len()).expect("JNI native count");
        call!(self, REGISTER_NATIVES, fn(JObject, *const Native, i32) -> i32, class, natives.as_ptr(), count)
    }

    /// `FindClass(name)` then [`Self::register`]; -1 when the class is missing.
    pub fn register_class(self, name: &CStr, natives: &[Native]) -> i32 {
        let class = self.find_class(name);
        if class.is_null() {
            return -1;
        }
        let result = self.register(class, natives);
        self.delete_local_ref(class);
        result
    }
}

#[derive(Clone, Copy)]
enum StringView { Utf8(*const c_char), Utf16(*const u16) }
struct StringBorrow { root: LocalRef, chars: StringView }
impl Drop for StringBorrow {
    fn drop(&mut self) {
        // SAFETY: constructed privately only after the matching successful
        // GetString* borrow. No view outlives this guard and each pin releases once.
        unsafe { match self.chars {
            StringView::Utf8(chars) => self.root.env.release_string_utf_chars(self.root.object, chars),
            StringView::Utf16(chars) => self.root.env.release_string_chars(self.root.object, chars),
        } }
    }
}

pub(crate) struct LocalRef {
    pub(crate) env: JniEnv,
    pub(crate) object: JObject,
}
impl Drop for LocalRef {
    fn drop(&mut self) { self.env.delete_local_ref(self.object); }
}

fn checked_args(method: Method, args: &[Arg]) -> *const JValue {
    assert_eq!(
        args.len(),
        method.arity,
        "JNI call passes {} arguments to a {}-parameter method",
        args.len(),
        method.arity
    );
    args.as_ptr().cast()
}

#[cfg(test)]
#[path = "../tests/unit/jni_env.rs"]
mod tests;
