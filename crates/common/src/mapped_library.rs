//! INPUT: pinned dynamic symbol and selected library path
//! OUTPUT: symbol ownership check by device and inode
//! POS: positive per-image kit marker validation

#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    ffi::{c_void, CStr, OsStr},
    fs,
    mem::MaybeUninit,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

/// # Safety
/// The module containing the symbol stays loaded until this call returns.
/// The selected kit and its loaded dependencies must stay unchanged.
pub unsafe fn defines(selected: &Path, symbol: *const c_void) -> bool {
    let mut info = MaybeUninit::<libc::Dl_info>::uninit();
    // SAFETY: dladdr accepts an address for lookup and initializes its output on success.
    if unsafe { libc::dladdr(symbol, info.as_mut_ptr()) } == 0 {
        return false;
    }
    // SAFETY: dladdr succeeded and the provider stays loaded through the path comparison.
    let name = unsafe { info.assume_init().dli_fname };
    if name.is_null() {
        return false;
    }
    // SAFETY: dladdr's module name is a terminated path owned by the dynamic loader.
    let owner = Path::new(OsStr::from_bytes(
        unsafe { CStr::from_ptr(name) }.to_bytes(),
    ));
    let (Ok(selected), Ok(owner)) = (fs::metadata(selected), fs::metadata(owner)) else {
        return false;
    };
    selected.dev() == owner.dev() && selected.ino() == owner.ino()
}
