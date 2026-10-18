//! INPUT:  shared ABI, discovery, policy and native-call helper modules
//! OUTPUT:  shared common modules, including the evidence-gated IL2CPP build table
//! POS:    reuses kit contracts without capability exports or shared-state replicas
//! rudroid-common: shared capability types, symbol constants and discovery
//! conventions for the Rust Parallel Package kit.
//!
//! This crate carries no Android/SDL assumptions and depends only on `libc`
//! (for the ARM64 detour), so every capability and the loader can link it. Terminology follows the
//! repository `CONTEXT.md` (Capability, Android Addon, Deploy Selection, ...).

pub mod arm64_detour;
pub mod android_assets;
pub mod audio_backpressure;
pub mod build_id;
pub mod clock;
pub mod gamepatch_support;
pub mod handles;
pub mod descriptor;
pub mod display_dimensions;
pub mod graphics_features;
pub mod frame_timing;
pub mod filesystem;
pub mod elf_identity;
pub mod host_api;
pub mod il2cpp;
pub mod managed_hooks;
pub mod hud_layout;
mod touch_controls;
pub mod unity_objects;
pub mod il2cpp_abi;
pub mod jni_abi;
pub mod jni_tasks;
pub mod jni_env;
pub mod module;
pub mod pgo;
pub mod registry;
pub mod runtime_policy;
pub mod scan;
pub mod symbols;

pub use build_id::{active_build_id, check_same_build, compiled_build_id};
pub use descriptor::{package_gate, CapabilityDescriptor, Kind};
pub use module::ModuleKind;
pub use scan::{DeployEntry, DeployScanError, scan_deploy_dir};
