//! INPUT: complete JNI descriptors, checked shared method IDs and native jvalue arrays
//! OUTPUT: exact byte/char/short/double return ABIs through one typed dispatch macro
//! POS: native boundary; classification is safe, casts reuse the VM's native_call authority
use super::{abi, Dispatch, JValue, MethodValue};
use std::ffi::c_void;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Return {
    Byte,
    Char,
    Short,
    Double,
}
impl Return {
    pub(super) fn kind(self) -> abi::MethodKind {
        match self {
            Self::Byte => abi::MethodKind::Byte,
            Self::Char => abi::MethodKind::Char,
            Self::Short => abi::MethodKind::Short,
            Self::Double => abi::MethodKind::Double,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Argument {
    None,
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
}

/// Every accepted argument/return pair has an exact native prototype below.
/// Larger/mixed/reference signatures stay explicit unavailable bindings.
pub(super) fn classify(signature: &str, address: usize) -> Option<MethodValue> {
    let (args, returned) = signature.strip_prefix('(')?.split_once(')')?;
    let returned = match returned {
        "B" => Return::Byte,
        "C" => Return::Char,
        "S" => Return::Short,
        "D" => Return::Double,
        _ => return None,
    };
    let args = match args {
        "" => Argument::None,
        "Z" => Argument::Boolean,
        "B" => Argument::Byte,
        "C" => Argument::Char,
        "S" => Argument::Short,
        "I" => Argument::Int,
        "J" => Argument::Long,
        "F" => Argument::Float,
        "D" => Argument::Double,
        _ => return None,
    };
    Some(MethodValue::NativeScalar(address, returned, args))
}

macro_rules! scalar_dispatch {
    ($helper:ident, $instance:ident, $static:ident, $kind:ident, $ty:ty) => {
        /// # Safety
        /// The JNI caller supplies readable arguments matching the method descriptor;
        /// registered code obeys that exact ABI and remains mapped through the call.
        pub(super) unsafe fn $helper(env: *mut c_void, receiver: *mut c_void, method: *mut c_void,
            args: *const JValue, dispatch: Dispatch) -> $ty {
            let Some(s) = super::session() else { return 0 as $ty; };
            let Some(MethodValue::NativeScalar(address, _, shape)) = s.call_checked_dispatch(
                receiver, method, abi::MethodKind::$kind, stringify!($instance), dispatch) else { return 0 as $ty; };
            if shape != Argument::None && args.is_null() {
                s.set_pending("java/lang/IllegalArgumentException", "missing native scalar arguments");
                return 0 as $ty;
            }
            // SAFETY: call_checked verified the exact return kind; classify fixes
            // each argument union member to its C ABI. No lock survives this call.
            match shape {
                Argument::None => native_call!(address, fn(*mut c_void, *mut c_void) -> $ty; env, receiver),
                Argument::Boolean => native_call!(address, fn(*mut c_void, *mut c_void, u8) -> $ty; env, receiver, (*args).z),
                Argument::Byte => native_call!(address, fn(*mut c_void, *mut c_void, i8) -> $ty; env, receiver, (*args).b),
                Argument::Char => native_call!(address, fn(*mut c_void, *mut c_void, u16) -> $ty; env, receiver, (*args).c),
                Argument::Short => native_call!(address, fn(*mut c_void, *mut c_void, i16) -> $ty; env, receiver, (*args).s),
                Argument::Int => native_call!(address, fn(*mut c_void, *mut c_void, i32) -> $ty; env, receiver, (*args).i),
                Argument::Long => native_call!(address, fn(*mut c_void, *mut c_void, i64) -> $ty; env, receiver, (*args).j),
                Argument::Float => native_call!(address, fn(*mut c_void, *mut c_void, f32) -> $ty; env, receiver, (*args).f),
                Argument::Double => native_call!(address, fn(*mut c_void, *mut c_void, f64) -> $ty; env, receiver, (*args).d),
            }
        }
        /// # Safety
        /// Same descriptor/code/argument lifetime contract as the shared dispatcher.
        pub unsafe extern "C" fn $instance(env: *mut c_void, object: *mut c_void,
            method: *mut c_void, args: *const JValue) -> $ty { $helper(env, object, method, args, Dispatch::Virtual) }
        /// # Safety
        /// Same contract; `class` must belong to the live method authority.
        pub unsafe extern "C" fn $static(env: *mut c_void, class: *mut c_void,
            method: *mut c_void, args: *const JValue) -> $ty { $helper(env, class, method, args, Dispatch::Static) }
    };
}
scalar_dispatch!(
    call_byte,
    call_byte_method_a,
    call_static_byte_method_a,
    Byte,
    i8
);
scalar_dispatch!(
    call_char,
    call_char_method_a,
    call_static_char_method_a,
    Char,
    u16
);
scalar_dispatch!(
    call_short,
    call_short_method_a,
    call_static_short_method_a,
    Short,
    i16
);
scalar_dispatch!(
    call_double,
    call_double_method_a,
    call_static_double_method_a,
    Double,
    f64
);

#[cfg(test)]
#[path = "../tests/unit/scalar_calls.rs"]
mod tests;
