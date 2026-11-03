//! INPUT: shared byte POD slices
//! OUTPUT: borrowed signed/unsigned byte views
//! POS: two audited byte-view adapters; all array ownership lives in primitive_arrays
/// Reinterpret byte POD under the same shared borrow, without a copy.
pub(super) fn unsigned(bytes: &[i8]) -> &[u8] {
    // SAFETY: i8/u8 have identical size/alignment and accept every bit pattern;
    // the output keeps the input's shared borrow and lifetime.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len()) }
}

/// Reinterpret byte POD under the same exclusive borrow, without a copy.
pub(super) fn unsigned_mut(bytes: &mut [i8]) -> &mut [u8] {
    // SAFETY: i8/u8 have identical size/alignment and accept every bit pattern;
    // the output keeps the input's exclusive borrow and lifetime.
    unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr().cast(), bytes.len()) }
}

