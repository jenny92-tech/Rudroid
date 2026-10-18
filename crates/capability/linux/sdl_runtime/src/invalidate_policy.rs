//! INPUT:  immutable Host API configuration and canonical guest GLES symbol names
//! OUTPUT:  opted-in no-op framebuffer invalidation entries; native fallback when disabled
//! POS:    implements the reference hint policy without reading or retaining guest pointers
#![forbid(unsafe_code)]
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};
use rudroid_common::host_api::HostApiV0;

static ENABLED: OnceLock<bool> = OnceLock::new();
static CALLS: AtomicU64 = AtomicU64::new(0);

pub(super) fn configure(api: &HostApiV0) -> bool {
    let enabled = api.config_bool(c"gpu.disableFramebufferInvalidate", false);
    *ENABLED.get_or_init(|| enabled) == enabled
}

pub(super) fn configured() -> bool { ENABLED.get().copied().unwrap_or(false) }

pub(super) fn symbol(enabled: bool, name: &[u8]) -> usize {
    if !enabled { return 0; }
    match name {
        b"glInvalidateFramebuffer" | b"glDiscardFramebufferEXT" => discard as usize,
        b"glInvalidateSubFramebuffer" => discard_sub as usize,
        _ => 0,
    }
}

fn discarded() {
    let count = CALLS.fetch_add(1, Ordering::Relaxed);
    // Bounded startup evidence; never log per frame or touch native GL state.
    if count < 3 { eprintln!("[SDL] framebuffer invalidation hint suppressed: total={}", count + 1); }
}

// The opt-in reference policy deliberately ignores the entire hint, including
// invalid arguments. No attachment pointer is read, written or retained.
// Pinned code remains callable without borrowing a session or driver object.
extern "C" fn discard(_target: u32, _count: i32, _attachments: *const u32) { discarded(); }
extern "C" fn discard_sub(_target: u32, _count: i32, _attachments: *const u32,
                          _x: i32, _y: i32, _w: i32, _h: i32) { discarded(); }

#[cfg(test)]
#[path = "../tests/unit/invalidate_policy.rs"]
mod tests;
