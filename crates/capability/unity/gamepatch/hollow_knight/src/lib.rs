//! INPUT:  IL2CPP metadata, viewport feature flags and typed detour slots
//! OUTPUT:  Hollow Knight camera/tk2d hooks, idempotent HUD fixups and installation events
//! POS:    viewport Title adaptation; code writes and original publication use the shared detour boundary
//! rudroid-unity-gamepatch-hollow-knight: Title gamepatch (ticket 29;
//! real patch behavior ticket 54).
//!
//! Rust counterpart of the read-only `hollow_knight_viewport` gamepatch
//! (`projects/unityloader/gamepatchs/hollow_knight_viewport`). The C++
//! reference builds this patch WITHOUT a package gate (empty `target_package`
//! in gamepatchs/CMakeLists.txt), so Parity keeps it ungated:
//! [`package_gate`] returns true for every package here. The gate mechanism
//! itself is shared (`rudroid_common::package_gate`) and is exercised by
//! the gated Silksong patch (ticket 30).
//!
//! **HK gate conflict decision (ticket 54):** old card 29 required a
//! `target_package` gate, but the reference is ungated ("presence in
//! unityloader.d is the one activation switch"). This crate keeps the
//! REFERENCE behavior — ungated — and the two requirements are satisfied by
//! DEPLOYMENT ISOLATION, not by code: the reference never put HK and
//! Silksong patches in one kit (each device build deploys its own
//! gamepatch set), and the Rust kit keeps that model, so the ungated HK
//! patch can never leak into a Silksong deployment. See the ticket-54 card
//! and manifest for the full decision record.
//!
//! Ticket 54 turns the former identity-only skeleton into the reference's
//! actual viewport chain:
//!
//! 1. `rudroid_plugin_init` — reads the reference switches with the
//!    reference fallback chain (legacy `hk_viewport.<name>` first, then
//!    `game_patches.hollow_knight_viewport.<name>` wins), subscribes to the
//!    module-loaded surface.
//! 2. On the il2cpp module event — the kit's analogue of the reference
//!    `register_il2cpp_post_init` trigger — the patch binds the guest
//!    `il2cpp_*` exports through `so_symbol`, resolves the reference targets
//!    (images by name, classes by namespace/name, methods by name/argc,
//!    tk2d FIELD OFFSETS through the field API — no hardcoded game offsets),
//!    and installs REAL inline-code detours (the shared
//!    `rudroid_common::arm64_detour`).
//! 3. The hook bodies implement the reference semantics: force-full camera
//!    rects, width-safe orthographic scaling (16:9 target), the tk2d
//!    settings rescale with the base-remembering slot cache, and the
//!    AutoScaleViewport 1.0 override — each behind its reference switch.

#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use rudroid_common::handles::HandleTable;
use rudroid_common::il2cpp::{Il2cpp, Image, Method, StaticI32Method};
use rudroid_common::registry::RudroidModuleEvent;
use rudroid_common::{package_gate, Kind, ModuleKind};
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_host_config::HostConfig;

// The detour machinery is shared through rudroid-common (cfg-gated to aarch64).
use rudroid_common::arm64_detour::{self as arm64, Original};

pub use arm64::HookShape;
use rudroid_common::gamepatch_support::install_detour as detour;

// The hooks install on `Il2cppReady`, which only il2cpp_patch fires.
rudroid_common::declare_descriptor!(
    "hollow_knight_viewport",
    Kind::GamePatch,
    package = None::<&str>,
    requires = Some("il2cpp_patch")
);

/// Reference parity: no package gate on this patch.
pub const TARGET_PACKAGE: Option<&str> = None;

/// Host log tag (reference `HKVIEW`).
const TAG: &CStr = c"HKVIEW";

/// Does this patch apply to the given Host Config package? Reference parity:
/// ALWAYS — the reference has no gate ("presence is the one activation
/// switch"); deployments isolate Titles by their gamepatch set.
pub fn applies_to(config: &HostConfig) -> bool {
    package_gate(TARGET_PACKAGE, config.package().package_name.as_deref())
}

/// The reference switch values for one config view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportFeatures {
    pub debug: bool,
    pub force_full_viewport: bool,
    pub tk2d_width_safe: bool,
    pub ui_orthographic_width_safe: bool,
}

impl ViewportFeatures {
    fn from_pairs(pairs: [(bool, bool); 4]) -> ViewportFeatures {
        ViewportFeatures {
            debug: pairs[0].0,
            force_full_viewport: pairs[1].0,
            tk2d_width_safe: pairs[2].0,
            ui_orthographic_width_safe: pairs[3].0,
        }
    }

    /// In-process read (tests). The reference chain through the shared
    /// override primitive (ticket 87 Phase C): the new
    /// `game_patches.hollow_knight_viewport.*` key wins, the legacy
    /// `hk_viewport.*` key is the fallback, then the default. A
    /// present-but-non-boolean value falls through exactly like a missing
    /// key (`get_bool_opt` = the chain-through condition of `get_bool`).
    pub fn from_config(config: &HostConfig) -> ViewportFeatures {
        let read = |name: &str, fallback: bool| {
            rudroid_common::gamepatch_support::override_setting(
                |key| config.get_bool_opt(key),
                &format!("game_patches.hollow_knight_viewport.{name}"),
                &format!("hk_viewport.{name}"),
                fallback,
            )
        };
        ViewportFeatures::from_pairs([
            (read("debug", false), false),
            (read("force_full_viewport", true), false),
            (read("tk2d_width_safe", true), false),
            (read("ui_orthographic_width_safe", true), false),
        ])
    }

    /// Host-API-getter read (the C++ consumption path).
    pub fn from_host_api(api: &HostApiV0) -> ViewportFeatures {
        let read = |name: &str, fallback: bool| {
            let get = |key: String, fallback: bool| match CString::new(key) {
                Ok(key) => api.config_bool(&key, fallback),
                Err(_) => fallback,
            };
            let legacy = get(format!("hk_viewport.{name}"), fallback);
            get(format!("game_patches.hollow_knight_viewport.{name}"), legacy)
        };
        ViewportFeatures::from_pairs([
            (read("debug", false), false),
            (read("force_full_viewport", true), false),
            (read("tk2d_width_safe", true), false),
            (read("ui_orthographic_width_safe", true), false),
        ])
    }
}

// ---------------------------------------------------------------------------
// The behavior registry (ticket 54 acceptance item 1, HK side).
// ---------------------------------------------------------------------------

/// Reference trigger events (see the ticket-54 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The reference `register_il2cpp_post_init` callback. Kit analogue: the
    /// module-loaded event of the il2cpp guest module.
    Il2cppPostInit,
}

/// How an entry behaves in this crate today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A real detour install against a resolved method entry.
    Hook(HookBody),
    /// Registered with the exact reference spec; not installed.
    RegisteredOnly(&'static str),
}

/// The hook bodies this crate carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookBody {
    /// Camera.set_rect_Injected → rect forced to (0,0,1,1) pre-original
    /// (reference `camera_set_rect_injected_hook`, force_full_viewport).
    ForceFullRect,
    /// Camera.set_orthographicSize → value scaled by the width-safe factor
    /// (reference `camera_set_orthographic_size_hook`, ui_orthographic_width_safe).
    UiOrthoWidthSafe,
    /// tk2dCamera.UpdateCameraMatrix → settings/zoom rescaled pre-original
    /// (reference `tk2d_update_camera_matrix_hook`, tk2d_width_safe).
    Tk2dWidthSafe,
    /// ForceCameraAspect.AutoScaleViewport → 1.0 when force_full_viewport,
    /// plus the tk2d/HUD fixups (reference `force_aspect_auto_scale_hook`).
    AutoScaleOverride,
    /// ForceCameraAspect.SetOverscanViewport pass-through with logging
    /// (reference `force_aspect_set_overscan_hook`, always-on group).
    OverscanPassthrough,
    /// GameCameras.SetOverscan pass-through with logging (reference
    /// `game_cameras_set_overscan_hook`, always-on group).
    SetOverscanPassthrough,
}

/// One registered behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanEntry {
    pub behavior: &'static str,
    /// The switch chain that gates it (empty = always installed when the
    /// target resolves, exactly like the reference always-on group).
    pub config_key: &'static str,
    pub reference_default: bool,
    pub trigger: Trigger,
    /// Target matching method (the reference resolves all of these through
    /// image/class/method-name lookups, never hardcoded addresses).
    pub target: &'static str,
    pub action: Action,
}

/// One registry row with the current config view attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub entry: PatchPlanEntry,
    /// Would the reference install this hook with the current config?
    pub enabled: bool,
}

/// The behavior registry in reference install order.
pub fn plan(features: &ViewportFeatures) -> Vec<PlanEntry> {
    vec![
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "force_full_viewport (camera rect)",
                config_key: "force_full_viewport",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "UnityEngine.Camera.set_rect_Injected(Rect) [UnityEngine.CoreModule image]",
                action: Action::Hook(HookBody::ForceFullRect),
            },
            enabled: features.force_full_viewport,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "force_full_viewport (AutoScaleViewport override)",
                config_key: "force_full_viewport",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "ForceCameraAspect.AutoScaleViewport [Assembly-CSharp image]",
                action: Action::Hook(HookBody::AutoScaleOverride),
            },
            enabled: features.force_full_viewport,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "ui_orthographic_width_safe",
                config_key: "ui_orthographic_width_safe",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "UnityEngine.Camera.set_orthographicSize(float)",
                action: Action::Hook(HookBody::UiOrthoWidthSafe),
            },
            enabled: features.ui_orthographic_width_safe,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "tk2d_width_safe",
                config_key: "tk2d_width_safe (also armed by debug)",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "tk2dCamera.UpdateCameraMatrix + tk2dCamera/tk2dCameraSettings FIELD OFFSETS (resolved, never guessed)",
                action: Action::Hook(HookBody::Tk2dWidthSafe),
            },
            enabled: features.tk2d_width_safe || features.debug,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "overscan pass-through logging (always-on group)",
                config_key: "(none — presence in the deploy dir activates the patch)",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "ForceCameraAspect.SetOverscanViewport + GameCameras.SetOverscan",
                action: Action::Hook(HookBody::OverscanPassthrough),
            },
            enabled: true,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "GameSettings overscan logging",
                config_key: "(none — reference logs Load/SaveOverscanSettings)",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "GameSettings.LoadOverscanSettings / SaveOverscanSettings + overScanAdjustment field offsets",
                action: Action::RegisteredOnly(
                    "logging-only wrappers; the fixture models no GameSettings class and the kit gains nothing from the logs",
                ),
            },
            enabled: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// Guest IL2CPP lookups over the shared `rudroid_common::il2cpp` binding
// (reference init resolves exactly these eight exports and disables the
// patch when one is missing).
// ---------------------------------------------------------------------------

/// The reference `init()` required export set.
const REQUIRED_EXPORTS: &[&str] = &[
    "il2cpp_class_from_type",
    "il2cpp_class_get_name",
    "il2cpp_class_get_namespace",
    "il2cpp_field_get_type",
    "il2cpp_field_get_flags",
    "il2cpp_method_get_flags",
    "il2cpp_method_get_param_count",
    "il2cpp_method_get_param",
    "il2cpp_method_get_return_type",
    "il2cpp_type_get_type",
    "il2cpp_type_is_byref",
    "il2cpp_domain_get",
    "il2cpp_domain_get_assemblies",
    "il2cpp_assembly_get_image",
    "il2cpp_image_get_name",
    "il2cpp_class_from_name",
    "il2cpp_class_get_method_from_name",
    "il2cpp_class_get_field_from_name",
    "il2cpp_field_get_offset",
];

/// `resolve_method`: class + method-name lookup in one image, native entry
/// (`*(uintptr_t*)mi`). Returns (entry, MethodInfo).
fn method_entry(api: &Il2cpp, image: Image, ns: &str, class: &str, method: &str, argc: c_int) -> Option<(usize, Method)> {
    let (static_method, params, returns): (bool, &[(i32, bool)], i32) = match method {
        "get_width" | "get_height" => (true, &[], 8),
        "get_orthographicSize" => (false, &[], 0x0c),
        "set_rect_Injected" => (false, &[(0x11, true)], 1),
        "set_orthographicSize" | "SetOverscanViewport" | "SetOverscan" => (false, &[(0x0c, false)], 1),
        "UpdateCameraMatrix" => (false, &[], 1),
        "AutoScaleViewport" => (false, &[], 0x0c),
        _ => return None,
    };
    let (ns, class, method) = (CString::new(ns).ok()?, CString::new(class).ok()?, CString::new(method).ok()?);
    let mi = api.method(api.class_from_name(image, &ns, &class)?, &method, argc)?;
    if !api.method_signature(mi, static_method, params, returns) { return None; }
    if method.as_bytes() == b"set_rect_Injected" {
        // A same-named valuetype in another image can have a different layout.
        let expected = api.class_from_name(image, c"UnityEngine", c"Rect")?;
        if api.class_from_type(api.method_param(mi, 0)?) != Some(expected) { return None; }
    }
    Some((api.method_entry(mi)?, mi))
}

/// `resolve_field_offset`: field-name lookup + offset read (the tk2d
/// layout comes from the guest metadata — never guessed).
fn field_offset(api: &Il2cpp, image: Image, ns: &str, class: &str, field: &str) -> Option<usize> {
    let (ns, class, field) = (CString::new(ns).ok()?, CString::new(class).ok()?, CString::new(field).ok()?);
    let info = api.field(api.class_from_name(image, &ns, &class)?, &field)?;
    let kind = match field.to_bytes() {
        b"zoomFactor" | b"orthographicSize" | b"orthographicPixelsPerMeter" => 0x0c,
        b"cameraSettings" | b"tk2dCam" | b"hudCam" | b"hudCamera" | b"mainCamera" => 0x12,
        _ => return None,
    };
    let ty = api.field_type(info)?;
    if api.field_flags(info)? & 0x10 != 0 || api.type_kind(ty) != Some(kind) || api.type_byref(ty) != Some(false) { return None; }
    if kind == 0x12 {
        let (image, namespace, name) = match field.to_bytes() {
            b"cameraSettings" => (image, c"", c"tk2dCameraSettings"),
            b"tk2dCam" => (image, c"", c"tk2dCamera"),
            b"hudCam" | b"hudCamera" | b"mainCamera" => (api.find_image("UnityEngine.CoreModule")?, c"UnityEngine", c"Camera"),
            _ => return None,
        };
        let expected = api.class_from_name(image, namespace, name)?;
        if api.class_from_type(ty) != Some(expected) { return None; }
    }
    api.field_offset(info)
}

// ---------------------------------------------------------------------------
// Install + hook bodies (reference semantics).
// ---------------------------------------------------------------------------

/// One install outcome.
pub use rudroid_common::gamepatch_support::InstallEvent;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Hook bodies must never panic: a poisoned lock still hands out the data.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The guest Rect layout the reference `UnityEngine_Rect_o` mirrors.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GuestRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl GuestRect {
    const FULL: GuestRect = GuestRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
}

/// Runtime switch snapshot the hooks observe (reference `g_force_full_viewport`
/// etc., read at call time).
static FORCE_FULL: AtomicBool = AtomicBool::new(true);
static UI_ORHO_SAFE: AtomicBool = AtomicBool::new(false);
static TK2D_SAFE: AtomicBool = AtomicBool::new(false);

/// Trampolines (callable originals) and resolved entries.
static SET_RECT_ORIG: Original<SetRectFn> = Original::new();
static SET_RECT_MI: AtomicUsize = AtomicUsize::new(0);
static SET_ORTHO_ORIG: Original<SetFloatFn> = Original::new();
static TK2D_MATRIX_ORIG: Original<UpdateFn> = Original::new();
static AUTO_ORIG: Original<AutoFn> = Original::new();
static OVERSCAN_VP_ORIG: Original<SetFloatFn> = Original::new();
static SET_OVERSCAN_ORIG: Original<SetFloatFn> = Original::new();
/// One coherent pair of native entries and their immutable MethodInfo handles.
static SCREEN: Mutex<Option<(StaticI32Method, StaticI32Method)>> = Mutex::new(None);
/// Camera owner references resolved through exact field types, never guessed.
static FORCE_ASPECT_OFFSETS: Mutex<Option<CameraOffsets>> = Mutex::new(None);
static GAME_CAMERA_OFFSETS: Mutex<Option<CameraOffsets>> = Mutex::new(None);
#[derive(Clone, Copy)]
struct CameraOffsets {
    tk2d: usize,
    hud: usize,
    main: Option<usize>,
}
#[derive(Clone, Copy)]
struct OrthoCalls {
    get: GetFloatFn,
    get_mi: usize,
    set_mi: usize,
}
static ORTHO_CALLS: Mutex<Option<OrthoCalls>> = Mutex::new(None);
// Addresses are comparison tokens only, never dereferenced from this cache.
// No managed reference escapes the owning native callback.
static ORTHO_SLOTS: Mutex<[(usize, f32, f32); 16]> = Mutex::new([(0, 0.0, 0.0); 16]);
/// tk2d layout offsets resolved through the field API.
static TK2D_OFFSETS: Mutex<Option<Tk2dOffsets>> = Mutex::new(None);
#[derive(Debug, Clone, Copy)]
struct Tk2dOffsets {
    camera_settings: usize,
    zoom: usize,
    ortho: usize,
    ppm: usize,
}
/// The reference `g_tk2d_base[16]` slot cache: per-camera base/patched values
/// so repeated UpdateCameraMatrix calls never double-scale.
static TK2D_SLOTS: Mutex<[Tk2dSlot; 16]> = Mutex::new([Tk2dSlot::EMPTY; 16]);
#[derive(Debug, Clone, Copy)]
struct Tk2dSlot {
    camera: usize,
    settings: usize,
    ortho_base: f32,
    ortho_patched: f32,
    ppm_base: f32,
    ppm_patched: f32,
    zoom_base: f32,
    zoom_patched: f32,
}
impl Tk2dSlot {
    const EMPTY: Tk2dSlot = Tk2dSlot {
        camera: 0,
        settings: 0,
        ortho_base: 0.0,
        ortho_patched: 0.0,
        ppm_base: 0.0,
        ppm_patched: 0.0,
        zoom_base: 0.0,
        zoom_patched: 0.0,
    };
}

/// Host table snapshot (reference `g_api->log`).
static HOST_LOG: Mutex<Option<HostApiV0>> = Mutex::new(None);
/// Bounded log counters (reference `static int count` guards).
static LOG_COUNTS: std::sync::LazyLock<Mutex<std::collections::HashMap<&'static str, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

fn hk_log_raw(message: &str) {
    let Some(api) = *lock(&HOST_LOG) else {
        return;
    };
    // NUL rejection preserved (skip, never escape) for the hook logs.
    if message.contains('\0') {
        return;
    }
    api.log_line(TAG, message);
}

/// Log a hook line at most `limit` times (reference `count++ < N` guards).
fn bounded_log(key: &'static str, limit: usize, message: impl FnOnce() -> String) {
    let mut counts = LOG_COUNTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let count = counts.entry(key).or_insert(0);
    if *count < limit {
        *count += 1;
        drop(counts);
        hk_log_raw(&message());
    }
}

unsafe fn read_f32(address: usize) -> f32 {
    // SAFETY: the caller passes a live guest object field address.
    unsafe { std::ptr::read_unaligned(address as *const f32) }
}

unsafe fn write_f32(address: usize, value: f32) {
    // SAFETY: as read_f32.
    unsafe { std::ptr::write_unaligned(address as *mut f32, value) }
}

unsafe fn read_ptr(address: usize) -> usize {
    // SAFETY: live guest object field.
    unsafe { std::ptr::read_unaligned(address as *const usize) }
}

fn absf(value: f32) -> f32 {
    if value < 0.0 { -value } else { value }
}

fn looks_like_current_patch(current: f32, patched: f32) -> bool {
    patched > 0.0 && current > 0.0 && absf(current - patched) < 0.01
}

/// Reference `width_safe_scale`: (16/9)/aspect while the screen is narrower
/// than 16:9, else 1.0 (no upscale).
/// # Safety
/// Called on the Unity thread, as required by the native Screen getters.
unsafe fn width_safe_scale() -> f32 {
    let screen = *lock(&SCREEN);
    let (w, h) = match screen {
        // SAFETY: the binding checks both prototypes and pairs their metadata;
        // the caller supplies the Unity thread. No mutex spans a guest call.
        Some((width, height)) => unsafe {
            (width.call(), height.call())
        },
        None => (-1, -1),
    };
    if w <= 0 || h <= 0 {
        return 1.0;
    }
    let aspect = w as f32 / h as f32;
    const TARGET: f32 = 16.0 / 9.0;
    if aspect <= 0.0 || aspect >= TARGET {
        return 1.0;
    }
    TARGET / aspect
}

/// Reference `remember_tk2d_base`: the per-camera slot cache that keeps the
/// un-scaled base values so repeated passes stay idempotent.
fn remember_tk2d_base(
    camera: usize,
    settings: usize,
    ortho: f32,
    ppm: f32,
    zoom: f32,
    scale: f32,
) -> Option<Tk2dSlot> {
    if camera == 0 {
        return None;
    }
    let mut slots = lock(&TK2D_SLOTS);
    if let Some(slot) = slots.iter_mut().find(|slot| slot.camera == camera) {
        if slot.settings != settings {
            slot.settings = settings;
            slot.ortho_base = ortho;
            slot.ppm_base = ppm;
            slot.zoom_base = zoom;
        } else {
            if ortho > 0.0 && !looks_like_current_patch(ortho, slot.ortho_patched) {
                slot.ortho_base = ortho;
            }
            if ppm > 0.0 && !looks_like_current_patch(ppm, slot.ppm_patched) {
                slot.ppm_base = ppm;
            }
            if zoom > 0.0 && !looks_like_current_patch(zoom, slot.zoom_patched) {
                slot.zoom_base = zoom;
            }
        }
        slot.ortho_patched = if slot.ortho_base > 0.0 { slot.ortho_base * scale } else { 0.0 };
        slot.ppm_patched = if slot.ppm_base > 0.0 { slot.ppm_base / scale } else { 0.0 };
        slot.zoom_patched = if slot.zoom_base > 0.0 { slot.zoom_base / scale } else { 0.0 };
        return Some(*slot);
    }
    if let Some(slot) = slots.iter_mut().find(|slot| slot.camera == 0) {
        *slot = Tk2dSlot {
            camera,
            settings,
            ortho_base: ortho,
            ortho_patched: if ortho > 0.0 { ortho * scale } else { 0.0 },
            ppm_base: ppm,
            ppm_patched: if ppm > 0.0 { ppm / scale } else { 0.0 },
            zoom_base: zoom,
            zoom_patched: if zoom > 0.0 { zoom / scale } else { 0.0 },
        };
        return Some(*slot);
    }
    None
}

/// Reference `apply_tk2d_width_safe`: rescale the settings/zoom fields
/// through the RESOLVED offsets before the original body runs.
///
/// # Safety
/// `tk2d` and its settings stay live and stationary for every field access;
/// the caller runs on the Unity thread without concurrent field mutation.
unsafe fn apply_tk2d_width_safe(tk2d: *mut c_void, tag: &'static str) {
    if !TK2D_SAFE.load(Ordering::Relaxed) || tk2d.is_null() {
        return;
    }
    let Some(offsets) = *lock(&TK2D_OFFSETS) else {
        return;
    };
    // SAFETY: this fixup runs inside a Unity-thread camera hook.
    let scale = unsafe { width_safe_scale() };
    if scale <= 1.0 {
        return;
    }
    // SAFETY: live guest object with resolved offsets.
    unsafe {
        let settings = read_ptr(tk2d as usize + offsets.camera_settings);
        if settings == 0 {
            return;
        }
        let ortho = read_f32(settings + offsets.ortho);
        let ppm = read_f32(settings + offsets.ppm);
        let zoom = read_f32(tk2d as usize + offsets.zoom);
        let Some(base) = remember_tk2d_base(tk2d as usize, settings, ortho, ppm, zoom, scale)
        else {
            return;
        };
        if base.ortho_patched > 0.0 {
            write_f32(settings + offsets.ortho, base.ortho_patched);
        }
        if base.ppm_patched > 0.0 {
            write_f32(settings + offsets.ppm, base.ppm_patched);
        }
        if base.zoom_patched > 0.0 {
            write_f32(tk2d as usize + offsets.zoom, base.zoom_patched);
        }
        bounded_log(
            tag,
            96,
            || format!(
                "{tag} tk2d width-safe ortho {ortho:.4}(base {ortho_base:.4})->{ortho_patched:.4}                  ppm {ppm:.4}(base {ppm_base:.4})->{ppm_patched:.4} zoom {zoom:.4}(base {zoom_base:.4})->{zoom_patched:.4} scale={scale:.4}",
                ortho_base = base.ortho_base,
                ortho_patched = base.ortho_patched,
                ppm_base = base.ppm_base,
                ppm_patched = base.ppm_patched,
                zoom_base = base.zoom_base,
                zoom_patched = base.zoom_patched,
            ),
        );
    }
}

/// Reference camera fixups: full rect plus native-initialized UI orthographic
/// correction. The getter and ORIGINAL setter avoid recursively scaling again.
///
/// # Safety
/// `camera` must point at a live guest Camera instance on the Unity thread.
unsafe fn apply_camera_fixups(camera: *mut c_void, tag: &'static str) {
    if camera.is_null() {
        return;
    }
    let ortho = *lock(&ORTHO_CALLS);
    // SAFETY: the owning native callback keeps camera live/stationary on the
    // Unity thread; all entries and paired MethodInfo values were ABI checked.
    // Copy bindings before guest calls, and release cache locks before setters.
    unsafe {
        if FORCE_FULL.load(Ordering::Relaxed) {
            if let Some(set_rect) = SET_RECT_ORIG.get() {
                let mi = SET_RECT_MI.load(Ordering::Relaxed) as *mut c_void;
                let mut rect = GuestRect::FULL;
                set_rect(camera, &mut rect, mi);
                bounded_log(tag, 32, || format!("{tag} full camera rect applied camera={camera:p}"));
            }
        }
        if let (true, Some(calls), Some(set)) = (UI_ORHO_SAFE.load(Ordering::Relaxed), ortho, SET_ORTHO_ORIG.get()) {
            let scale = width_safe_scale();
            if scale > 1.0 {
                let current = (calls.get)(camera, calls.get_mi as *mut c_void);
                if let Some(patched) = remember_ortho(camera as usize, current, scale) {
                    set(camera, patched, calls.set_mi as *mut c_void);
                    bounded_log("ui_ortho_fixup", 64, || format!("{tag} UI ortho width-safe {current:.4} -> {patched:.4} scale={scale:.4}"));
                }
            }
        }
    }
}

fn remember_ortho(camera: usize, current: f32, scale: f32) -> Option<f32> {
    if camera == 0 || !current.is_finite() || current <= 0.0 || !scale.is_finite() || scale <= 1.0 {
        return None;
    }
    let mut slots = lock(&ORTHO_SLOTS);
    let index = slots.iter().position(|slot| slot.0 == camera)
        .or_else(|| slots.iter().position(|slot| slot.0 == 0))?;
    let slot = &mut slots[index];
    let base = if slot.0 == camera && looks_like_current_patch(current, slot.2) { slot.1 } else { current };
    let patched = base * scale;
    if !patched.is_finite() { return None; }
    *slot = (camera, base, patched);
    Some(patched)
}

/// Common post-call sequence for all three game camera callbacks. The raw
/// references are borrowed only while their live owner remains in this hook.
/// # Safety
/// `owner` and its metadata-checked camera fields stay live and stationary
/// throughout the Unity-thread callback, including all nested native calls.
unsafe fn apply_owner_fixups(owner: *mut c_void, offsets: Option<CameraOffsets>, tag: &'static str) {
    let (false, Some(offsets)) = (owner.is_null(), offsets) else { return; };
    // SAFETY: caller retains the live owner and its camera fields for this
    // complete callback; offsets came from exact class/type metadata.
    unsafe {
        let tk2d = read_ptr(owner as usize + offsets.tk2d) as *mut c_void;
        let hud = read_ptr(owner as usize + offsets.hud) as *mut c_void;
        let main = offsets.main.map(|offset| read_ptr(owner as usize + offset) as *mut c_void);
        apply_tk2d_width_safe(tk2d, tag);
        apply_camera_fixups(hud, tag);
        if let Some(main) = main { apply_camera_fixups(main, tag); }
    }
}

type SetRectFn = unsafe extern "C-unwind" fn(*mut c_void, *mut GuestRect, *mut c_void);
type SetFloatFn = unsafe extern "C-unwind" fn(*mut c_void, f32, *mut c_void);
type GetFloatFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> f32;
type UpdateFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void);
type AutoFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> f32;

/// Trampoline slot of one always-on hook (their signatures differ).
enum AlwaysOnSlot {
    Float(SetFloatFn, &'static Original<SetFloatFn>),
    Auto(AutoFn, &'static Original<AutoFn>),
}
impl AlwaysOnSlot {
    /// # Safety
    /// The resolved entry matches this hook and stays idle through publication.
    unsafe fn install(self, behavior: &'static str, target: &str, entry: usize) -> Result<arm64::InstalledDetour, InstallEvent> {
        // SAFETY: the caller supplies the entry's ABI and patch window; the
        // enum keeps each replacement paired with its exact original type.
        unsafe {
            match self {
                Self::Float(hook, slot) => detour(behavior, target, entry, slot, hook),
                Self::Auto(hook, slot) => detour(behavior, target, entry, slot, hook),
            }
        }
    }
}

/// Reference `camera_set_rect_injected_hook`: force the rect to full BEFORE
/// the original runs, then log the observation.
unsafe extern "C-unwind" fn set_rect_injected_hook(self_: *mut c_void, rect: *mut GuestRect, mi: *mut c_void) {
    if FORCE_FULL.load(Ordering::Relaxed) && !rect.is_null() {
        // SAFETY: the caller handed a live out-rect.
        let before = unsafe { std::ptr::read_unaligned(rect) };
        // SAFETY: as above.
        unsafe { std::ptr::write_unaligned(rect, GuestRect::FULL) };
        bounded_log(
            "set_rect",
            96,
            || format!(
                "Camera.set_rect_Injected rect {:.3},{:.3} {:.3}x{:.3} -> full",
                before.x, before.y, before.width, before.height
            ),
        );
    }
    if let Some(orig) = SET_RECT_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        unsafe { orig(self_, rect, mi) };
    }
}

/// Reference `camera_set_orthographic_size_hook`: scale the value by the
/// width-safe factor (1.0 when the switch is off), then call the original.
unsafe extern "C-unwind" fn set_orthographic_size_hook(self_: *mut c_void, value: f32, mi: *mut c_void) {
    let scale = if UI_ORHO_SAFE.load(Ordering::Relaxed) {
        // SAFETY: the camera setter is called on the Unity thread.
        unsafe { width_safe_scale() }
    } else {
        1.0
    };
    let patched = value * scale;
    bounded_log(
        "set_ortho",
        96,
        || format!(
            "Camera.set_orthographicSize {value:.4} -> {patched:.4} scale={scale:.4}"
        ),
    );
    if let Some(orig) = SET_ORTHO_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        unsafe { orig(self_, patched, mi) };
    }
}

/// Reference `tk2d_update_camera_matrix_hook`: apply the width-safe rescale
/// BEFORE the original reads the settings.
unsafe extern "C-unwind" fn tk2d_update_camera_matrix_hook(self_: *mut c_void, mi: *mut c_void) {
    // SAFETY: live guest tk2dCamera.
    unsafe { apply_tk2d_width_safe(self_, "tk2dCamera.UpdateCameraMatrix") };
    if let Some(orig) = TK2D_MATRIX_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        unsafe { orig(self_, mi) };
    }
}

/// Reference `force_aspect_auto_scale_hook`: run the original, apply the tk2d
/// and HUD fixups through the resolved ForceCameraAspect fields,
/// then return 1.0 when force_full_viewport holds.
unsafe extern "C-unwind" fn auto_scale_hook(self_: *mut c_void, mi: *mut c_void) -> f32 {
    let ret = match AUTO_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        Some(orig) => unsafe { orig(self_, mi) },
        None => 0.0,
    };
    let offsets = *lock(&FORCE_ASPECT_OFFSETS);
    // SAFETY: the native callback retains its live owner and referenced cameras.
    unsafe { apply_owner_fixups(self_, offsets, "ForceCameraAspect.AutoScale") };
    bounded_log(
        "auto_scale",
        32,
        || format!("ForceCameraAspect.AutoScaleViewport -> {ret:.4}"),
    );
    if FORCE_FULL.load(Ordering::Relaxed) {
        1.0
    } else {
        ret
    }
}

/// Preserve the original overscan value, then correct the owner's cameras.
unsafe extern "C-unwind" fn set_overscan_viewport_hook(self_: *mut c_void, adjustment: f32, mi: *mut c_void) {
    bounded_log(
        "set_overscan_viewport",
        32,
        || format!(
            "ForceCameraAspect.SetOverscanViewport adjustment={adjustment:.4} self={:p}",
            self_
        ),
    );
    if let Some(orig) = OVERSCAN_VP_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        unsafe {
            orig(self_, adjustment, mi);
            let offsets = *lock(&FORCE_ASPECT_OFFSETS);
            apply_owner_fixups(self_, offsets, "ForceCameraAspect.SetOverscanViewport");
        };
    }
}

/// Preserve the original overscan value, then correct HUD/main/tk2d cameras.
unsafe extern "C-unwind" fn set_overscan_hook(self_: *mut c_void, value: f32, mi: *mut c_void) {
    bounded_log(
        "set_overscan",
        32,
        || format!("GameCameras.SetOverscan value={value:.4} self={:p}", self_),
    );
    if let Some(orig) = SET_OVERSCAN_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        unsafe {
            orig(self_, value, mi);
            let offsets = *lock(&GAME_CAMERA_OFFSETS);
            apply_owner_fixups(self_, offsets, "GameCameras.SetOverscan");
        };
    }
}

/// Run the reference install chain against a bound IL2CPP API. Switched-off
/// hooks are reported as `Skipped` and NOT installed (ticket 54 negative
/// contract).
///
/// # Safety
/// The guest and patch modules remain mapped for every hook call. Installation
/// runs on the Unity thread with all patched entries idle through publication.
/// Camera hooks receive live, stationary objects with synchronized fields.
pub unsafe fn install(api: &Il2cpp, features: &ViewportFeatures) -> Vec<InstallEvent> {
    FORCE_FULL.store(features.force_full_viewport, Ordering::Relaxed);
    UI_ORHO_SAFE.store(features.ui_orthographic_width_safe, Ordering::Relaxed);
    TK2D_SAFE.store(features.tk2d_width_safe, Ordering::Relaxed);
    *lock(&FORCE_ASPECT_OFFSETS) = None;
    *lock(&GAME_CAMERA_OFFSETS) = None;
    *lock(&ORTHO_CALLS) = None;
    *lock(&ORTHO_SLOTS) = [(0, 0.0, 0.0); 16];
    *lock(&TK2D_OFFSETS) = None;
    *lock(&SCREEN) = None;

    let mut events = Vec::new();

    let asm_image = api.find_image("Assembly-CSharp");
    let core_image = api.find_image("UnityEngine.CoreModule");
    let (Some(asm_image), Some(core_image)) = (asm_image, core_image) else {
        // Reference: "image missing asm=%p core=%p" and no hooks.
        hk_log_raw("image missing — native viewport unchanged");
        events.push(InstallEvent::TargetMissing {
            behavior: "hollow_knight_viewport",
            target: "Assembly-CSharp / UnityEngine.CoreModule images".to_owned(),
        });
        return events;
    };

    // A missing field skips the secondary fixups rather than reading a
    // guessed object layout. AutoScaleViewport's return override still works.
    let tk2d = field_offset(api, asm_image, "", "ForceCameraAspect", "tk2dCam");
    let hud = field_offset(api, asm_image, "", "ForceCameraAspect", "hudCam");
    if let (Some(tk2d), Some(hud)) = (tk2d, hud) {
        *lock(&FORCE_ASPECT_OFFSETS) = Some(CameraOffsets { tk2d, hud, main: None });
    } else {
        events.push(InstallEvent::TargetMissing {
            behavior: "force_full_viewport (camera fixups)",
            target: "ForceCameraAspect.tk2dCam/hudCam fields".to_owned(),
        });
    }
    let tk2d = field_offset(api, asm_image, "", "GameCameras", "tk2dCam");
    let hud = field_offset(api, asm_image, "", "GameCameras", "hudCamera");
    let main = field_offset(api, asm_image, "", "GameCameras", "mainCamera");
    if let (Some(tk2d), Some(hud), Some(main)) = (tk2d, hud, main) {
        *lock(&GAME_CAMERA_OFFSETS) = Some(CameraOffsets { tk2d, hud, main: Some(main) });
    } else {
        events.push(InstallEvent::TargetMissing {
            behavior: "game camera fixups",
            target: "GameCameras.tk2dCam/hudCamera/mainCamera fields".to_owned(),
        });
    }

    // Screen getters (resolve only, reference `resolve_call`).
    let resolve_screen = |name| {
        method_entry(api, core_image, "UnityEngine", "Screen", name, 0)
            .and_then(|(_, mi)| api.static_i32_method(mi))
    };
    let (width, height) = (resolve_screen("get_width"), resolve_screen("get_height"));
    for (name, present) in [("get_width", width.is_some()), ("get_height", height.is_some())] {
        if !present {
            events.push(InstallEvent::TargetMissing {
                behavior: "hollow_knight_viewport",
                target: format!("UnityEngine.Screen.{name}"),
            });
        }
    }
    *lock(&SCREEN) = width.zip(height);

    // Camera.set_rect_Injected (resolve for the HUD fixups + detour when on).
    let set_rect = method_entry(api, core_image, "UnityEngine", "Camera", "set_rect_Injected", 1);
    if let Some((_entry, mi)) = set_rect {
        SET_RECT_MI.store(mi.as_ptr() as usize, Ordering::Relaxed);
    }
    if !features.force_full_viewport {
        events.push(InstallEvent::Skipped {
            behavior: "force_full_viewport",
            reason: "switch off",
        });
    } else {
        match set_rect {
            Some((entry, _mi)) => {
                // SAFETY: guest code pages; the patch window keeps execute permission.
                match unsafe { detour("force_full_viewport", "Camera.set_rect_Injected", entry, &SET_RECT_ORIG, set_rect_injected_hook) } {
                    Ok(installed) => {
                        hk_log_raw(&format!(
                            "force_full_viewport=1 camera.set_rect_Injected={:#x}",
                            installed.target
                        ));
                        events.push(InstallEvent::Installed {
                            behavior: "force_full_viewport",
                            target: "Camera.set_rect_Injected".to_owned(),
                            shape: installed.shape,
                        });
                    }
                    Err(event) => events.push(event),
                }
            }
            None => events.push(InstallEvent::TargetMissing {
                behavior: "force_full_viewport",
                target: "Camera.set_rect_Injected".to_owned(),
            }),
        }
    }

    // Camera.set_orthographicSize (ui_orthographic_width_safe).
    if !features.ui_orthographic_width_safe {
        events.push(InstallEvent::Skipped {
            behavior: "ui_orthographic_width_safe",
            reason: "switch off",
        });
    } else {
        match method_entry(api, core_image, "UnityEngine", "Camera", "set_orthographicSize", 1) {
            Some((entry, setter_mi)) => {
                let getter = method_entry(api, core_image, "UnityEngine", "Camera", "get_orthographicSize", 0);
                if getter.is_none() {
                    events.push(InstallEvent::TargetMissing {
                        behavior: "native UI camera correction",
                        target: "checked Camera.get_orthographicSize".to_owned(),
                    });
                }
                // SAFETY: guest code pages; the patch window keeps execute permission.
                let installed = unsafe {
                    let installed = detour("ui_orthographic_width_safe", "Camera.set_orthographicSize", entry, &SET_ORTHO_ORIG, set_orthographic_size_hook);
                    if installed.is_ok() {
                        if let Some((entry, getter_mi)) = getter {
                            // This typed slot binds a validated existing native
                            // getter; both code and metadata stay process-pinned.
                            let getter = Original::<GetFloatFn>::new();
                            getter.set(entry);
                            *lock(&ORTHO_CALLS) = getter.get().map(|get| OrthoCalls {
                                get, get_mi: getter_mi.as_ptr() as usize,
                                set_mi: setter_mi.as_ptr() as usize,
                            });
                        }
                    }
                    installed
                };
                match installed {
                    Ok(installed) => {
                        events.push(InstallEvent::Installed {
                            behavior: "ui_orthographic_width_safe",
                            target: "Camera.set_orthographicSize".to_owned(),
                            shape: installed.shape,
                        });
                    }
                    Err(event) => events.push(event),
                }
            }
            None => events.push(InstallEvent::TargetMissing {
                behavior: "ui_orthographic_width_safe",
                target: "Camera.set_orthographicSize".to_owned(),
            }),
        }
    }

    // tk2d field offsets (reference resolve_field_offset flow).
    let camera_settings = field_offset(api, asm_image, "", "tk2dCamera", "cameraSettings");
    let zoom = field_offset(api, asm_image, "", "tk2dCamera", "zoomFactor");
    let ortho = field_offset(api, asm_image, "", "tk2dCameraSettings", "orthographicSize");
    let ppm = field_offset(api, asm_image, "", "tk2dCameraSettings", "orthographicPixelsPerMeter");
    match (camera_settings, zoom, ortho, ppm) {
        (Some(camera_settings), Some(zoom), Some(ortho), Some(ppm)) => {
            *lock(&TK2D_OFFSETS) = Some(Tk2dOffsets { camera_settings, zoom, ortho, ppm });
            hk_log_raw(&format!(
                "tk2dCamera fields settings={camera_settings:#x} zoom={zoom:#x}; \
                 tk2dCameraSettings ortho={ortho:#x} ppm={ppm:#x}"
            ));
        }
        _ => events.push(InstallEvent::TargetMissing {
            behavior: "tk2d_width_safe",
            target: "tk2dCamera/tk2dCameraSettings field offsets".to_owned(),
        }),
    }

    // tk2dCamera.UpdateCameraMatrix (tk2d_width_safe OR debug).
    if features.tk2d_width_safe || features.debug {
        match method_entry(api, asm_image, "", "tk2dCamera", "UpdateCameraMatrix", 0) {
            Some((entry, _mi)) => {
                // SAFETY: guest code pages; the patch window keeps execute permission.
                match unsafe { detour("tk2d_width_safe", "tk2dCamera.UpdateCameraMatrix", entry, &TK2D_MATRIX_ORIG, tk2d_update_camera_matrix_hook) } {
                    Ok(installed) => {
                        events.push(InstallEvent::Installed {
                            behavior: "tk2d_width_safe",
                            target: "tk2dCamera.UpdateCameraMatrix".to_owned(),
                            shape: installed.shape,
                        });
                    }
                    Err(event) => events.push(event),
                }
            }
            None => events.push(InstallEvent::TargetMissing {
                behavior: "tk2d_width_safe",
                target: "tk2dCamera.UpdateCameraMatrix".to_owned(),
            }),
        }
    } else {
        events.push(InstallEvent::Skipped {
            behavior: "tk2d_width_safe",
            reason: "switch off",
        });
    }

    // The reference always-on group: SetOverscanViewport, AutoScaleViewport,
    // SetOverscan (missing targets log and continue in the oracle too).
    for (class, method, argc, hook, behavior) in [
        (
            "ForceCameraAspect",
            "SetOverscanViewport",
            1,
            AlwaysOnSlot::Float(set_overscan_viewport_hook, &OVERSCAN_VP_ORIG),
            "overscan pass-through logging",
        ),
        (
            "ForceCameraAspect",
            "AutoScaleViewport",
            0,
            AlwaysOnSlot::Auto(auto_scale_hook, &AUTO_ORIG),
            "force_full_viewport (AutoScaleViewport override)",
        ),
        (
            "GameCameras",
            "SetOverscan",
            1,
            AlwaysOnSlot::Float(set_overscan_hook, &SET_OVERSCAN_ORIG),
            "overscan pass-through logging",
        ),
    ] {
        match method_entry(api, asm_image, "", class, method, argc) {
            Some((entry, _mi)) => {
                // SAFETY: guest code pages; the patch window keeps execute permission.
                match unsafe { hook.install(behavior, &format!("{class}.{method}"), entry) } {
                    Ok(installed) => {
                        hk_log_raw(&format!("hooked {class}.{method}({argc}) @ {:#x}", installed.target));
                        events.push(InstallEvent::Installed {
                            behavior,
                            target: format!("{class}.{method}"),
                            shape: installed.shape,
                        });
                    }
                    Err(event) => events.push(event),
                }
            }
            None => events.push(InstallEvent::TargetMissing {
                behavior,
                target: format!("{class}.{method}"),
            }),
        }
    }

    events
}

// ---------------------------------------------------------------------------
// C ABI: init (ungated), module subscription, install on the il2cpp event.
// ---------------------------------------------------------------------------

struct ArmedState {
    api: HostApiV0,
    features: ViewportFeatures,
}

/// Armed states, one per init (the subscription `user` handles). Re-init
/// re-arms for the in-process test suite; the loader brings a cdylib up
/// exactly once. Entries live for the process.
static ARMED: HandleTable<ArmedState> = HandleTable::new();

fn api_log(api: &HostApiV0, message: &str) {
    // NUL rejection preserved (skip, never escape) for this call site.
    if message.contains('\0') {
        return;
    }
    api.log_line(TAG, message);
}

/// Module-loaded callback: bind + install on the il2cpp module — the kit
/// analogue of the reference `register_il2cpp_post_init` trigger.
/// The bound IL2CPP surface from the Mapped event, kept for the
/// Il2cppReady callback (ticket 78): every hook target resolves through the
/// guest metadata API, so installs wait for `il2cpp_init` to return — the
/// reference `register_il2cpp_post_init` trigger point.
static IL2CPP: Mutex<Option<Il2cpp>> = Mutex::new(None);

extern "C" fn on_module_loaded(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(armed)) = (event, ARMED.get(user)) else {
        return;
    };
    if event.kind != ModuleKind::Il2cpp as u32 {
        return;
    }
    // SAFETY: an Il2cpp module event reports the load base of the mapped,
    // never-unmapped guest libil2cpp; `so_symbol` is the loader's resolver.
    let bound = unsafe { Il2cpp::bind(event.base, armed.api.so_symbol, REQUIRED_EXPORTS) };
    match bound {
        Ok(api) => {
            *lock(&IL2CPP) = Some(api);
            api_log(
                &armed.api,
                "il2cpp module bound: waiting for il2cpp_init (Il2cppReady) before installing",
            );
        }
        Err(missing) => {
            api_log(
                &armed.api,
                &format!(
                    "missing il2cpp exports; disabled ({} missing): {missing:?}",
                    missing.len()
                ),
            );
        }
    }
}

/// Il2cppReady callback (ticket 78): metadata is usable — run the reference
/// install chain against the binding stored at Mapped time.
extern "C" fn on_il2cpp_ready(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(armed)) = (event, ARMED.get(user)) else {
        return;
    };
    if event.kind != ModuleKind::Il2cppReady as u32 {
        return;
    }
    let binding = lock(&IL2CPP).take();
    let Some(api) = binding else {
        api_log(
            &armed.api,
            "il2cpp_ready before the il2cpp module mapped — ignoring (no hooks installed)",
        );
        return;
    };
    api_log(
        &armed.api,
        &format!(
            "plugin armed: full_viewport={} tk2d_width_safe={} ui_ortho_safe={} debug={}",
            armed.features.force_full_viewport as c_int,
            armed.features.tk2d_width_safe as c_int,
            armed.features.ui_orthographic_width_safe as c_int,
            armed.features.debug as c_int,
        ),
    );
    // SAFETY: Il2cppReady runs right after `il2cpp_init` returned, before
    // managed code executes the targets; patch windows keep execute permission.
    let events = unsafe { install(&api, &armed.features) };
        for event in &events {
        api_log(&armed.api, &event.log_line());
    }
}

/// Plugin init: version check, feature read (legacy + new dialect), module
/// subscription. Ungated by reference parity — see the module docs for the
/// gate-conflict decision and its deployment-isolation answer.
///
/// # Safety
/// The same-kit host must pin both modules and provide correctly typed native
/// exports and module events. Ready callbacks run on the Unity thread with
/// every hook target idle through installation and original publication.
pub unsafe fn plugin_init(api: &HostApiV0) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let features = ViewportFeatures::from_host_api(api);
    // Ungated (reference parity): log the activation explicitly instead of a
    // package check. The kit validate step never places this patch in a
    // mismatched Title kit — deployments isolate gamepatch sets per Title.
    api_log(
        api,
        "ungated reference parity: presence in the deploy dir is the activation switch",
    );
    *lock(&HOST_LOG) = Some(*api);

    let user = ARMED.insert(ArmedState {
        api: *api,
        features,
    });
    // Ticket 78: two subscriptions — Mapped (bind the exports) and
    // Il2cppReady (metadata usable, run the reference install chain).
    let registered_mapped = api.on_module_loaded(ModuleKind::Il2cpp, on_module_loaded, user);
    let registered_ready = api.on_module_loaded(ModuleKind::Il2cppReady, on_il2cpp_ready, user);
    if !registered_mapped || !registered_ready {
        api_log(api, "module subscription rejected — patch not armed");
        return -3;
    }
    0
}

#[no_mangle]
/// # Safety
/// `api` is the process-pinned same-kit host table; symbol lookup and emitted
/// module events identify live, correctly typed guest exports.
pub unsafe extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    // SAFETY: the native entry requires the same host and lifecycle contract.
    unsafe { plugin_init(api) }
}

/// Install liveness for the integration suite (no guest memory poking).
pub fn hook_slots() -> (bool, bool, bool, bool, bool) {
    (
        SET_RECT_ORIG.address() != 0,
        SET_ORTHO_ORIG.address() != 0,
        TK2D_MATRIX_ORIG.address() != 0,
        AUTO_ORIG.address() != 0,
        lock(&TK2D_OFFSETS).is_some(),
    )
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
