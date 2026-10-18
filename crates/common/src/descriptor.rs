//! INPUT:  shared kit identity, capability kind and optional training export macro
//! OUTPUT:  C descriptor ABI, kind taxonomy and per-image training marker
//! POS:    publishes same-kit capability identity without changing its C layout
//!
//! Kinds align with the C++ reference `RudroidPluginKind` values so descriptors
//! stay comparable across kits; wording follows `CONTEXT.md` where a Rust term
//! replaces the legacy one (SDK plugins are Android Addons).

use std::fmt;

/// Capability layer/role. Discriminants match `RudroidPluginKind` in the
/// read-only C++ reference (`projects/unityloader/plugin_api.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum Kind {
    /// Host log sink (`platform_log_stderr` role).
    Logging = 1,
    /// Guest Android base surface (`android_base` role).
    AndroidBase = 2,
    /// Unity engine profile (`unity_2021_3` / `unity_6` role).
    UnityProfile = 3,
    /// Host platform backend: window/audio/graphics (`platform_sdl_runtime` role).
    Platform = 4,
    /// Android Addon (former `sdk_*`): GMS, Play Core, FMOD, burst, helpers, xlua.
    AndroidAddon = 5,
    /// Mechanism runtime: il2cpp hook, thread trace and similar glue.
    Runtime = 6,
    /// Title gamepatch under `capability/unity/gamepatch/`.
    GamePatch = 7,
}

/// C ABI version of the kit boundary. Mirrors `RUDROID_PLUGIN_ABI_VERSION`
/// in the read-only reference (`plugin_api.h`); same-build kits only.
pub const PLUGIN_ABI_VERSION: u32 = 1;

/// C view of the descriptor, field-compatible with the reference
/// `RudroidPluginDescriptor`. `target_package`/`required_plugins` are null when
/// absent; `required_plugins` is a comma-separated list of exact ids.
#[repr(C)]
pub struct CRudroidPluginDescriptor {
    pub abi_version: u32,
    pub runtime_build_id: *const std::os::raw::c_char,
    pub id: *const std::os::raw::c_char,
    pub kind: u32,
    pub target_package: *const std::os::raw::c_char,
    pub required_plugins: *const std::os::raw::c_char,
}

/// `Send`/`Sync` wrapper for the exported static descriptor. All pointed-to
/// strings are process-lifetime leaks built once, so sharing across threads
/// after initialization is sound.
pub struct SharedDescriptor(pub CRudroidPluginDescriptor);
unsafe impl Send for SharedDescriptor {}
unsafe impl Sync for SharedDescriptor {}

/// Leak a NUL-terminated copy for the C ABI (`str` literals are not
/// NUL-terminated; the C descriptor contract requires them to be).
pub fn leak_cstr(text: &str) -> &'static std::ffi::CString {
    let owned = std::ffi::CString::new(text.to_owned())
        .expect("descriptor strings must not contain NUL");
    Box::leak(Box::new(owned))
}

/// Export `rudroid_plugin_descriptor` from a capability cdylib.
///
/// The descriptor embeds the same-build `runtime_build_id` compiled into
/// `rudroid-common` (ticket 06): the loader refuses capabilities whose id
/// differs. Arguments must be string literals / const expressions:
///
/// ```ignore
/// rudroid_common::declare_descriptor!("log_stderr", rudroid_common::Kind::Logging);
/// rudroid_common::declare_descriptor!(
///     "silksong_unity6",
///     rudroid_common::Kind::GamePatch,
///     package = "com.game.silksong",
///     requires = Some("unity_6,android_base")
/// );
/// ```
#[macro_export]
macro_rules! declare_descriptor {
    ($id:expr, $kind:expr) => {
        $crate::declare_descriptor!($id, $kind, package = None::<&str>, requires = None::<&str>);
    };
    ($id:expr, $kind:expr, package = $pkg:expr) => {
        $crate::declare_descriptor!($id, $kind, package = $pkg, requires = None::<&str>);
    };
    ($id:expr, $kind:expr, package = $pkg:expr, requires = $reqs:expr) => {
        $crate::export_pgo_training!();
        #[no_mangle]
        extern "C" fn rudroid_plugin_descriptor(
        ) -> *const $crate::descriptor::CRudroidPluginDescriptor {
            use std::sync::OnceLock;
            static C_DESCRIPTOR: OnceLock<$crate::descriptor::SharedDescriptor> = OnceLock::new();

            let shared = C_DESCRIPTOR.get_or_init(|| {
                let leak = $crate::descriptor::leak_cstr;
                $crate::descriptor::SharedDescriptor(
                    $crate::descriptor::CRudroidPluginDescriptor {
                        abi_version: $crate::descriptor::PLUGIN_ABI_VERSION,
                        runtime_build_id: leak($crate::build_id::compiled_build_id()).as_ptr()
                            as *const std::os::raw::c_char,
                        id: leak($id).as_ptr() as *const std::os::raw::c_char,
                        kind: $kind as u32,
                        target_package: $pkg.map_or(std::ptr::null(), |p: &str| {
                            leak(p).as_ptr() as *const std::os::raw::c_char
                        }),
                        required_plugins: $reqs.map_or(std::ptr::null(), |r: &str| {
                            leak(r).as_ptr() as *const std::os::raw::c_char
                        }),
                    },
                )
            });
            &shared.0
        }
    };
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Logging,
        Kind::AndroidBase,
        Kind::UnityProfile,
        Kind::Platform,
        Kind::AndroidAddon,
        Kind::Runtime,
        Kind::GamePatch,
    ];

    pub fn from_u32(value: u32) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| *k as u32 == value)
    }

    /// Stable machine name used in logs, Deploy Selection profiles and docs.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Logging => "logging",
            Kind::AndroidBase => "android_base",
            Kind::UnityProfile => "unity_profile",
            Kind::Platform => "platform",
            Kind::AndroidAddon => "android_addon",
            Kind::Runtime => "runtime",
            Kind::GamePatch => "gamepatch",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Read-only capability metadata queried from a dynamic library before any
/// phase entry point runs. Rust-side view; the C FFI layout lives in the
/// individual cdylib exports and keeps the same field order as the reference
/// `RudroidPluginDescriptor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    /// Exact capability id, unique inside a deployed kit (e.g. `android_base`).
    pub id: &'static str,
    pub kind: Kind,
    /// Gamepatch package gate: capability only activates for this package.
    /// `None` means no gate.
    pub target_package: Option<&'static str>,
    /// Exact capability ids this one requires, in declaration order.
    /// Dependencies affect validation and phase order; they never auto-enable
    /// a missing capability.
    pub required: &'static [&'static str],
}

impl CapabilityDescriptor {
    pub const fn new(id: &'static str, kind: Kind) -> Self {
        CapabilityDescriptor {
            id,
            kind,
            target_package: None,
            required: &[],
        }
    }

    pub const fn target_package(mut self, package: &'static str) -> Self {
        self.target_package = Some(package);
        self
    }

    pub const fn requires(mut self, required: &'static [&'static str]) -> Self {
        self.required = required;
        self
    }
}

/// Gamepatch package gate: `Some(package)` in the descriptor only activates
/// when the Host Config `[package].packageName` matches; `None` (no gate)
/// always loads. Reference parity: "target_package only prevents a game patch
/// from being installed into the wrong port".
pub fn package_gate(
    descriptor_package: Option<&str>,
    config_package: Option<&str>,
) -> bool {
    match descriptor_package {
        None => true,
        Some(target) => config_package == Some(target),
    }
}

#[cfg(test)]
#[path = "../tests/unit/descriptor.rs"]
mod tests;
