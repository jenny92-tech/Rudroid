//! INPUT:  one engine `Spec`, the loader Host API table and the shared JNI session
//! OUTPUT:  `Spec`/`Profile`, `register_jni`, the shared prepare/bootstrap/run player lifecycle and `log`
//! POS:    the single Unity player lifecycle; engine versions differ only by a `Spec` literal and native-phase patches
//!
//! Every Unity engine profile capability (`unity_2021_3`, `unity_6`, …) is a
//! thin cdylib around this library: it declares its descriptor, one [`Spec`]
//! and its own native-phase engine patch, and exports
//! `rudroid_plugin_register_jni` by calling [`register_jni`]. The phases
//! mirror the read-only reference (`plugins/unity_*` + `unity_engine_runner`):
//!
//! 1. [`register_jni`] — validates the spec, then registers the
//!    `RudroidUnityProfile` (prepare/bootstrap/run) with the shared runtime
//!    registry at the reference "early JNI phase". Requires the shared JNI
//!    session (reference: `api->jni_runtime`).
//! 2. `prepare(&HostApiV0)` — engine-side setup: player class lookup,
//!    engine Java services, player callbacks, shared player/context/Surface
//!    objects, config reads (`unity.context_type`, `unity.render_sleep_us`
//!    clamped 0..=1_000_000, `unity.target_fps`,
//!    `unity.signal_vsync_before_render`) and the module subscription that
//!    captures the guest main module's load base.
//! 3. `bootstrap(session, native_library_dir)` — the real
//!    `NativeLoader.load(String)Z` registered by libmain, called through the
//!    shared VM; never reports success before the guest code ran.
//! 4. `run(session)` — initJni in the spec's shape, the reference lifecycle
//!    sequence (recreate gfx → restart indicator if present → surface
//!    changed → resume → focus), then the render loop with input pumping,
//!    elapsed-aware `unity.target_fps` pacing, once-per-second maintenance
//!    and the stop event before `nativeDone`.
//!
//! Engine differences are data in [`Spec`] (no `Default`, so a new version
//! states every one of them); the lifecycle code itself has no per-version
//! branches. Each profile cdylib links its own copy of this library, so the
//! statics here are per profile and never a cross-library service.

mod lifecycle;
pub mod spy;

use std::ffi::{c_int, CStr};

use rudroid_common::descriptor;
pub mod java;
mod network_proxy;
mod network_connectivity;
mod soft_input;

use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_env::JniEnv;
use rudroid_common::registry::{RudroidUnityProfile, HostStr, RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG};

/// The two initJni shapes the reference specs use.
pub const INIT_JNI_CONTEXT: &str = "(Landroid/content/Context;)V";
pub const INIT_JNI_CONTEXT_INT: &str = "(Landroid/content/Context;I)V";

/// Everything one Unity engine version may change about the shared player
/// lifecycle. Deliberately no `Default`: adding a version means stating each
/// field, so a difference can never be inherited by accident.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// Capability id: descriptor id, artifact stem and registered profile id.
    pub id: &'static str,
    /// Host-log tag (the reference plugin's tag).
    pub log_tag: &'static CStr,
    /// Log line after a successful early registration.
    pub registered_message: &'static str,
    /// Reference `unity_engine::Spec` fields (the C++ oracle).
    pub player_class: &'static str,
    pub player_constructor_signature: &'static str,
    pub init_jni_signature: &'static str,
    pub flags: u32,
    pub init_jni_int_arg: i32,
    pub render_sleep_us: u32,
    /// Player callbacks this engine's Java code needs beyond
    /// [`COMMON_PLAYER_CALLBACKS`].
    pub extra_player_callbacks: &'static [PlayerCallback],
    /// Publish the prepared Activity as the static
    /// `UnityPlayer.currentActivity` field.
    pub publishes_current_activity: bool,
    /// Engine Java services registered right after the player class lookup
    /// (non-zero return fails prepare).
    pub register_java_services: Option<fn(env: JniEnv, api: &HostApiV0) -> i32>,
}

impl Spec {
    /// Internal consistency every profile must satisfy before it may
    /// register; [`register_jni`] refuses a spec that fails it.
    pub fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("id", self.id),
            ("player_class", self.player_class),
            ("player_constructor_signature", self.player_constructor_signature),
            ("registered_message", self.registered_message),
        ] {
            if value.is_empty() {
                return Err(format!("{field} is empty"));
            }
        }
        if self.log_tag.is_empty() {
            return Err("log_tag is empty".to_owned());
        }
        let int_shape = match self.init_jni_signature {
            INIT_JNI_CONTEXT => false,
            INIT_JNI_CONTEXT_INT => true,
            other => return Err(format!("initJni signature {other} has no dispatch shape")),
        };
        if int_shape != (self.flags & RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG != 0) {
            return Err(format!(
                "flags {:#x} disagree with the initJni shape {}",
                self.flags, self.init_jni_signature
            ));
        }
        if clamp_render_sleep(i64::from(self.render_sleep_us), 0) != self.render_sleep_us {
            return Err(format!("render_sleep_us {} is outside 0..=1000000", self.render_sleep_us));
        }
        let mut names: Vec<&CStr> = COMMON_PLAYER_CALLBACKS.iter().map(|c| c.name).collect();
        for callback in self.extra_player_callbacks {
            if names.contains(&callback.name) {
                return Err(format!(
                    "player callback {} is registered twice",
                    callback.name.to_string_lossy()
                ));
            }
            names.push(callback.name);
        }
        Ok(())
    }
}

/// A native body the profile registers on the player class for a method the
/// guest engine calls back into. The body fixes its JNI signature, so a
/// name can never be paired with the wrong native prototype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackBody {
    /// `()V`: drain framework-owned Java jobs on the player thread.
    DrainMainThreadJobs,
    /// `()Z`: answer false.
    False,
    /// `(Ljava/lang/String;)Z`: no Java-side library loading (false).
    LibraryUnavailable,
    /// `()V`: no activity indicator to stop.
    NoActivityIndicator,
    /// `(I)V`: no activity indicator to start; the style is ignored.
    IgnoredActivityIndicator,
}

impl CallbackBody {
    pub const fn signature(self) -> &'static CStr {
        match self {
            CallbackBody::DrainMainThreadJobs | CallbackBody::NoActivityIndicator => c"()V",
            CallbackBody::False => c"()Z",
            CallbackBody::LibraryUnavailable => c"(Ljava/lang/String;)Z",
            CallbackBody::IgnoredActivityIndicator => c"(I)V",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlayerCallback {
    pub name: &'static CStr,
    pub body: CallbackBody,
}

/// Player callbacks every supported engine version registers.
pub const COMMON_PLAYER_CALLBACKS: &[PlayerCallback] = &[
    PlayerCallback { name: c"executeMainThreadJobs", body: CallbackBody::DrainMainThreadJobs },
    PlayerCallback { name: c"shouldSetGameState", body: CallbackBody::False },
    PlayerCallback { name: c"isUaaLUseCase", body: CallbackBody::False },
    PlayerCallback { name: c"loadLibrary", body: CallbackBody::LibraryUnavailable },
    PlayerCallback { name: c"stopActivityIndicator", body: CallbackBody::NoActivityIndicator },
    PlayerCallback { name: c"startActivityIndicator", body: CallbackBody::IgnoredActivityIndicator },
];

/// One engine profile: the cdylib's static [`Spec`].
pub trait Profile: 'static {
    const SPEC: &'static Spec;
}

/// Reference `prepare` clamp for `unity.render_sleep_us`: 0..=1_000_000 is
/// taken verbatim, anything else falls back to the spec default.
pub fn clamp_render_sleep(configured: i64, spec: u32) -> u32 {
    if (0..=1_000_000).contains(&configured) {
        configured as u32
    } else {
        spec
    }
}

/// Log through the loader's sink with a profile's tag.
pub fn log(api: &HostApiV0, tag: &CStr, message: &str) {
    api.log_line(tag, message);
}

/// The early JNI phase for profile `P` (`rudroid_plugin_register_jni`).
/// Return codes: -1 null table, -2 ABI mismatch, -3 invalid spec, -4 no
/// shared JNI session, -5 registry refusal.
pub fn register_jni<P: Profile>(api: Option<&'static HostApiV0>) -> c_int {
    let spec = P::SPEC;
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }
    if let Err(reason) = spec.validate() {
        log(api, spec.log_tag, &format!("invalid engine profile spec {}: {reason}", spec.id));
        return -3;
    }
    // Reference parity: registration requires the JNI runtime — here the
    // shared session's `JavaVM*` (ticket 52) — and the registration surface.
    if api.java_vm().is_none() {
        log(
            api,
            spec.log_tag,
            "no JNI session registered (deploy android_jni_vm); the engine \
             profile cannot register",
        );
        return -4;
    }
    // The strings are process-lifetime leaks; the host copies the profile.
    let leak = |text: &str| HostStr::new(descriptor::leak_cstr(text));
    let profile = RudroidUnityProfile {
        id: leak(spec.id),
        player_class: leak(spec.player_class),
        player_constructor_signature: leak(spec.player_constructor_signature),
        init_jni_signature: leak(spec.init_jni_signature),
        flags: spec.flags,
        init_jni_int_arg: spec.init_jni_int_arg,
        prepare: Some(lifecycle::prepare::<P>),
        bootstrap: Some(lifecycle::bootstrap),
        run: Some(lifecycle::run),
    };
    if !api.register_profile(&profile) {
        log(api, spec.log_tag, "failed to register early Unity engine profile");
        return -5;
    }
    log(api, spec.log_tag, spec.registered_message);
    0
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
