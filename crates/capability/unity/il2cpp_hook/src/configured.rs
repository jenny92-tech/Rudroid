//! INPUT:  validated legacy patch requests, pinned Host API and initialized IL2CPP module
//! OUTPUT:  resolved/installed configuration patches or precise rejection diagnostics
//! POS:    connects config to common's checked native boundary without raw object offsets
use super::{PatchMode, PatchRequest};
use rudroid_common::{
    host_api::HostApiV0,
    il2cpp::Il2cpp,
    managed_hooks::{self, ManagedHookPlan, PatchSpec},
};
use std::{
    ffi::CString,
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
};
static REQUESTS: OnceLock<Vec<PatchRequest>> = OnceLock::new();
static LAYOUT: OnceLock<Option<super::layout_config::LayoutRequest>> = OnceLock::new();
static TOUCH: OnceLock<bool> = OnceLock::new();
static BASE: AtomicUsize = AtomicUsize::new(0);
pub(super) fn configure(requests: Vec<PatchRequest>, host: &HostApiV0) {
    let _ = REQUESTS.set(requests);
    let _ = TOUCH.set(
        host.config_string(c"package.packageName", 512)
            .is_some_and(|name| name == "com.game.silksong"),
    );
    match super::layout_config::LayoutRequest::read(host) {
        Ok(request) => {
            let _ = LAYOUT.set(request);
        }
        Err(reason) => {
            host.log_line(c"UI", &format!("HUD config rejected: {reason}"));
            let _ = LAYOUT.set(None);
        }
    }
}
pub(super) extern "C" fn stopping(
    _: Option<&rudroid_common::registry::RudroidModuleEvent>,
    _: *mut std::ffi::c_void,
) {
    managed_hooks::stop();
}
pub(super) fn mapped(base: usize) {
    BASE.store(base, Ordering::Release);
}

pub(super) fn apply(host: &HostApiV0) {
    let Some(requests) = REQUESTS.get() else {
        return;
    };
    if requests.is_empty()
        && LAYOUT.get().and_then(Option::as_ref).is_none()
        && TOUCH.get() != Some(&true)
    {
        return;
    }
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    // SAFETY: module event identifies trusted process-pinned IL2CPP code and standard exports.
    let api = match unsafe { Il2cpp::bind(base, host.so_symbol, &[]) } {
        Ok(api) => api,
        Err(missing) => {
            host.log_line(c"IL2PATCH", &format!("config binding failed: {missing:?}"));
            return;
        }
    };
    for request in requests {
        let result: Result<usize, String> = (|| {
            let strings = [
                &request.assembly,
                &request.namespace,
                &request.class,
                &request.method,
            ]
            .map(|s| CString::new(s.as_bytes()));
            let [assembly, namespace, class, method] = strings;
            let (assembly, namespace, class, method) = (
                assembly.map_err(|_| "assembly contains NUL")?,
                namespace.map_err(|_| "namespace contains NUL")?,
                class.map_err(|_| "class contains NUL")?,
                method.map_err(|_| "method contains NUL")?,
            );
            let spec = PatchSpec {
                assembly: &assembly,
                namespace: &namespace,
                class: &class,
                method: &method,
                argc: request.argument_count as u32,
                object_register: if request.object_argument < 0 {
                    0
                } else {
                    request.object_argument as usize + 1
                },
                field_offset: usize::try_from(request.field).ok(),
                source_offset: usize::try_from(request.source_field).ok(),
                multiplier: request.multiplier,
                value: request.value,
                integer: request.integer_field,
                mode: match request.mode {
                    PatchMode::Scale => managed_hooks::PatchMode::Scale,
                    PatchMode::Freeze => managed_hooks::PatchMode::Freeze,
                    PatchMode::ForceReturn => managed_hooks::PatchMode::ForceReturn,
                    PatchMode::Trace => managed_hooks::PatchMode::Trace,
                    PatchMode::Bypass => managed_hooks::PatchMode::Bypass,
                },
            };
            let plan = ManagedHookPlan::patch(api, &spec)?;
            // SAFETY: called just after il2cpp_init, before game execution, with its target entries idle.
            // Common validates complete register ABI and pins metadata/host/trampolines for the process.
            unsafe { managed_hooks::install(plan, *host) }
        })();
        match result {
            Ok(slot) => host.log_line(
                c"IL2PATCH",
                &format!("configured patch {} installed slot={slot}", request.id),
            ),
            Err(reason) => host.log_line(
                c"IL2PATCH",
                &format!("configured patch {} rejected: {reason}", request.id),
            ),
        }
    }
    if let Some(layout) = LAYOUT.get().and_then(Option::as_ref) {
        let result = ManagedHookPlan::hud(api, &layout.spec())
            .map_err(str::to_owned)
            .and_then(|plan| {
                // SAFETY: same post-init idle installation window and common-verified complete native signature.
                unsafe { managed_hooks::install(plan, *host) }
            });
        match result {
            Ok(slot) => host.log_line(c"UI", &format!("HUD layout installed slot={slot}")),
            Err(reason) => host.log_line(c"UI", &format!("HUD layout rejected: {reason}")),
        }
    }
    if TOUCH.get() == Some(&true) {
        let result = ManagedHookPlan::touch_controls(api)
            .map_err(str::to_owned)
            .and_then(|plan| {
                // SAFETY: same post-init idle window; common validates exact setter, array and bool field metadata.
                unsafe { managed_hooks::install(plan, *host) }
            });
        match result {
            Ok(slot) => host.log_line(
                c"UI",
                &format!("TouchManager adapter installed slot={slot}"),
            ),
            Err(reason) => host.log_line(
                c"UI",
                &format!("TouchManager adapter unavailable: {reason}"),
            ),
        }
    }
}
