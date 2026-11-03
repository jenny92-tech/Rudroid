//! INPUT:  fixed guest module identities and Rust runtime lifecycle conventions
//! OUTPUT:  ModuleKind identities, names and numeric validation
//! POS:    shares mapped-module and mapping-free lifecycle identities across the kit
//! Guest module kinds, mirroring the reference `RudroidModuleKind`.
//!
//! Addons subscribe to specific kinds (e.g. the burst addon waits for
//! `Unity` before mapping its library); the guest bring-up emits these
//! events in reference order.
#![forbid(unsafe_code)]

/// Guest module identity. Discriminants 0..=8 match `RudroidModuleKind` in the
/// read-only reference (`plugin_api.h`).
///
/// `Il2cppReady` is a Rust-side runtime lifecycle pseudo-event (ticket 78):
/// fired by the `il2cpp_hook` capability through
/// `HostApiV0.fire_module_event` after the guest's `il2cpp_init` returned —
/// the reference `il2cpp_post_init` broker's event-model translation. The
/// value sits above the reference discriminant range on purpose: it never
/// collides with a real guest module kind.
/// `UnityFrame`, `UnityMaintenance` and `UnityStopping` are mapping-free events,
/// dispatched on the selected profile's player thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum ModuleKind {
    Unknown = 0,
    Main = 1,
    Il2cpp = 2,
    Unity = 3,
    Burst = 4,
    Fmod = 5,
    FmodStudio = 6,
    UnityHelpers = 7,
    Xlua = 8,
    /// Rust-only lifecycle event: guest IL2CPP metadata is usable. Never a
    /// mapped module; consumers use the binding they stored at `Il2cpp`.
    Il2cppReady = 100,
    /// Player-thread maintenance after nativeRender returned, at most once
    /// per second. Not a mapping; optional plugins keep their own state.
    UnityMaintenance = 101,
    /// Player thread is stopping, before nativeDone destroys the runtime.
    UnityStopping = 102,
    /// One successful nativeRender, on the player thread. Not a mapping.
    UnityFrame = 103,
}

impl ModuleKind {
    pub const ALL: [ModuleKind; 13] = [
        ModuleKind::Unknown,
        ModuleKind::Main,
        ModuleKind::Il2cpp,
        ModuleKind::Unity,
        ModuleKind::Burst,
        ModuleKind::Fmod,
        ModuleKind::FmodStudio,
        ModuleKind::UnityHelpers,
        ModuleKind::Xlua,
        ModuleKind::Il2cppReady,
        ModuleKind::UnityMaintenance,
        ModuleKind::UnityStopping,
        ModuleKind::UnityFrame,
    ];

    pub fn from_u32(value: u32) -> Option<ModuleKind> {
        ModuleKind::ALL.into_iter().find(|k| *k as u32 == value)
    }

    /// Stable machine name used in logs and events (C++ `module_kind_name`).
    pub const fn name(self) -> &'static str {
        match self {
            ModuleKind::Unknown => "unknown",
            ModuleKind::Main => "main",
            ModuleKind::Il2cpp => "il2cpp",
            ModuleKind::Unity => "unity",
            ModuleKind::Burst => "burst",
            ModuleKind::Fmod => "fmod",
            ModuleKind::FmodStudio => "fmodstudio",
            ModuleKind::UnityHelpers => "unity_helpers",
            ModuleKind::Xlua => "xlua",
            ModuleKind::Il2cppReady => "il2cpp_ready",
            ModuleKind::UnityMaintenance => "unity_maintenance",
            ModuleKind::UnityStopping => "unity_stopping",
            ModuleKind::UnityFrame => "unity_frame",
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/module.rs"]
mod tests;
