//! Loader test fixture (review 2026-09-22 finding 1): descriptors the
//! loader must refuse with a diagnostic instead of trusting their layout.
//!
//! - default: a descriptor from a foreign kit ABI — `abi_version` is not the
//!   loader's and every pointer field is a non-null address that must never
//!   be dereferenced, so a loader that reads past the version crashes;
//! - `empty-id`: a valid same-build descriptor with an empty id.
//!
//! Kept outside the workspace so it never ships in a kit.

#[cfg(not(feature = "empty-id"))]
mod foreign {
    /// Same leading field as every descriptor layout; the rest is junk.
    #[repr(C)]
    pub struct ForeignDescriptor {
        abi_version: u32,
        runtime_build_id: usize,
        id: usize,
        kind: u32,
        target_package: usize,
        required_plugins: usize,
    }

    static FOREIGN: ForeignDescriptor = ForeignDescriptor {
        abi_version: rudroid_common::descriptor::PLUGIN_ABI_VERSION + 1,
        runtime_build_id: 0x8,
        id: 0x8,
        kind: 0,
        target_package: 0x8,
        required_plugins: 0x8,
    };

    #[no_mangle]
    extern "C" fn rudroid_plugin_descriptor() -> *const ForeignDescriptor {
        &FOREIGN
    }
}

#[cfg(feature = "empty-id")]
rudroid_common::declare_descriptor!("", rudroid_common::Kind::Runtime);
