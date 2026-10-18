//! INPUT:  shared JNI, package-gated IL2CPP events, Silksong features and typed detours
//! OUTPUT:  package path, startup/language/scene hooks, postprocess and title bundle policies
//! POS:    Title adaptation with typed hooks and player-thread resource ownership
//! rudroid-unity-gamepatch-silksong: Silksong Title gamepatch (ticket 30;
//! real patch behavior ticket 54).
//!
//! Merges the reference `silksong_unity6` (package-gated player-loop driver)
//! and `silksong_viewport` patches into one capability; the split back into
//! separate libraries only matters if both must load independently, which no
//! current deploy profile does. The package gate matches
//! `com.game.silksong` against the Host Config `[package].packageName`
//! exactly like the reference `RUDROID_PLUGIN_TARGET_PACKAGE`.
//!
//! Ticket 54 turns the former config-only skeleton into the reference's
//! actual behavior chain, against the read-only oracle
//! `projects/unityloader/gamepatchs/{silksong_unity6,silksong_viewport}`:
//!
//! 1. `rudroid_plugin_init` — reads the full `[game_patches.silksong_unity6]`
//!    feature table through the Host API v0 getters (the reference
//!    `game_patch::Context::config_bool` consumption path), applies the
//!    package gate in-process (the kit gate also refuses the descriptor at
//!    validate time), and subscribes to the module-loaded surface.
//! 2. On the il2cpp module event — the kit's analogue of the reference
//!    `register_il2cpp_post_init` trigger — the patch binds the guest's
//!    `il2cpp_*` exports through `so_symbol`, resolves the reference target
//!    methods through the metadata API (class/namespace/method-name/argc,
//!    method entry = `*(uintptr_t*)mi`), and installs REAL inline-code
//!    detours (the shared `rudroid_common::arm64_detour`).
//! 3. The hook bodies implement the reference semantics: suppressed
//!    Application.Quit, ambient-particle controller forced through its
//!    disable path, no-op audio preloader and decorative Update, and the
//!    merged viewport pipeline that removes the native aspect lower bound.
//!
//! Opt-in after-title bundle reclamation runs from successful render events,
//! only in Menu_Title, once, with scene-exit and stop cancellation.
//!
//! Title feature flags come from the device config's
//! `[game_patches.silksong_unity6]` section — no offsets live in this crate.
//! Remaining touch-manager, census, destruction and unused-asset behavior is registered
//! in [`plan`] with its trigger,
//! target matching and default, and stays an explicit registered-only entry
//! until its runtime surface (runtime_invoke chains, render-frame callbacks)
//! exists in the kit.

#![deny(unsafe_op_in_unsafe_fn)]

use std::cell::Cell;
use std::ffi::{c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use rudroid_common::handles::HandleTable;
use rudroid_common::il2cpp::{Il2cpp, Method, StaticI32Method};
use rudroid_common::registry::RudroidModuleEvent;
use rudroid_common::{package_gate, Kind, ModuleKind};
use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_host_config::HostConfig;

// The detour machinery is shared through rudroid-common (cfg-gated to aarch64).
use rudroid_common::arm64_detour::{self as arm64, Original};

pub use arm64::HookShape;
use rudroid_common::gamepatch_support::install_detour as detour;

mod after_title;
mod java;
mod launcher;
mod cinematic;
mod postprocess;
mod bootstrap;
mod language;

// The hooks install on `Il2cppReady`, which only il2cpp_patch fires.
rudroid_common::declare_descriptor!(
    "silksong_unity6",
    Kind::GamePatch,
    package = Some("com.game.silksong"),
    requires = Some("il2cpp_patch")
);

/// Reference parity package gate.
pub const TARGET_PACKAGE: &str = "com.game.silksong";

/// Host log tag (reference `SILK` / `SSVIEW`).
const TAG: &CStr = c"SILK";

/// Viewport sub-patch log tag (reference `SSVIEW`).
const VIEWPORT_TAG: &CStr = c"SSVIEW";

/// Does this patch apply to the given Host Config package?
pub fn applies_to(config: &HostConfig) -> bool {
    package_gate(Some(TARGET_PACKAGE), config.package().package_name.as_deref())
}

/// The `[game_patches.silksong_unity6]` config prefix.
const FEATURE_PREFIX: &str = "game_patches.silksong_unity6.";

/// Config-driven Silksong feature flags. Every flag reads with its documented
/// default (the reference `config_bool`/`config_get_i64` fallbacks); unknown
/// configs stay compatible.
///
/// The full reference flag set (ticket 54) — before this ticket the table
/// only covered the pruning flags. `decorative_sprite_percent` follows the
/// reference acceptance window: only 25..=100 is taken, anything else keeps
/// the default 100 (the previous `clamp(0, 100)` was a divergence and is
/// fixed here). `viewport_enable` is the one NEW key: it can turn the merged
/// `silksong_viewport` half off without splitting the library back into two
/// .so files (the reference activated that patch purely by presence; the
/// default true matches that).
#[derive(Debug, Clone, PartialEq)]
pub struct SilksongFeatures {
    pub suppress_application_quit: bool,
    pub skip_launcher_disclaimer: bool,
    pub skip_apkvision_signature_check: bool,
    pub load_main_menu_directly: bool,
    pub skip_unsupported_cinematics: bool,
    pub disable_touch_camera: bool,
    pub disable_touch_controls: bool,
    pub disable_watermark_canvas: bool,
    pub destroy_bootstrap_objects: bool,
    pub freeze_decorative_behaviours: bool,
    pub decorative_sprite_percent: i64,
    pub disable_ambient_particles: bool,
    pub disable_heavy_postprocess: bool,
    pub disable_bloom: bool,
    pub disable_camera_blur_plane: bool,
    pub disable_light_blur: bool,
    pub disable_light_blurred_background: bool,
    pub disable_uber_postprocess: bool,
    pub resource_census: bool,
    pub unload_unused_assets_after_prune: bool,
    pub unload_assetbundles_after_title: bool,
    pub assetbundle_unload_delay_frames: i64,
    pub disable_audio_preloader: bool,
    pub trace_resource_load_methods: bool,
    pub trace_audio_clip_records: bool,
    /// Merged `silksong_viewport` half (NEW key, default true = the
    /// reference's presence-activation).
    pub viewport_enable: bool,
}

impl SilksongFeatures {
    /// The OR-chain the reference builds for the postprocess group:
    /// `disable_heavy_postprocess` turns on bloom/camera-blur/light-blur and
    /// uber-postprocess pruning as well.
    pub fn postprocess_pruning_enabled(&self) -> bool {
        self.disable_bloom
            || self.disable_camera_blur_plane
            || self.disable_light_blur
            || self.disable_light_blurred_background
            || self.disable_uber_postprocess
    }

    /// Read through the HostConfig object (in-process callers and tests).
    pub fn from_config(config: &HostConfig) -> SilksongFeatures {
        let flag = |key: &str, default: bool| {
            config.get_bool(&format!("{FEATURE_PREFIX}{key}"), default)
        };
        let disable_heavy_postprocess = flag("disable_heavy_postprocess", false);
        SilksongFeatures {
            suppress_application_quit: flag("suppress_application_quit", true),
            skip_launcher_disclaimer: flag("skip_launcher_disclaimer", true),
            skip_apkvision_signature_check: flag("skip_apkvision_signature_check", true),
            load_main_menu_directly: flag("load_main_menu_directly", true),
            skip_unsupported_cinematics: flag("skip_unsupported_cinematics", true),
            disable_touch_camera: flag("disable_touch_camera", false),
            disable_touch_controls: flag("disable_touch_controls", false),
            disable_watermark_canvas: flag("disable_watermark_canvas", false),
            destroy_bootstrap_objects: flag("destroy_bootstrap_objects", false),
            freeze_decorative_behaviours: flag("freeze_decorative_behaviours", false),
            // Reference window: 25..=100 is accepted, everything else keeps
            // the 100 default (no pruning).
            decorative_sprite_percent: match config
                .get_i64(&format!("{FEATURE_PREFIX}decorative_sprite_percent"), 100)
            {
                value if (25..=100).contains(&value) => value,
                _ => 100,
            },
            disable_ambient_particles: flag("disable_ambient_particles", false),
            disable_heavy_postprocess,
            disable_bloom: disable_heavy_postprocess || flag("disable_bloom", false),
            disable_camera_blur_plane: disable_heavy_postprocess
                || flag("disable_camera_blur_plane", false),
            disable_light_blur: disable_heavy_postprocess || flag("disable_light_blur", false),
            disable_light_blurred_background: flag("disable_light_blurred_background", false),
            disable_uber_postprocess: disable_heavy_postprocess
                || flag("disable_uber_postprocess", false),
            resource_census: flag("resource_census", false),
            unload_unused_assets_after_prune: flag("unload_unused_assets_after_prune", false),
            unload_assetbundles_after_title: flag("unload_assetbundles_after_title", false),
            assetbundle_unload_delay_frames: match config.get_i64(
                &format!("{FEATURE_PREFIX}assetbundle_unload_delay_frames"),
                600,
            ) {
                value if (60..=6000).contains(&value) => value,
                _ => 600,
            },
            disable_audio_preloader: flag("disable_audio_preloader", !config.memory_policy().unwrap_or_default().preload_audio),
            trace_resource_load_methods: flag("trace_resource_load_methods", false),
            trace_audio_clip_records: flag("trace_audio_clip_records", false),
            viewport_enable: config.get_bool("game_patches.silksong_viewport.enable", true),
        }
    }

    /// Read through the Host API v0 getters — the exact consumption path of
    /// the reference `game_patch::Context` (dotted-key getters only).
    pub fn from_host_api(api: &HostApiV0) -> SilksongFeatures {
        fn flag(api: &HostApiV0, key: &str, default: bool) -> bool {
            match CString::new(key) {
                Ok(key) => api.config_bool(&key, default),
                Err(_) => default,
            }
        }
        fn number(api: &HostApiV0, key: &str, default: i64, window: std::ops::RangeInclusive<i64>) -> i64 {
            let value = match CString::new(key) {
                Ok(key) => api.config_i64(&key, default),
                Err(_) => default,
            };
            if window.contains(&value) { value } else { default }
        }
        let key = |name: &str| format!("{FEATURE_PREFIX}{name}");
        let disable_heavy_postprocess = flag(api, &key("disable_heavy_postprocess"), false);
        SilksongFeatures {
            suppress_application_quit: flag(api, &key("suppress_application_quit"), true),
            skip_launcher_disclaimer: flag(api, &key("skip_launcher_disclaimer"), true),
            skip_apkvision_signature_check: flag(api, &key("skip_apkvision_signature_check"), true),
            load_main_menu_directly: flag(api, &key("load_main_menu_directly"), true),
            skip_unsupported_cinematics: flag(api, &key("skip_unsupported_cinematics"), true),
            disable_touch_camera: flag(api, &key("disable_touch_camera"), false),
            disable_touch_controls: flag(api, &key("disable_touch_controls"), false),
            disable_watermark_canvas: flag(api, &key("disable_watermark_canvas"), false),
            destroy_bootstrap_objects: flag(api, &key("destroy_bootstrap_objects"), false),
            freeze_decorative_behaviours: flag(api, &key("freeze_decorative_behaviours"), false),
            decorative_sprite_percent: number(
                api,
                &key("decorative_sprite_percent"),
                100,
                25..=100,
            ),
            disable_ambient_particles: flag(api, &key("disable_ambient_particles"), false),
            disable_heavy_postprocess,
            disable_bloom: disable_heavy_postprocess || flag(api, &key("disable_bloom"), false),
            disable_camera_blur_plane: disable_heavy_postprocess
                || flag(api, &key("disable_camera_blur_plane"), false),
            disable_light_blur: disable_heavy_postprocess || flag(api, &key("disable_light_blur"), false),
            disable_light_blurred_background: flag(
                api,
                &key("disable_light_blurred_background"),
                false,
            ),
            disable_uber_postprocess: disable_heavy_postprocess
                || flag(api, &key("disable_uber_postprocess"), false),
            resource_census: flag(api, &key("resource_census"), false),
            unload_unused_assets_after_prune: flag(api, &key("unload_unused_assets_after_prune"), false),
            unload_assetbundles_after_title: flag(api, &key("unload_assetbundles_after_title"), false),
            assetbundle_unload_delay_frames: number(
                api,
                &key("assetbundle_unload_delay_frames"),
                600,
                60..=6000,
            ),
            disable_audio_preloader: flag(api, &key("disable_audio_preloader"), !flag(api, "runtime.preload_audio", true)),
            trace_resource_load_methods: flag(api, &key("trace_resource_load_methods"), false),
            trace_audio_clip_records: flag(api, &key("trace_audio_clip_records"), false),
            viewport_enable: flag(api, "game_patches.silksong_viewport.enable", true),
        }
    }
}

// ---------------------------------------------------------------------------
// The behavior registry: every reference behavior with its trigger, target
// matching and default (ticket 54 acceptance item 1). `Hook` entries are
// installed for real; `RegisteredOnly` entries carry the reason their runtime
// surface does not exist in the kit yet.
// ---------------------------------------------------------------------------

/// Reference trigger events (see the ticket-54 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The reference `register_il2cpp_post_init` callback. Kit analogue: the
    /// module-loaded event of the il2cpp guest module, followed by
    /// `so_symbol` binding + metadata resolution + detour install.
    Il2cppPostInit,
    /// The reference `register_render_frame_callback` driver (delayed prune
    /// frames). The kit dispatches UnityFrame after successful nativeRender.
    RenderFrameCallbacks,
}

/// How an entry behaves in this crate today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A real detour install against a resolved method entry (verified by the
    /// ticket-54 fixture suite).
    Hook(HookBody),
    /// Managed engine operation driven by the player lifecycle.
    Runtime(&'static str),
    /// Registered with the exact reference spec; not installed. The reason is
    /// stable text so tests and the ticket table can assert it.
    RegisteredOnly(&'static str),
}

/// The hook bodies this crate actually carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookBody {
    /// Application.Quit() suppressed outside the checked user-menu call chain.
    QuitNoArgs,
    /// Application.Quit(int) uses the same scoped user-menu exception.
    QuitWithCode,
    /// SceneParticlesController.EnableParticles → orig(true) + DisableParticles
    /// (reference `enable_scene_particles_hook`).
    ParticlesDisable,
    /// AudioPreloader.PreloadClip → no-op (reference `audio_preload_clip_noop`).
    AudioPreloaderNoop,
    /// Decorative Update methods → no-op (reference `decorative_update_noop`).
    DecorativeUpdateNoop,
    /// ForceCameraAspect.AutoScaleViewport scope + MinMaxFloat.GetClampedBetween
    /// lower-bound removal (the merged `silksong_viewport` patch).
    ViewportUnclamp,
}

/// One registered behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanEntry {
    pub behavior: &'static str,
    /// The config key that gates it (empty = always installed when the target
    /// resolves, exactly like the reference).
    pub config_key: &'static str,
    /// Reference default of the gating switch.
    pub reference_default: bool,
    pub trigger: Trigger,
    /// Target matching: `(namespace, class, method, argc)`; namespace "" for
    /// global-name lookup (`find_class_named`), or a static description.
    pub target: &'static str,
    pub action: Action,
}

/// The behavior registry, in reference install order. `features` decides the
/// `enabled` view of each entry.
pub fn plan(features: &SilksongFeatures) -> Vec<PlanEntry> {
    let enabled = |on: bool| on;
    vec![
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "suppress_application_quit",
                config_key: "suppress_application_quit",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "UnityEngine.Application.Quit() / Quit(int); Silksong UIManager.QuitGame() scope",
                action: Action::Hook(HookBody::QuitNoArgs),
            },
            // The hook INSTALL is unconditional (reference); the switch only
            // gates the swallow at call time.
            enabled: true,
            runtime_switch: features.suppress_application_quit,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "disable_ambient_particles",
                config_key: "disable_ambient_particles",
                reference_default: false,
                trigger: Trigger::Il2cppPostInit,
                target: "SceneParticlesController.EnableParticles(bool)",
                action: Action::Hook(HookBody::ParticlesDisable),
            },
            enabled: enabled(features.disable_ambient_particles),
            runtime_switch: features.disable_ambient_particles,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "disable_audio_preloader",
                config_key: "disable_audio_preloader",
                reference_default: false,
                trigger: Trigger::Il2cppPostInit,
                target: "AudioPreloader.PreloadClip(1)",
                action: Action::Hook(HookBody::AudioPreloaderNoop),
            },
            enabled: enabled(features.disable_audio_preloader),
            runtime_switch: features.disable_audio_preloader,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "freeze_decorative_behaviours",
                config_key: "freeze_decorative_behaviours",
                reference_default: false,
                trigger: Trigger::Il2cppPostInit,
                target: "MossClump.Update / GrassBehaviour.{BatchedLateUpdate,BatchedFixedUpdate} / JitterSelf.BatchedUpdate",
                action: Action::Hook(HookBody::DecorativeUpdateNoop),
            },
            enabled: enabled(features.freeze_decorative_behaviours),
            runtime_switch: features.freeze_decorative_behaviours,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "silksong_viewport (merged reference patch)",
                config_key: "game_patches.silksong_viewport.enable (new key; the reference had none)",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "ForceCameraAspect.AutoScaleViewport + TeamCherry.SharedUtils.MinMaxFloat.GetClampedBetween(float)",
                action: Action::Hook(HookBody::ViewportUnclamp),
            },
            enabled: enabled(features.viewport_enable),
            runtime_switch: features.viewport_enable,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "skip_launcher_disclaimer + load_main_menu_directly",
                config_key: "skip_launcher_disclaimer / load_main_menu_directly",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "GameLauncher.Start/Update, CustomSceneManager chains via il2cpp_runtime_invoke + field offsets",
                action: Action::Runtime("metadata-checked button callback and deferred initialization-scene request; no managed objects retained across frames"),
            },
            enabled: enabled(features.skip_launcher_disclaimer),
            runtime_switch: features.skip_launcher_disclaimer,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "skip_unsupported_cinematics",
                config_key: "skip_unsupported_cinematics",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "CinematicSequence.Begin → Skip",
                action: Action::Runtime("validated Begin preserves setup, then invokes normal Skip for the unsupported Android video path"),
            },
            enabled: enabled(features.skip_unsupported_cinematics),
            runtime_switch: features.skip_unsupported_cinematics,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "skip_apkvision_signature_check",
                config_key: "skip_apkvision_signature_check",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "GameRunner.$a (APKVISION launcher policy)",
                action: Action::RegisteredOnly(
                    "launcher signature-check bypass is not provided; legitimate signer/game input is required",
                ),
            },
            enabled: enabled(features.skip_apkvision_signature_check),
            runtime_switch: features.skip_apkvision_signature_check,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "touch-manager controls compatibility",
                config_key: "(none — unconditional when the class resolves)",
                reference_default: true,
                trigger: Trigger::Il2cppPostInit,
                target: "InControl.TouchManager.set_controlsEnabled + touchControls/_controlsEnabled field offsets",
                action: Action::RegisteredOnly(
                    "owned by required il2cpp_patch: common installs the metadata-checked TouchManager adapter before ready subscribers",
                ),
            },
            enabled: true,
            runtime_switch: true,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "postprocess component pruning",
                config_key: "disable_bloom/-camera_blur_plane/-light_blur/-light_blurred_background/-uber_postprocess (or disable_heavy_postprocess)",
                reference_default: false,
                trigger: Trigger::Il2cppPostInit,
                target: "CustomSceneManager.Start delay + FindObjectsOfTypeAll+set_enabled chains",
                action: Action::Runtime(
                    "scene-start delay, signature-checked Behaviour enumeration and callback-local GC roots",
                ),
            },
            enabled: enabled(features.postprocess_pruning_enabled()),
            runtime_switch: features.postprocess_pruning_enabled(),
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "decorative sprite pruning",
                config_key: "decorative_sprite_percent",
                reference_default: true,
                trigger: Trigger::RenderFrameCallbacks,
                target: "Object.FindObjectsOfTypeAll(SpriteRenderer) + name prefix filter + set_enabled(false)",
                action: Action::Runtime(
                    "scene-start delay, active Sprite query, exact component and name filter; Renderer.enabled=false",
                ),
            },
            enabled: enabled(features.decorative_sprite_percent < 100),
            runtime_switch: features.decorative_sprite_percent < 100,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "bootstrap object teardown (touch camera/controls, watermark)",
                config_key: "disable_touch_camera / disable_touch_controls / disable_watermark_canvas",
                reference_default: false,
                trigger: Trigger::Il2cppPostInit,
                target: "accepted launcher initialization-scene request + GameObject.Find + SetActive(false)",
                action: Action::Runtime(
                    "exact reference name lists, checked methods and callback-local GC roots; no object destruction",
                ),
            },
            enabled: enabled(
                features.disable_touch_camera
                    || features.disable_touch_controls
                    || features.disable_watermark_canvas,
            ),
            runtime_switch: features.disable_touch_camera
                || features.disable_touch_controls
                || features.disable_watermark_canvas,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "bootstrap object destruction",
                config_key: "destroy_bootstrap_objects",
                reference_default: false,
                trigger: Trigger::Il2cppPostInit,
                target: "Object.Destroy after named deactivation",
                action: Action::RegisteredOnly("optional object destruction remains unimplemented; configured deactivation is supported"),
            },
            enabled: enabled(features.destroy_bootstrap_objects),
            runtime_switch: features.destroy_bootstrap_objects,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "resource census / method traces / audio record trace",
                config_key: "resource_census / trace_resource_load_methods / trace_audio_clip_records",
                reference_default: false,
                trigger: Trigger::Il2cppPostInit,
                target: "diagnostics logging over FindObjectsOfTypeAll + class method walks",
                action: Action::RegisteredOnly(
                    "resource census and method/audio trace adapters are not implemented",
                ),
            },
            enabled: enabled(
                features.resource_census
                    || features.trace_resource_load_methods
                    || features.trace_audio_clip_records,
            ),
            runtime_switch: features.resource_census
                || features.trace_resource_load_methods
                || features.trace_audio_clip_records,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "asset bundle unload",
                config_key: "unload_assetbundles_after_title / assetbundle_unload_delay_frames",
                reference_default: false,
                trigger: Trigger::RenderFrameCallbacks,
                target: "Menu_Title rendered-frame delay + AssetBundle.UnloadAllAssetBundles(false)",
                action: Action::Runtime("once-only, cancelled when leaving the title; uses validated engine APIs"),
            },
            enabled: features.unload_assetbundles_after_title,
            runtime_switch: features.unload_assetbundles_after_title,
        },
        PlanEntry {
            entry: PatchPlanEntry {
                behavior: "unused-asset unload after prune",
                config_key: "unload_unused_assets_after_prune",
                reference_default: false,
                trigger: Trigger::RenderFrameCallbacks,
                target: "prune completion + Resources.UnloadUnusedAssets",
                action: Action::RegisteredOnly(
                    "postprocess/decorative pruning has not been implemented",
                ),
            },
            enabled: features.unload_unused_assets_after_prune,
            runtime_switch: features.unload_unused_assets_after_prune,
        },
    ]
}

/// One registry row with the CURRENT config view attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub entry: PatchPlanEntry,
    /// Would the reference install this behavior with the current config?
    pub enabled: bool,
    /// The runtime switch value the hook bodies observe.
    pub runtime_switch: bool,
}

// ---------------------------------------------------------------------------
// Guest IL2CPP lookups over the shared `rudroid_common::il2cpp` binding.
// ---------------------------------------------------------------------------

/// The reference init's REQUIRED export set (the C++ patch returns -1 when
/// any of these is missing — mirror: bind fails and nothing installs).
const REQUIRED_EXPORTS: &[&str] = &[
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
    "il2cpp_image_get_class_count",
    "il2cpp_image_get_class",
    "il2cpp_class_from_name",
    "il2cpp_class_get_method_from_name",
    "il2cpp_class_get_type",
    "il2cpp_class_get_name",
    "il2cpp_class_get_namespace",
    "il2cpp_class_get_field_from_name",
    "il2cpp_field_get_offset",
    "il2cpp_field_get_value",
    "il2cpp_type_get_object",
    "il2cpp_object_get_class",
    "il2cpp_runtime_invoke",
    "il2cpp_string_new",
];

/// Resolve `(class, method, argc)` to the native entry the reference reads
/// out of the MethodInfo (`*(uintptr_t*)mi`): the class by namespace in any
/// image (`find_class_in_domain`), else by bare name over every image's
/// class list (`find_class_named`). Returns the entry and the MethodInfo.
fn method_entry(api: &Il2cpp, ns: &str, class: &str, method: &str, argc: c_int) -> Option<(usize, Method)> {
    let (static_method, params, returns): (bool, &[(i32, bool)], i32) = match (method, argc) {
        ("get_width" | "get_height", 0) => (true, &[], 8),
        ("Quit", 0) => (true, &[], 1),
        ("Quit", 1) => (true, &[(8, false)], 1),
        ("EnableParticles", 1) => (false, &[(2, false)], 1),
        ("DisableParticles" | "Begin" | "Skip" | "Update" | "BatchedLateUpdate" | "BatchedFixedUpdate" | "BatchedUpdate", 0) => (false, &[], 1),
        ("PreloadClip", 1) => (false, &[(0x12, false)], 1),
        ("AutoScaleViewport", 0) => (false, &[], 0x0c),
        ("GetClampedBetween", 1) => (false, &[(0x0c, false)], 0x0c),
        _ => return None,
    };
    let (ns, class, method) = (CString::new(ns).ok()?, CString::new(class).ok()?, CString::new(method).ok()?);
    let class = api.find_class(&ns, &class).or_else(|| api.find_class_named(&class))?;
    let mi = api.method(class, &method, argc)?;
    if !api.method_signature(mi, static_method, params, returns) { return None; }
    Some((api.method_entry(mi)?, mi))
}

// ---------------------------------------------------------------------------
// Install: resolve + real detours. Events double as the test/report surface.
// ---------------------------------------------------------------------------

/// One install outcome.
pub use rudroid_common::gamepatch_support::InstallEvent;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Hook bodies must never panic: a poisoned lock still hands out the data.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Trampoline (callable original) of the Application.Quit() detour.
static QUIT_NOARGS_ORIG: Original<QuitNoArgsFn> = Original::new();
/// Trampoline of the Application.Quit(int) detour.
static QUIT_CODE_ORIG: Original<QuitCodeFn> = Original::new();
/// Original confirmed menu action; its native body still owns exit preparation.
static MENU_QUIT_ORIG: Original<UpdateFn> = Original::new();
static QUIT_STEP_ORIG: Original<QuitStepFn> = Original::new();
/// The reference `g_suppress_quit` runtime switch.
static SUPPRESS_QUIT: AtomicBool = AtomicBool::new(true);
thread_local! {
    static MENU_QUIT_SCOPE: Cell<bool> = const { Cell::new(false) };
}
struct RestoreMenuQuitScope(bool);
impl Drop for RestoreMenuQuitScope {
    fn drop(&mut self) { MENU_QUIT_SCOPE.with(|scope| scope.set(self.0)); }
}
fn with_menu_quit_scope<R>(action: impl FnOnce() -> R) -> R {
    let previous = MENU_QUIT_SCOPE.with(|scope| scope.replace(true));
    let _restore = RestoreMenuQuitScope(previous);
    action()
}
/// SceneParticlesController context: trampoline of EnableParticles plus the
/// DisableParticles entry/MethodInfo (reference resolve-at-install pair).
/// Inert until `orig_enable` is installed.
static PARTICLES: ParticlesCtx = ParticlesCtx {
    orig_enable: Original::new(),
    disable_entry: Original::new(),
    disable_mi: AtomicUsize::new(0),
};
/// Typed entry holders plus the guest `MethodInfo*` kept as an address: all
/// atomics, so the static is `Sync` as is.
struct ParticlesCtx {
    orig_enable: Original<EnableFn>,
    disable_entry: Original<UpdateFn>,
    disable_mi: AtomicUsize,
}
/// AudioPreloader no-op marker (install evidence for the report).
static AUDIO_PRELOADER_HOOKED: AtomicBool = AtomicBool::new(false);
/// Decorative Update no-op marker.
static DECORATIVE_HOOKED: AtomicBool = AtomicBool::new(false);
/// Viewport context (the merged silksong_viewport patch); each hook stays
/// inert until its trampoline is installed.
static VIEWPORT: ViewportCtx = ViewportCtx {
    orig_auto: Original::new(),
    orig_clamp: Original::new(),
    screen: Mutex::new(None),
};
static VIEWPORT_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Reference `clamp_hook` `static unsigned reports`: the lower-bound log is
/// emitted at most `VIEWPORT_CLAMP_REPORT_LIMIT` times.
static VIEWPORT_CLAMP_REPORTS: AtomicU32 = AtomicU32::new(0);
const VIEWPORT_CLAMP_REPORT_LIMIT: u32 = 8;
/// See `ParticlesCtx`.
struct ViewportCtx {
    orig_auto: Original<AutoFn>,
    orig_clamp: Original<ClampFn>,
    screen: Mutex<Option<(StaticI32Method, StaticI32Method)>>,
}
// The AutoScaleViewport call scope (reference `thread_local AspectScope`):
// the aspect being computed and whether its clamp observation was consumed.
thread_local! {
    static VIEWPORT_SCOPE: Cell<Option<(f32, bool)>> = const { Cell::new(None) };
}

/// Host table snapshot the hook bodies log through (reference `g_api->log`).
static HOST_LOG: Mutex<Option<HostApiV0>> = Mutex::new(None);

fn tagged_log(tag: &CStr, message: &str) {
    let Some(api) = *lock(&HOST_LOG) else {
        return;
    };
    // NUL rejection preserved (skip, never escape) for the hook logs.
    if message.contains('\0') {
        return;
    }
    api.log_line(tag, message);
}

fn host_log_raw(message: &str) {
    tagged_log(TAG, message);
}

fn viewport_log_raw(message: &str) {
    tagged_log(VIEWPORT_TAG, message);
}

type EnableFn = unsafe extern "C-unwind" fn(*mut c_void, bool, *mut c_void);
type UpdateFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void);
type QuitNoArgsFn = unsafe extern "C-unwind" fn(*mut c_void);
type QuitCodeFn = unsafe extern "C-unwind" fn(i32, *mut c_void);
type QuitStepFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> bool;
type ClampFn = unsafe extern "C-unwind" fn(*mut c_void, f32, *mut c_void) -> f32;
type AutoFn = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> f32;

struct RestoreViewportScope(Option<(f32, bool)>);
impl Drop for RestoreViewportScope {
    fn drop(&mut self) { VIEWPORT_SCOPE.with(|scope| scope.set(self.0)); }
}

fn with_viewport_scope<R>(aspect: f32, action: impl FnOnce() -> R) -> R {
    let previous = VIEWPORT_SCOPE.with(|scope| scope.replace(Some((aspect, false))));
    let _restore = RestoreViewportScope(previous);
    action()
}

// ---- hook bodies (reference semantics) ----

/// Preserve Android startup suppression, but let the confirmed native menu
/// action reach the engine. Never terminate from inside a managed hook.
unsafe extern "C-unwind" fn quit_no_args_hook(method: *mut c_void) {
    if SUPPRESS_QUIT.load(Ordering::Relaxed) && !MENU_QUIT_SCOPE.with(Cell::get) {
        host_log_raw("Application.Quit() suppressed");
        return;
    }
    if let Some(orig) = QUIT_NOARGS_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        unsafe { orig(method) };
    }
}

/// Reference `quit_with_code_hook`.
unsafe extern "C-unwind" fn quit_with_code_hook(exit_code: i32, method: *mut c_void) {
    if SUPPRESS_QUIT.load(Ordering::Relaxed) && !MENU_QUIT_SCOPE.with(Cell::get) {
        host_log_raw(&format!("Application.Quit({exit_code}) suppressed"));
        return;
    }
    if let Some(orig) = QUIT_CODE_ORIG.get() {
        // SAFETY: the trampoline replays the saved prologue.
        unsafe { orig(exit_code, method) };
    }
}

/// The exact instance void UIManager.QuitGame() action in Silksong.dll.
/// No receiver or managed object survives the original call; the scope is
/// thread-local and restores its previous value on return or managed unwind.
unsafe extern "C-unwind" fn menu_quit_hook(receiver: *mut c_void, method: *mut c_void) {
    let Some(original) = MENU_QUIT_ORIG.get() else { return; };
    host_log_raw("confirmed menu quit forwarded through the original action");
    with_menu_quit_scope(|| {
        // SAFETY: typed original of the metadata-checked instance void/zero-arg
        // method; guest receiver and MethodInfo stay within this callback.
        unsafe { original(receiver, method) };
    });
}

/// A coroutine advances after the UI callback returns. Only its checked
/// MoveNext call chain gets the exemption, never the time between its steps.
unsafe extern "C-unwind" fn quit_step_hook(receiver: *mut c_void, method: *mut c_void) -> bool {
    let Some(original) = QUIT_STEP_ORIG.get() else { return false; };
    with_menu_quit_scope(|| {
        // SAFETY: exact non-generic instance bool/zero-arg iterator metadata;
        // callback-owned receiver/MethodInfo, no reference retained by Rust.
        unsafe { original(receiver, method) }
    })
}

fn deferred_quit_entry(api: &Il2cpp) -> Option<usize> {
    let image = api.exact_image("Silksong.dll")?;
    let manager = api.class_from_name(image, c"", c"GameManager")?;
    let factory = api.unique_method(manager, c"QuitGame", 0)?;
    if !api.ordinary_method(factory) || !api.method_signature(factory, false, &[], 0x12) { return None; }
    let returns = api.class_from_type(api.method_return_type(factory)?)?;
    if api.class_namespace(returns).as_deref() != Some("System.Collections")
        || api.class_name(returns).as_deref() != Some("IEnumerator") { return None; }
    let mut iterator = None;
    for nested in api.nested_classes(manager)? {
        let name = api.class_name(nested)?;
        let Some(ordinal) = name.strip_prefix("<QuitGame>d__") else { continue; };
        if ordinal.is_empty() || !ordinal.bytes().all(|c| c.is_ascii_digit()) { continue; }
        if iterator.replace(nested).is_some() { return None; }
    }
    let step = api.unique_method(iterator?, c"MoveNext", 0)?;
    if !api.ordinary_method(step) || !api.method_signature(step, false, &[], 2) { return None; }
    api.method_entry(step)
}

/// Reference `enable_scene_particles_hook`: keep the controller's zone/state
/// setup by calling the ORIGINAL with `true` (its no-scene-particles path),
/// then finish with the game's own DisableParticles for all ambient roots.
unsafe extern "C-unwind" fn particles_enable_hook(self_: *mut c_void, _enable: bool, mi: *mut c_void) {
    let (Some(orig), Some(disable)) = (PARTICLES.orig_enable.get(), PARTICLES.disable_entry.get()) else {
        return;
    };
    // SAFETY: trampoline + resolved disable entry live in the guest module.
    unsafe {
        orig(self_, true, mi);
        disable(self_, PARTICLES.disable_mi.load(Ordering::Relaxed) as *mut c_void);
    }
    host_log_raw("ambient scene particles disabled via controller");
}

/// Reference `audio_preload_clip_noop`: the preloader is fully disabled.
extern "C-unwind" fn audio_preload_clip_noop(_self: *mut c_void, _clip: *mut c_void, _mi: *mut c_void) {
}

/// Reference `decorative_update_noop`: decorative behaviours freeze.
extern "C-unwind" fn decorative_update_noop(_self: *mut c_void, _mi: *mut c_void) {}

/// Reference silksong_viewport `clamp_hook`: the native clamp result is
/// returned UNLESS it raised the current AutoScaleViewport aspect above the
/// game's value — then the unclamped value flows through (the native lower
/// bound is removed; later subscriber calls stay native).
unsafe extern "C-unwind" fn viewport_clamp_hook(self_: *mut c_void, value: f32, mi: *mut c_void) -> f32 {
    let clamped = match VIEWPORT.orig_clamp.get() {
        // SAFETY: the trampoline replays the saved prologue.
        Some(orig) => unsafe { orig(self_, value, mi) },
        None => value,
    };
    let Some((scope_aspect, consumed)) = VIEWPORT_SCOPE.with(|scope| scope.get()) else {
        return clamped;
    };
    if VIEWPORT_ACTIVE.load(Ordering::Relaxed)
        && !consumed
        && value.is_finite()
        && (value - scope_aspect).abs() < 0.00001
    {
        VIEWPORT_SCOPE.with(|scope| scope.set(Some((scope_aspect, true))));
        if clamped > value {
            // Load first: past the limit the hot path does no RMW and the
            // counter cannot wrap back into logging.
            if VIEWPORT_CLAMP_REPORTS.load(Ordering::Relaxed) < VIEWPORT_CLAMP_REPORT_LIMIT
                && VIEWPORT_CLAMP_REPORTS.fetch_add(1, Ordering::Relaxed) < VIEWPORT_CLAMP_REPORT_LIMIT
            {
                viewport_log_raw(&format!(
                    "native aspect lower bound {:.4} -> {:.4}; native FOV/height/viewport events retained",
                    clamped, value
                ));
            }
            return value;
        }
    }
    clamped
}

/// Reference silksong_viewport `auto_hook`: open an aspect scope with the
/// CURRENT screen aspect, then let the native body compute rect/HUD/FOV and
/// all notifications — the return value is never overridden here.
unsafe extern "C-unwind" fn viewport_auto_hook(self_: *mut c_void, mi: *mut c_void) -> f32 {
    let screen = *lock(&VIEWPORT.screen);
    let (Some(orig), Some((screen_width, screen_height))) =
        (VIEWPORT.orig_auto.get(), screen)
    else {
        return 0.0;
    };
    // SAFETY: ABI-checked Screen methods; this managed hook runs on the Unity
    // thread. Copy the pair before calling guest code, with no lock held.
    let (width, height) = unsafe {
        (screen_width.call(), screen_height.call())
    };
    // SAFETY: the installed trampoline receives this guest call's live
    // receiver and metadata. The scope helper restores state during unwind.
    let call_original = || unsafe { orig(self_, mi) };
    if !VIEWPORT_ACTIVE.load(Ordering::Relaxed) || width <= 0 || height <= 0 {
        return call_original();
    }
    let aspect = width as f32 / height as f32;
    with_viewport_scope(aspect, call_original)
}


/// The merged silksong_viewport install (reference `install()`): Screen
/// width/height + AutoScaleViewport + GetClampedBetween, then the scope +
/// clamp hook pair. Failures never abort the remaining reference chain.
///
/// # Safety
/// Same contract as [`install`].
unsafe fn install_viewport(
    api: &Il2cpp,
    features: &SilksongFeatures,
    events: &mut Vec<InstallEvent>,
) {
    if !features.viewport_enable {
        events.push(InstallEvent::Skipped {
            behavior: "silksong_viewport",
            reason: "switch off",
        });
        viewport_log_raw("viewport disabled by game_patches.silksong_viewport.enable=false");
        return;
    }
    let width = method_entry(api, "UnityEngine", "Screen", "get_width", 0)
        .and_then(|(_, mi)| api.static_i32_method(mi));
    let height = method_entry(api, "UnityEngine", "Screen", "get_height", 0)
        .and_then(|(_, mi)| api.static_i32_method(mi));
    let auto = method_entry(api, "", "ForceCameraAspect", "AutoScaleViewport", 0);
    let clamp = method_entry(api, "TeamCherry.SharedUtils", "MinMaxFloat", "GetClampedBetween", 1);
    let (Some(width), Some(height), Some((auto_entry, _auto_mi)), Some((clamp_entry, _clamp_mi))) =
        (width, height, auto, clamp)
    else {
        // Reference: "metadata mismatch: native viewport unchanged".
        viewport_log_raw("metadata mismatch: native viewport unchanged");
        for (name, resolved) in [
            ("UnityEngine.Screen.get_width", width.is_some()),
            ("UnityEngine.Screen.get_height", height.is_some()),
            ("ForceCameraAspect.AutoScaleViewport", auto.is_some()),
            ("MinMaxFloat.GetClampedBetween", clamp.is_some()),
        ] {
            if !resolved {
                events.push(InstallEvent::TargetMissing {
                    behavior: "silksong_viewport",
                    target: name.to_owned(),
                });
            }
        }
        return;
    };
    VIEWPORT_CLAMP_REPORTS.store(0, Ordering::Relaxed);
    *lock(&VIEWPORT.screen) = Some((width, height));
    // Both hooks stay inert until their detours land.
    VIEWPORT.orig_auto.clear();
    VIEWPORT.orig_clamp.clear();
    // SAFETY: guest code pages; the patch window keeps execute permission.
    let clamp_detour = unsafe {
        detour(
            "silksong_viewport",
            "MinMaxFloat.GetClampedBetween",
            clamp_entry,
            &VIEWPORT.orig_clamp,
            viewport_clamp_hook,
        )
    };
    let clamp_detour = match clamp_detour {
        Ok(installed) => installed,
        Err(event) => {
            events.push(event);
            return;
        }
    };
    // SAFETY: guest code pages; the patch window keeps execute permission.
    let auto_detour = unsafe {
        detour(
            "silksong_viewport",
            "ForceCameraAspect.AutoScaleViewport",
            auto_entry,
            &VIEWPORT.orig_auto,
            viewport_auto_hook,
        )
    };
    let auto_detour = match auto_detour {
        Ok(installed) => installed,
        Err(event) => {
            // Reference parity: the clamp hook stays installed but inactive
            // (no restore path in the oracle either).
            events.push(event);
            return;
        }
    };
    VIEWPORT_ACTIVE.store(true, Ordering::Relaxed);
    viewport_log_raw("native aspect pipeline installed; no camera/zoom/blur overrides");
    events.push(InstallEvent::Installed {
        behavior: "silksong_viewport",
        target: "ForceCameraAspect.AutoScaleViewport".to_owned(),
        shape: auto_detour.shape,
    });
    events.push(InstallEvent::Installed {
        behavior: "silksong_viewport",
        target: "MinMaxFloat.GetClampedBetween".to_owned(),
        shape: clamp_detour.shape,
    });
}

/// Run the reference install chain against a bound IL2CPP API. Every outcome
/// is reported as an [`InstallEvent`] and nothing is installed for switches
/// that are off (ticket 54 negative contract).
///
/// # Safety
/// The guest and patch modules remain mapped for every hook call. Installation
/// runs on the Unity thread with all patched entries idle through publication;
/// managed receivers and arguments stay live through each forwarded call.
pub unsafe fn install(api: &Il2cpp, features: &SilksongFeatures) -> Vec<InstallEvent> {
    let mut events = Vec::new();

    // ---- ambient particles (reference install_ambient_particles_hook) ----
    if !features.disable_ambient_particles {
        events.push(InstallEvent::Skipped {
            behavior: "disable_ambient_particles",
            reason: "switch off",
        });
    } else if let Some((enable_entry, _enable_mi)) =
        method_entry(api, "", "SceneParticlesController", "EnableParticles", 1)
    {
        let disable = match method_entry(api, "", "SceneParticlesController", "DisableParticles", 0) {
            Some((entry, mi)) if entry != enable_entry => Some((entry, mi)),
            _ => None,
        };
        match disable {
            Some((disable_entry, disable_mi)) => {
                PARTICLES.disable_mi.store(disable_mi.as_ptr() as usize, Ordering::Relaxed);
                // SAFETY: the resolved native DisableParticles entry of the
                // mapped guest module.
                unsafe { PARTICLES.disable_entry.set(disable_entry) };
                // The hook stays inert until its detour lands.
                PARTICLES.orig_enable.clear();
                // SAFETY: guest code pages; the patch window keeps execute permission.
                match unsafe { detour("disable_ambient_particles", "SceneParticlesController.EnableParticles", enable_entry, &PARTICLES.orig_enable, particles_enable_hook) } {
                    Ok(installed) => {
                        events.push(InstallEvent::Installed {
                            behavior: "disable_ambient_particles",
                            target: "SceneParticlesController.EnableParticles".to_owned(),
                            shape: installed.shape,
                        });
                    }
                    Err(event) => events.push(event),
                }
            }
            None => events.push(InstallEvent::TargetMissing {
                behavior: "disable_ambient_particles",
                target: "SceneParticlesController.DisableParticles".to_owned(),
            }),
        }
    } else {
        events.push(InstallEvent::TargetMissing {
            behavior: "disable_ambient_particles",
            target: "SceneParticlesController.EnableParticles".to_owned(),
        });
    }

    // ---- audio preloader (reference install_audio_preloader_hook) ----
    if !features.disable_audio_preloader {
        events.push(InstallEvent::Skipped {
            behavior: "disable_audio_preloader",
            reason: "switch off",
        });
    } else {
        match method_entry(api, "", "AudioPreloader", "PreloadClip", 1) {
            Some((entry, _mi)) => {
                // SAFETY: mapped guest entry with the no-op's native ABI;
                // the caller supplies the patch window.
                let installed = unsafe { arm64::install(entry, audio_preload_clip_noop as usize) }
                    .map_err(|error| InstallEvent::DetourFailed {
                        behavior: "disable_audio_preloader",
                        target: "AudioPreloader.PreloadClip".to_owned(),
                        reason: error.to_string(),
                    });
                match installed {
                    Ok(installed) => {
                        AUDIO_PRELOADER_HOOKED.store(true, Ordering::Relaxed);
                        events.push(InstallEvent::Installed {
                            behavior: "disable_audio_preloader",
                            target: format!("AudioPreloader.PreloadClip @ {:#x}", installed.target),
                            shape: installed.shape,
                        });
                    }
                    Err(event) => events.push(event),
                }
            }
            None => events.push(InstallEvent::TargetMissing {
                behavior: "disable_audio_preloader",
                target: "AudioPreloader.PreloadClip".to_owned(),
            }),
        }
    }

    // ---- decorative freeze (reference freeze_decorative_behaviour_updates) ----
    if !features.freeze_decorative_behaviours {
        events.push(InstallEvent::Skipped {
            behavior: "freeze_decorative_behaviours",
            reason: "switch off",
        });
    } else {
        // The reference freezes FOUR methods with target dedup; the fixture
        // models MossClump.Update (the others stay TargetMissing unless the
        // guest metadata carries them).
        const DECORATIVE: [(&str, &str, c_int); 4] = [
            ("MossClump", "Update", 0),
            ("GrassBehaviour", "BatchedLateUpdate", 0),
            ("GrassBehaviour", "BatchedFixedUpdate", 0),
            ("JitterSelf", "BatchedUpdate", 0),
        ];
        let mut hooked_targets: Vec<usize> = Vec::new();
        let mut any_installed = false;
        for (class, method, argc) in DECORATIVE {
            let Some((entry, _mi)) = method_entry(api, "", class, method, argc) else {
                events.push(InstallEvent::TargetMissing {
                    behavior: "freeze_decorative_behaviours",
                    target: format!("{class}.{method}"),
                });
                continue;
            };
            if hooked_targets.contains(&entry) {
                continue; // reference duplicate-target skip
            }
            // SAFETY: mapped guest entry with the no-op's native ABI;
            // the caller supplies the patch window.
            let installed = unsafe { arm64::install(entry, decorative_update_noop as usize) }
                .map_err(|error| InstallEvent::DetourFailed {
                    behavior: "freeze_decorative_behaviours",
                    target: format!("{class}.{method}"),
                    reason: error.to_string(),
                });
            match installed {
                Ok(installed) => {
                    hooked_targets.push(entry);
                    any_installed = true;
                    host_log_raw(&format!("froze decorative method: {class}.{method}"));
                    events.push(InstallEvent::Installed {
                        behavior: "freeze_decorative_behaviours",
                        target: format!("{class}.{method} @ {:#x}", installed.target),
                        shape: installed.shape,
                    });
                }
                Err(event) => events.push(event),
            }
        }
        DECORATIVE_HOOKED.store(any_installed, Ordering::Relaxed);
        host_log_raw(&format!(
            "decorative Update methods frozen={}",
            hooked_targets.len()
        ));
    }

    // SAFETY: same quiescent patch window; launcher validates all callbacks,
    // reference fields and scene-call metadata before installing either hook.
    events.extend(unsafe { launcher::install(*api, features) });
    // SAFETY: same idle Il2cppReady window; language module validates its complete ABI.
    events.extend(unsafe { language::install(*api) });
    // SAFETY: same idle install window; both cinematic signatures checked.
    events.extend(unsafe { cinematic::install(*api, features) });
    // SAFETY: same idle install window; exact CustomSceneManager.Start instance signature checked.
    events.extend(unsafe { postprocess::install(*api, features.postprocess_pruning_enabled() || features.decorative_sprite_percent<100) });

    // ---- merged silksong_viewport (reference install()) ----
    // SAFETY: the caller's install contract (install_viewport resolves + detours only).
    unsafe { install_viewport(api, features, &mut events) };

    // A scoped exception for the real, confirmed menu action. Do not disable
    // suppression globally: this Android launcher also requests Quit at startup.
    let menu_quit = api.exact_image("Silksong.dll")
        .and_then(|image| api.class_from_name(image, c"", c"UIManager"))
        .and_then(|class| api.unique_method(class, c"QuitGame", 0))
        .filter(|&method| api.ordinary_method(method) && api.method_signature(method, false, &[], 1))
        .and_then(|method| api.method_entry(method));
    if let Some(entry) = menu_quit {
        // SAFETY: same quiescent install window, exact assembly and non-generic
        // instance void/zero-argument metadata; original publication is typed.
        match unsafe { detour("suppress_application_quit", "UIManager.QuitGame/0", entry, &MENU_QUIT_ORIG, menu_quit_hook) } {
            Ok(installed) => events.push(InstallEvent::Installed {
                behavior: "suppress_application_quit", target: "UIManager.QuitGame/0".to_owned(), shape: installed.shape,
            }),
            Err(event) => events.push(event),
        }
    } else {
        events.push(InstallEvent::TargetMissing {
            behavior: "suppress_application_quit", target: "Silksong UIManager.QuitGame/0".to_owned(),
        });
    }

    if let Some(entry) = deferred_quit_entry(api) {
        // SAFETY: same idle install window; factory return identity, nested
        // ownership, unique generated iterator and exact MoveNext ABI checked.
        match unsafe { detour("suppress_application_quit", "GameManager.QuitGame iterator MoveNext/0", entry, &QUIT_STEP_ORIG, quit_step_hook) } {
            Ok(installed) => events.push(InstallEvent::Installed {
                behavior: "suppress_application_quit", target: "GameManager.QuitGame iterator MoveNext/0".to_owned(), shape: installed.shape,
            }),
            Err(event) => events.push(event),
        }
    } else {
        events.push(InstallEvent::TargetMissing {
            behavior: "suppress_application_quit", target: "GameManager.QuitGame iterator MoveNext/0".to_owned(),
        });
    }

    // ---- Application.Quit suppression (reference post_init tail; the hook
    //      INSTALL is unconditional, the runtime switch gates the swallow) ----
    if api.find_class(c"UnityEngine", c"Application").is_some() {
        for (method, argc, behavior) in [
            ("Quit", 0, HookBody::QuitNoArgs),
            ("Quit", 1, HookBody::QuitWithCode),
        ] {
            // A missing entry or mismatched signature is the same miss.
            let Some((entry, _)) = method_entry(api, "UnityEngine", "Application", method, argc) else {
                events.push(InstallEvent::TargetMissing {
                    behavior: "suppress_application_quit",
                    target: format!("UnityEngine.Application.{method}({argc})"),
                });
                continue;
            };
            let target = format!("Application.{method}/{argc}");
            // SAFETY: mapped guest entry with the selected overload's ABI;
            // the install window keeps it idle until original publication.
            let installed = unsafe {
                match behavior {
                    HookBody::QuitNoArgs => detour("suppress_application_quit", &target, entry, &QUIT_NOARGS_ORIG, quit_no_args_hook),
                    HookBody::QuitWithCode => detour("suppress_application_quit", &target, entry, &QUIT_CODE_ORIG, quit_with_code_hook),
                    _ => unreachable!("only quit bodies install here"),
                }
            };
            match installed {
                Ok(installed) => {
                    events.push(InstallEvent::Installed {
                        behavior: "suppress_application_quit",
                        target: format!("UnityEngine.Application.{method}({argc})"),
                        shape: installed.shape,
                    });
                }
                Err(event) => events.push(event),
            }
        }
    } else {
        // Reference: post_init returns silently when Application is missing.
        events.push(InstallEvent::TargetMissing {
            behavior: "suppress_application_quit",
            target: "UnityEngine.Application".to_owned(),
        });
    }

    events
}

// ---------------------------------------------------------------------------
// C ABI: plugin init (gate + feature read + module subscription) and the
// module-loaded callback that binds + installs — the kit analogue of the
// reference `register_il2cpp_post_init` trigger.
// ---------------------------------------------------------------------------

struct ArmedState {
    api: HostApiV0,
    features: SilksongFeatures,
    observe_scenes: bool,
    title_unload: Option<after_title::Runtime>,
    postprocess: Option<postprocess::Runtime>,
}

/// Armed states, one per init (the subscription `user` handles). Re-init
/// re-arms for the in-process test suite; the loader brings a cdylib up
/// exactly once in production. Entries live for the process.
static ARMED: HandleTable<ArmedState> = HandleTable::new();

fn api_log(api: &HostApiV0, message: &str) {
    // NUL rejection preserved (skip, never escape) for this call site.
    if message.contains('\0') {
        return;
    }
    api.log_line(TAG, message);
}

/// The bound IL2CPP surface from the Mapped event, kept for the
/// Il2cppReady callback (ticket 78): metadata-dependent installs wait for
/// `il2cpp_init` to return, exactly like the reference post-init broker.
static IL2CPP: Mutex<Option<Il2cpp>> = Mutex::new(None);

/// Module-loaded callback: on the il2cpp module, bind the guest exports and
/// hold them for the ready stage. Install waits for `Il2cppReady` (ticket
/// 78) — the reference installs after `il2cpp_init` returns, when
/// `il2cpp_class_from_name` and friends are actually usable.
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
                    "il2cpp export bind failed ({} missing) — no hooks installed: {missing:?}",
                    missing.len()
                ),
            );
        }
    }
}

/// Install-event report lines, shared by the ready callback. The line
/// format now comes from the shared gamepatch_support vocabulary (ticket 87
/// Phase B), replacing the two per-Title copies.
fn report_install_events(armed: &ArmedState, events: &[InstallEvent]) {
    for event in events {
        api_log(&armed.api, &event.log_line());
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
        "il2cpp ready: resolving reference patch targets",
    );
    // SAFETY: Il2cppReady runs right after `il2cpp_init` returned, before
    // managed code executes the targets; patch windows keep execute permission.
    let events = unsafe { install(&api, &armed.features) };
    report_install_events(&armed, &events);
    if let Some(runtime) = &armed.title_unload {
        runtime.ready(api);
    }
    if let Some(runtime) = &armed.postprocess { runtime.ready(api); }
}

extern "C" fn on_player_frame(event: Option<&RudroidModuleEvent>, user: *mut c_void) {
    let (Some(event), Some(armed)) = (event, ARMED.get(user)) else { return; };
    if event.kind == ModuleKind::UnityStopping as u32 { launcher::stop(); cinematic::stop(); language::stop(); }
    if event.kind == ModuleKind::UnityFrame as u32 && armed.observe_scenes {
        // SAFETY: UnityFrame is emitted on the owning player thread after
        // nativeRender; observer keeps no managed objects between callbacks.
        unsafe { launcher::observe_scene() };
    }
    if let Some(runtime)=&armed.postprocess {
        match ModuleKind::from_u32(event.kind) {
            Some(ModuleKind::UnityFrame)=>runtime.frame(user as usize),
            Some(ModuleKind::UnityStopping)=>runtime.stop(user as usize),
            _=>{}
        }
    }
    if let Some(runtime)=&armed.title_unload {
        match ModuleKind::from_u32(event.kind) {
            Some(ModuleKind::UnityFrame)=>runtime.frame(user as usize),
            Some(ModuleKind::UnityStopping)=>runtime.stop(user as usize),
            _=>{}
        }
    }
}

/// Plugin init: version check, package gate, feature read, module
/// subscription. Gate mismatch is side-effect free; repeated init re-arms
/// process state and requires the same idle installation window.
///
/// # Safety
/// The same-kit host must pin both modules and provide correctly typed native
/// exports and module events. Ready callbacks run on the Unity thread with
/// every hook target idle through installation and original publication.
pub unsafe fn plugin_init(api: &HostApiV0) -> c_int {
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    let features = SilksongFeatures::from_host_api(api);

    // Package gate (the kit validate step also refuses a mismatched
    // descriptor; this in-process check is the presence-activation
    // equivalent for direct loads).
    let package = read_package_name(api);
    if !package_gate(Some(TARGET_PACKAGE), package.as_deref()) {
        api_log(
            api,
            &format!(
                "gate closed: package '{}' does not match '{TARGET_PACKAGE}' — patch not armed, no subscription",
                package.as_deref().unwrap_or("<none>")
            ),
        );
        return 0;
    }

    // Host log snapshot for the hook bodies.
    *lock(&HOST_LOG) = Some(*api);
    SUPPRESS_QUIT.store(features.suppress_application_quit, Ordering::Relaxed);
    postprocess::arm();

    // Arm: the state stays in the table for the process (like the reference
    // plugin handle); a second init re-arms for the in-process test suite
    // (the loader brings a cdylib up exactly once in production).
    let observe_scenes = features.skip_launcher_disclaimer && api.config_bool(c"unity.log_frame_progress", false);
    let user = ARMED.insert(ArmedState {
        api: *api,
        features: features.clone(),
        observe_scenes,
        title_unload: features.unload_assetbundles_after_title.then(||
            after_title::Runtime::new(*api, features.assetbundle_unload_delay_frames as u32)),
        postprocess: (features.postprocess_pruning_enabled() || features.decorative_sprite_percent<100).then(|| postprocess::Runtime::new(*api,&features)),
    });
    // Ticket 78: two subscriptions — Mapped (bind the exports) and
    // Il2cppReady (metadata usable, run the reference install chain).
    let registered_mapped = api.on_module_loaded(ModuleKind::Il2cpp, on_module_loaded, user);
    let registered_ready = api.on_module_loaded(ModuleKind::Il2cppReady, on_il2cpp_ready, user);
    let registered_frames = (!(features.unload_assetbundles_after_title
        || observe_scenes || features.postprocess_pruning_enabled() || features.decorative_sprite_percent<100)
        || api.on_module_loaded(ModuleKind::UnityFrame, on_player_frame, user))
        && api.on_module_loaded(ModuleKind::UnityStopping, on_player_frame, user);
    if !registered_mapped || !registered_ready || !registered_frames {
        api_log(
            api,
            &format!(
                "module subscription rejected (mapped={registered_mapped}, ready={registered_ready}, frames={registered_frames}) — patch not armed"
            ),
        );
        return -3;
    }
    api_log(
        api,
        &format!(
            "armed for {TARGET_PACKAGE}: waiting for the il2cpp module and il2cpp_init \
             (viewport={}, particles={}, preloader={}, decorative freeze={}, quit suppression={})",
            features.viewport_enable,
            features.disable_ambient_particles,
            features.disable_audio_preloader,
            features.freeze_decorative_behaviours,
            features.suppress_application_quit,
        ),
    );
    0
}

fn read_package_name(api: &HostApiV0) -> Option<String> {
    api.config_string(c"package.packageName", 512)
        .filter(|value| !value.is_empty())
}

/// Register only the title's resource-path service, using the shared VM and
/// Context authority. Package mismatch has no JNI side effects.
#[no_mangle]
pub extern "C" fn rudroid_plugin_register_jni(api: Option<&'static HostApiV0>) -> c_int {
    use rudroid_common::jni_env::JNI_VERSION_1_6;
    let Some(api) = api else { return -1; };
    if api.abi_version != HOST_API_VERSION { return -2; }
    if !package_gate(Some(TARGET_PACKAGE), read_package_name(api).as_deref()) { return 0; }
    let Some(env) = api.java_vm().and_then(|vm| vm.get_env(JNI_VERSION_1_6)) else { return -3; };
    java::register(env)
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

/// Hook-slot counters used by the integration suite to prove install liveness
/// without poking guest memory.
pub fn hook_slots() -> (bool, bool, bool, bool, bool) {
    (
        QUIT_NOARGS_ORIG.address() != 0,
        QUIT_CODE_ORIG.address() != 0,
        PARTICLES.orig_enable.address() != 0,
        AUDIO_PRELOADER_HOOKED.load(Ordering::Relaxed),
        VIEWPORT_ACTIVE.load(Ordering::Relaxed),
    )
}


#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
