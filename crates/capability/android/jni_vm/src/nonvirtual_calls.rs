//! INPUT: shared receiver/class/method authority and exact JNI jvalue prototypes
//! OUTPUT: typed nonvirtual A entries and typed unavailable DefineClass response
//! POS: validates class provenance before reusing the VM's existing call boundaries
use super::{abi, shims, Dispatch, JValue};
use std::ffi::{c_char, c_void};

macro_rules! nonvirtual_entry {
    ($entry:ident, $helper:path, $returned:ty) => {
        /// # Safety
        /// JNI callers retain live references and provide argument storage
        /// matching the registered exact descriptor. Native code stays mapped
        /// through the synchronous call, as for ordinary A dispatch.
        pub unsafe extern "C" fn $entry(
            env: *mut c_void,
            object: *mut c_void,
            class: *mut c_void,
            method: *mut c_void,
            args: *const JValue,
        ) -> $returned {
            // SAFETY: the shared dispatcher checks receiver/class/method/kind
            // before any native cast, and reads only descriptor-matched args.
            $helper(env, object, method, args, Dispatch::Nonvirtual(class))
        }
    };
}
nonvirtual_entry!(call_nonvirtual_object_a, shims::call_object_a, *mut c_void);
nonvirtual_entry!(call_nonvirtual_boolean_a, shims::call_boolean_a, u8);
nonvirtual_entry!(call_nonvirtual_byte_a, super::scalar_calls::call_byte, i8);
nonvirtual_entry!(call_nonvirtual_char_a, super::scalar_calls::call_char, u16);
nonvirtual_entry!(
    call_nonvirtual_short_a,
    super::scalar_calls::call_short,
    i16
);
nonvirtual_entry!(call_nonvirtual_int_a, shims::call_int_a, i32);
nonvirtual_entry!(call_nonvirtual_long_a, shims::call_long, i64);
nonvirtual_entry!(call_nonvirtual_float_a, shims::call_float, f32);
nonvirtual_entry!(
    call_nonvirtual_double_a,
    super::scalar_calls::call_double,
    f64
);
nonvirtual_entry!(call_nonvirtual_void_a, shims::call_void_a, ());

// No JVM bytecode execution is supplied. Keep the full JNI ABI while failing
// explicitly; never inspect untrusted class bytes or manufacture success.
pub extern "C" fn define_class_unavailable(
    _env: *mut c_void,
    _name: *const c_char,
    _loader: *mut c_void,
    _bytes: *const i8,
    _length: i32,
) -> *mut c_void {
    if let Some(session) = super::session() {
        if !session.blocked_by_pending("DefineClass") {
            session.set_pending(
                "java/lang/UnsupportedOperationException",
                "DefineClass bytecode execution is unavailable",
            );
        }
    }
    std::ptr::null_mut()
}

pub(crate) fn bind_nonvirtual(table: &mut [usize; abi::ENV_SLOT_COUNT]) {
    use abi::env_slot as slot;
    table[slot::DefineClass] = define_class_unavailable as *const c_void as usize;
    table[slot::CallNonvirtualObjectMethodA] = call_nonvirtual_object_a as *const c_void as usize;
    table[slot::CallNonvirtualBooleanMethodA] = call_nonvirtual_boolean_a as *const c_void as usize;
    table[slot::CallNonvirtualByteMethodA] = call_nonvirtual_byte_a as *const c_void as usize;
    table[slot::CallNonvirtualCharMethodA] = call_nonvirtual_char_a as *const c_void as usize;
    table[slot::CallNonvirtualShortMethodA] = call_nonvirtual_short_a as *const c_void as usize;
    table[slot::CallNonvirtualIntMethodA] = call_nonvirtual_int_a as *const c_void as usize;
    table[slot::CallNonvirtualLongMethodA] = call_nonvirtual_long_a as *const c_void as usize;
    table[slot::CallNonvirtualFloatMethodA] = call_nonvirtual_float_a as *const c_void as usize;
    table[slot::CallNonvirtualDoubleMethodA] = call_nonvirtual_double_a as *const c_void as usize;
    table[slot::CallNonvirtualVoidMethodA] = call_nonvirtual_void_a as *const c_void as usize;
}

#[cfg(test)]
#[path = "../tests/unit/nonvirtual_calls.rs"]
mod tests;
