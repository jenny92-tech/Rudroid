//! INPUT:  the ticket-53 guest spy fixture exports (`project/unity_loader/tests/fixtures/rudroid53_spy.c`)
//! OUTPUT:  spy export names, event kinds and the C event layout
//! POS:    optional fixture observation surface; a real game never exports these symbols
//!
//! The spy stands in for the guest's registered native bodies in the loader
//! integration tests. A drift from the C fixture breaks those tests loudly.
#![forbid(unsafe_code)]

use std::ffi::CStr;

pub const SIGNAL_VSYNC: &CStr = c"rudroid_spy_signal_vsync";
pub const EVENT_COUNT: &CStr = c"rudroid_spy_event_count";
pub const EVENT_AT: &CStr = c"rudroid_spy_event_at";

/// Event kinds recorded by the spy (mirrors `SPY_*` in the C fixture).
pub const KIND_JNI_ONLOAD: i32 = 1;
pub const KIND_NATIVE_LOADER_LOAD: i32 = 2;
pub const KIND_INIT_JNI_CTX: i32 = 3;
pub const KIND_INIT_JNI_CTX_INT: i32 = 4;
pub const KIND_RECREATE_GFX: i32 = 5;
pub const KIND_RESTART_INDICATOR: i32 = 6;
pub const KIND_SURFACE_CHANGED: i32 = 7;
pub const KIND_FOCUS_CHANGED: i32 = 8;
pub const KIND_RESUME: i32 = 9;
pub const KIND_RENDER: i32 = 10;
pub const KIND_SIGNAL_VSYNC: i32 = 11;

/// C layout of `struct RudroidSpyEvent` (int + 2×long long + char[48]).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub kind: i32,
    pub a: i64,
    pub b: i64,
    pub text: [u8; 48],
}

impl Event {
    pub fn text_lossy(&self) -> String {
        let len = self.text.iter().position(|b| *b == 0).unwrap_or(48);
        String::from_utf8_lossy(&self.text[..len]).into_owned()
    }
}

/// Stable name for the host-log echo (the call-sequence evidence).
pub fn kind_name(kind: i32) -> &'static str {
    match kind {
        KIND_JNI_ONLOAD => "JNI_ONLOAD",
        KIND_NATIVE_LOADER_LOAD => "NATIVE_LOADER_LOAD",
        KIND_INIT_JNI_CTX => "INIT_JNI_CTX",
        KIND_INIT_JNI_CTX_INT => "INIT_JNI_CTX_INT",
        KIND_RECREATE_GFX => "RECREATE_GFX",
        KIND_RESTART_INDICATOR => "RESTART_INDICATOR",
        KIND_SURFACE_CHANGED => "SURFACE_CHANGED",
        KIND_FOCUS_CHANGED => "FOCUS_CHANGED",
        KIND_RESUME => "RESUME",
        KIND_RENDER => "RENDER",
        KIND_SIGNAL_VSYNC => "SIGNAL_VSYNC",
        12 => "PAUSE",
        13 => "DONE",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
#[path = "../tests/unit/spy.rs"]
mod tests;
