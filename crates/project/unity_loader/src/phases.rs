//! Ordered capability phases (ticket 31).
//!
//! Reference bring-up order, subset for the current kit surface:
//! 1. `rudroid_plugin_early_init` — logging/infrastructure first,
//! 2. `rudroid_plugin_register_jni` — JNI registration in load order,
//! 3. `rudroid_plugin_init` — main init.
//!
//! Every capability runs a whole phase before the next phase starts, exactly
//! like the reference plugin host. Within the early phase the logging
//! capability runs first, then the rest in validated order (reference
//! `load_jni`); the later phases use the validated order unchanged.
//!
//! Known divergences from the reference, kept until the capabilities and the
//! Host API catch up (review 2026-09-22 finding 2): every phase export is
//! optional here, whereas the reference fails a logging plugin without
//! `rudroid_plugin_early_init` and any plugin without
//! `rudroid_plugin_init`; and Host API v0 has no log-sink registration,
//! so the reference "logging plugin registered a sink" check has nothing to
//! observe yet. Capability handles stay pinned for the
//! process lifetime (registrations and detours cannot be rolled back), so a
//! failed phase aborts startup — never a partially initialized kit.

use rudroid_common::{symbols, Kind};

use super::host_api;
use super::LoadedCapability;

/// The ordered capability phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    EarlyInit,
    RegisterJni,
    Init,
}

impl Phase {
    /// Entry-point symbol for this phase.
    pub fn symbol(self) -> &'static str {
        match self {
            Phase::EarlyInit => symbols::EARLY_INIT,
            Phase::RegisterJni => symbols::REGISTER_JNI,
            Phase::Init => symbols::INIT,
        }
    }
}

/// Run `phase` across all capabilities in the (already validated) order,
/// the logging capability first in the early phase. Absent exports are
/// legal — phases are optional per capability (see the module divergences).
pub fn run_phase(phase: Phase, capabilities: &[LoadedCapability]) -> Result<(), super::LoaderError> {
    let logging_first = phase == Phase::EarlyInit;
    let ordered = capabilities
        .iter()
        .filter(|c| logging_first && c.descriptor.kind == Kind::Logging)
        .chain(
            capabilities
                .iter()
                .filter(|c| !(logging_first && c.descriptor.kind == Kind::Logging)),
        );
    for capability in ordered {
        let outcome = unsafe { host_api::run_phase_entry(phase, capability.library()) };
        match outcome {
            Ok(()) => {}
            Err(host_api::InitError::NotPresent) => {}
            // run_phase_entry loses the capability id (it only sees the
            // library handle); re-attach it so failures are actionable.
            Err(host_api::InitError::Failed { id: _, code }) => {
                return Err(super::LoaderError::PhaseFailed {
                    phase,
                    id: capability.descriptor.id.clone(),
                    code,
                })
            }
            Err(host_api::InitError::Symbol { reason }) => {
                return Err(super::LoaderError::PhaseOther {
                    phase,
                    id: capability.descriptor.id.clone(),
                    reason,
                })
            }
        }
    }
    Ok(())
}

/// All three phases in reference order.
pub fn run_all(capabilities: &[LoadedCapability]) -> Result<(), super::LoaderError> {
    run_phase(Phase::EarlyInit, capabilities)?;
    run_phase(Phase::RegisterJni, capabilities)?;
    run_phase(Phase::Init, capabilities)
}
