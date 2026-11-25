//! INPUT:  ModuleKind discriminants for guest mapping and engine lifecycle
//! OUTPUT:  stable u32 role constants for guest-plan builders and services
//! POS:    documents engine lifecycle aliases without renaming ModuleKind variants
#![forbid(unsafe_code)]

use rudroid_common::ModuleKind;

/// Guest mapping roles (ModuleKind 1..=8).
pub const MAIN: u32 = ModuleKind::Main as u32;
pub const IL2CPP: u32 = ModuleKind::Il2cpp as u32;
pub const UNITY: u32 = ModuleKind::Unity as u32;
pub const BURST: u32 = ModuleKind::Burst as u32;
pub const FMOD: u32 = ModuleKind::Fmod as u32;
pub const FMOD_STUDIO: u32 = ModuleKind::FmodStudio as u32;
pub const UNITY_HELPERS: u32 = ModuleKind::UnityHelpers as u32;
pub const XLUA: u32 = ModuleKind::Xlua as u32;

/// Engine lifecycle roles (ModuleKind 100..=103).
pub const ENGINE_SCRIPT_READY: u32 = ModuleKind::Il2cppReady as u32;
pub const ENGINE_MAINTENANCE: u32 = ModuleKind::UnityMaintenance as u32;
pub const ENGINE_STOPPING: u32 = ModuleKind::UnityStopping as u32;
pub const ENGINE_FRAME: u32 = ModuleKind::UnityFrame as u32;
