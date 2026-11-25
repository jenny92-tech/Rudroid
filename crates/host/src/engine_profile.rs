//! INPUT: registered engine profile and Android hooks, deployed capability descriptors
//! OUTPUT: serialized prepare/bootstrap/run transitions and a terminal outcome
//! POS: loader-owned engine profile lifecycle
#![forbid(unsafe_code)]
use super::{registered_profile, registered_android_hooks};
use crate::{LoadedCapability, LoaderError};
use rudroid_common::registry::{HostStr, RudroidEngineProfile};
use std::{ffi::{c_void, CString}, sync::Mutex};

pub(super) fn clear() {
    let mut profile = PROFILE_SESSION.lock().unwrap();
    profile.generation = profile.generation.wrapping_add(1);
    profile.session = None;
}

/// The prepared profile session (opaque pointer owned by the profile
/// capability): `Some` between a successful prepare and the run hand-off.
/// Cleared by [`super::shutdown`] so a failed init never leaves a live session
/// behind.
struct ProfileState {
    session: Option<usize>,
    generation: u64,
    busy: bool,
}
static PROFILE_SESSION: Mutex<ProfileState> = Mutex::new(ProfileState {
    session: None, generation: 0, busy: false,
});

// A callback may reenter other loader services. Reserve its lifecycle stage
// under the mutex and release the mutex before any external call.
struct ProfileStage(u64);
impl ProfileStage {
    fn enter(capabilities: &[LoadedCapability], stage: &'static str) -> Result<Self, LoaderError> {
        let mut state = PROFILE_SESSION.lock().unwrap();
        if state.busy {
            return Err(LoaderError::ProfileLifecycle {
                id: capabilities.iter().find(|c| c.descriptor.kind == rudroid_common::Kind::EngineProfile)
                    .map(|c| c.descriptor.id.clone()).unwrap_or_default(),
                stage,
                reason: "another profile lifecycle callback is in progress".into(),
            });
        }
        state.busy = true;
        Ok(Self(state.generation))
    }
}
impl Drop for ProfileStage {
    fn drop(&mut self) {
        PROFILE_SESSION.lock().unwrap().busy = false;
    }
}

/// Cross-check one registered profile against the deployed capability:
/// identity plus prepare/bootstrap/run are required; validate_config and
/// build_guest_plan stay optional at the loader gate.
fn validate_registered_profile(
    profile: &RudroidEngineProfile,
    capability_id: &str,
) -> Result<(), LoaderError> {
    let read = |text: HostStr<'static>| -> Option<String> {
        let text = text.to_cstr()?.to_string_lossy().into_owned();
        (!text.is_empty()).then_some(text)
    };
    let id = read(profile.id);
    if id.as_deref() != Some(capability_id) {
        return Err(LoaderError::ProfileLifecycle {
            id: capability_id.to_owned(),
            stage: "register",
            reason: format!(
                "the registered profile id {id:?} does not match the deployed \
                 capability id {capability_id:?}; the kit is inconsistent"
            ),
        });
    }
    if profile.prepare.is_none() || profile.bootstrap.is_none() || profile.run.is_none() {
        let missing = [
            ("prepare", profile.prepare.is_none()),
            ("bootstrap", profile.bootstrap.is_none()),
            ("run", profile.run.is_none()),
        ]
        .into_iter()
        .filter(|(_, absent)| *absent)
        .map(|(field, _)| field)
        .collect::<Vec<_>>()
        .join(", ");
        return Err(LoaderError::ProfileLifecycle {
            id: capability_id.to_owned(),
            stage: "register",
            reason: format!("the registered profile misses its `{missing}` callback"),
        });
    }
    Ok(())
}

/// The deployed engine profile capability plus its registered callbacks and
/// the prepared session (bootstrap/run precondition).
fn prepared_profile(
    capabilities: &[LoadedCapability],
    stage: &'static str,
) -> Result<(String, RudroidEngineProfile, *mut c_void), LoaderError> {
    let capability = capabilities
        .iter()
        .find(|c| c.descriptor.kind == rudroid_common::Kind::EngineProfile)
        .ok_or(LoaderError::NoEngineProfile)?;
    let id = capability.descriptor.id.clone();
    let profile = registered_profile().ok_or_else(|| LoaderError::ProfileLifecycle {
        id: id.clone(),
        stage: "register",
        reason: "the profile capability did not register its RudroidEngineProfile \
                 during the register-JNI phase; redeploy a consistent kit"
            .to_owned(),
    })?;
    validate_registered_profile(&profile, &id)?;
    let session = PROFILE_SESSION.lock().unwrap().session
        .ok_or_else(|| LoaderError::ProfileLifecycle {
            id: id.clone(),
            stage,
            reason: "called before a successful prepare; check bring-up order"
                .to_owned(),
        })?;
    Ok((id, profile, session as *mut c_void))
}

/// Host-side half of the ticket-53 prepare path: the
/// deployed profile must have registered exactly one engine profile during
/// the register-JNI phase, the Android runtime hooks must already be
/// registered (reference: "android_base plugin is missing or failed"), and
/// the profile's own `prepare` must return a session. Runs BEFORE the guest
/// modules are mapped (reference order).
pub fn prepare_engine_runtime(
    capabilities: &[LoadedCapability],
) -> Result<(), LoaderError> {
    let stage = ProfileStage::enter(capabilities, "prepare")?;
    {
        let session = PROFILE_SESSION.lock().unwrap();
        if session.session.is_some() {
            let id = capabilities
                .iter()
                .find(|c| c.descriptor.kind == rudroid_common::Kind::EngineProfile)
                .map(|c| c.descriptor.id.clone())
                .unwrap_or_default();
            return Err(LoaderError::ProfileLifecycle {
                id,
                stage: "prepare",
                reason: "the engine session is already prepared".to_owned(),
            });
        }
    }
    let capability = capabilities
        .iter()
        .find(|c| c.descriptor.kind == rudroid_common::Kind::EngineProfile)
        .ok_or(LoaderError::NoEngineProfile)?;
    let id = capability.descriptor.id.clone();
    let profile = registered_profile().ok_or_else(|| LoaderError::ProfileLifecycle {
        id: id.clone(),
        stage: "register",
        reason: "the profile capability did not register its RudroidEngineProfile \
                 during the register-JNI phase; redeploy a consistent kit"
            .to_owned(),
    })?;
    validate_registered_profile(&profile, &id)?;
    // Reference gate: prepare proceeds only with both runtime hooks
    // registered (the android_base role).
    let hooks = registered_android_hooks();
    let hooks_ok = matches!(&hooks, Some(hooks) if hooks.flush_preferences.is_some() && hooks.signal_vsync.is_some());
    if !hooks_ok {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "prepare",
            reason: "the android_base plugin is missing or failed: flush_preferences \
                     and signal_vsync must be registered before the engine prepares"
                .to_owned(),
        });
    }
    let Some(prepare) = profile.prepare else {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "register",
            reason: "the registered profile misses its `prepare` callback".to_owned(),
        });
    };
    let session = prepare(Some(crate::host_api::table()));
    if session.is_null() {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "prepare",
            reason: "the profile's prepare returned no session (see profile diagnostics above)"
                .to_owned(),
        });
    }
    {
        let mut state = PROFILE_SESSION.lock().unwrap();
        if state.generation != stage.0 {
            return Err(LoaderError::ProfileLifecycle {
                id, stage: "prepare", reason: "loader shut down during prepare".into(),
            });
        }
        state.session = Some(session as usize);
    }
    crate::host_api::loader_log("PROFILE", &format!("engine profile prepared: {id}"));
    Ok(())
}

/// Host-side half of the ticket-53 bootstrap path:
/// hand the prepared session and the native library dir to the profile's
/// registered bootstrap (the `NativeLoader.load` analogue). Non-zero means
/// the guest refused — the run fails with the reason, never a fake success.
pub fn bootstrap_engine_runtime(
    capabilities: &[LoadedCapability],
    native_library_dir: &str,
) -> Result<(), LoaderError> {
    let _stage = ProfileStage::enter(capabilities, "bootstrap")?;
    let (id, profile, session) = prepared_profile(capabilities, "bootstrap")?;
    if native_library_dir.is_empty() {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "bootstrap",
            reason: "the native library dir is empty".to_owned(),
        });
    }
    let c_dir = CString::new(native_library_dir.replace('\0', "\\0"))
        .expect("NUL bytes are replaced above");
    let Some(bootstrap) = profile.bootstrap else {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "register",
            reason: "the registered profile misses its `bootstrap` callback".to_owned(),
        });
    };
    let rc = bootstrap(session, rudroid_common::registry::HostStr::new(&c_dir));
    if rc != 0 {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "bootstrap",
            reason: format!(
                "the profile bootstrap returned {rc} (see profile diagnostics above)"
            ),
        });
    }
    crate::host_api::loader_log(
        "PROFILE",
        &format!("engine profile bootstrapped: {id} native_library_dir={native_library_dir}"),
    );
    Ok(())
}

/// Full teardown, or an explicit handoff to immediate process termination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "ProcessExit requires immediate process termination with the kit still pinned"]
pub enum EngineRunCompletion {
    Stopped,
    ProcessExit,
}

/// Run the engine profile, then flush applied Android preferences. Consume
/// ProcessExit immediately through the CLI terminal boundary with every
/// library/service owner still pinned; Stopped completed full teardown.
pub fn run_engine_runtime(
    capabilities: &[LoadedCapability],
) -> Result<EngineRunCompletion, LoaderError> {
    let _stage = ProfileStage::enter(capabilities, "run")?;
    let (id, profile, session) = prepared_profile(capabilities, "run")?;
    let Some(run) = profile.run else {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "register",
            reason: "the registered profile misses its `run` callback".to_owned(),
        });
    };
    PROFILE_SESSION.lock().unwrap().session.take();
    let rc = run(session);
    if let Some(flush) = registered_android_hooks().and_then(|hooks| hooks.flush_preferences) {
        flush();
    }
    if rc == rudroid_common::registry::RUDROID_ENGINE_RUN_PROCESS_EXIT {
        crate::host_api::loader_log("PROFILE", "engine stop ready for immediate process exit (saved preferences)");
        return Ok(EngineRunCompletion::ProcessExit);
    }
    if rc != 0 {
        return Err(LoaderError::ProfileLifecycle {
            id,
            stage: "run",
            reason: format!(
                "the profile run returned {rc} (see profile diagnostics above)"
            ),
        });
    }
    crate::host_api::loader_log(
        "PROFILE",
        &format!("engine profile run complete: {id} (player stopped)"),
    );
    Ok(EngineRunCompletion::Stopped)
}


