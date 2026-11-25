//! INPUT: the active SDL graphics backend and guest NDK window calls
//! OUTPUT: opaque reference-counted window handles and fixed buffer geometry
//! POS: shared ANativeWindow C ABI for GLES and Vulkan
#![deny(unsafe_op_in_unsafe_fn)]

use std::{ffi::c_void, ptr, sync::Arc};

type Handle = *mut c_void;
const NO_DEVICE: i32 = -19;
const UNSUPPORTED: i32 = -95;

/// Each backend exposes one primary SDL window. Every guest lease points at
/// the Arc allocation used for that backend's surface identity.
pub(super) trait Backend: Sized {
    const FORMAT: Option<i32>;
    const NULL_WINDOW: i32;
    const INVALID_GEOMETRY: i32;
    fn active() -> Option<Arc<Self>>;
    fn size(&self) -> (i32, i32);
    fn trace_surface(&self, _present: bool) {}
    fn trace_geometry(&self, _width: i32, _height: i32, _format: i32, _result: i32) {}
}

pub(super) fn resolve<B: Backend>(name: &[u8]) -> usize {
    match name {
        b"ANativeWindow_fromSurface" => from_surface::<B> as usize,
        b"ANativeWindow_acquire" => acquire::<B> as usize,
        b"ANativeWindow_release" => release::<B> as usize,
        b"ANativeWindow_getWidth" => width::<B> as usize,
        b"ANativeWindow_getHeight" => height::<B> as usize,
        b"ANativeWindow_getFormat" if B::FORMAT.is_some() => format::<B> as usize,
        b"ANativeWindow_setBuffersGeometry" => geometry::<B> as usize,
        _ => 0,
    }
}

pub(super) extern "C" fn from_surface<B: Backend>(_: Handle, surface: Handle) -> Handle {
    let Some(owner) = B::active() else {
        return ptr::null_mut();
    };
    owner.trace_surface(!surface.is_null());
    if surface.is_null() {
        return ptr::null_mut();
    }
    Arc::into_raw(owner).cast_mut().cast()
}

/// # Safety
/// Caller holds a lease issued by this backend throughout the call.
pub(super) unsafe extern "C" fn acquire<B: Backend>(window: Handle) {
    if !window.is_null() {
        // SAFETY: the owned reference excludes a concurrent last release.
        unsafe { Arc::increment_strong_count(window.cast::<B>()) };
    }
}

/// # Safety
/// Caller transfers precisely one reference from fromSurface or acquire.
pub(super) unsafe extern "C" fn release<B: Backend>(window: Handle) {
    if !window.is_null() {
        // SAFETY: consuming the reference remains valid after backend shutdown.
        unsafe { Arc::decrement_strong_count(window.cast::<B>()) };
    }
}

fn owner<B: Backend>(window: Handle) -> Result<Arc<B>, i32> {
    if window.is_null() {
        return Err(B::NULL_WINDOW);
    }
    B::active()
        .filter(|active| ptr::eq(Arc::as_ptr(active), window.cast()))
        .ok_or(NO_DEVICE)
}

pub(super) extern "C" fn width<B: Backend>(window: Handle) -> i32 {
    owner::<B>(window).map_or_else(|err| err, |g| g.size().0)
}
pub(super) extern "C" fn height<B: Backend>(window: Handle) -> i32 {
    owner::<B>(window).map_or_else(|err| err, |g| g.size().1)
}
extern "C" fn format<B: Backend>(window: Handle) -> i32 {
    owner::<B>(window).map_or_else(|err| err, |_| B::FORMAT.unwrap_or(UNSUPPORTED))
}
pub(super) extern "C" fn geometry<B: Backend>(window: Handle, w: i32, h: i32, format: i32) -> i32 {
    let g = match owner::<B>(window) {
        Ok(g) => g,
        Err(err) => return err,
    };
    let result = if w < 0 || h < 0 || (w == 0) != (h == 0) || format < 0 {
        B::INVALID_GEOMETRY
    } else if (format != 0 && Some(format) != B::FORMAT) || (w != 0 && (w, h) != g.size()) {
        UNSUPPORTED
    } else {
        0
    };
    g.trace_geometry(w, h, format, result);
    result
}

#[cfg(test)]
#[path = "../tests/unit/native_window.rs"]
mod tests;
