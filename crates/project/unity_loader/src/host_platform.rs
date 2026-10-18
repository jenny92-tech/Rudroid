//! INPUT:  selected display backend, measured drawable and HostConfig render policy
//! OUTPUT:  bring_up_host_session, verified HostSessionReport and an owned activation transaction
//! POS:    keeps display startup/validation independent of SDL, Unity versions and title plugins
#![forbid(unsafe_code)]
use crate::{LoadedCapability, LoaderError};
use rudroid_common::display_dimensions::RenderDimensions;
use rudroid_host_config::{HostConfig, GraphicsApi};

/// Verified host session facts as observed through the selected display backend.
#[derive(Debug, Clone)]
pub struct HostSessionReport {
    pub graphics_api: GraphicsApi,
    pub window_w: u32,
    pub window_h: u32,
    pub video_driver: String,
    pub major: u32,
    pub minor: u32,
    pub profile_mask: u32,
    pub swap_count: u32,
    pub readback_non_background: bool,
    pub gl_vendor: String,
    pub gl_renderer: String,
    pub gl_version: String,
}

/// Internal adapters keep their native handles private. Dropping an unpinned
/// session releases failed initialization; pin retains a successful session
/// until process exit. Neither interface is used in the per-frame hot path.
pub(crate) trait DisplaySession {
    fn report(&self) -> Result<HostSessionReport, String>;
    fn activate(&mut self, geometry: RenderDimensions) -> Result<(), String>;
    fn pin(self);
}
pub(crate) trait DisplayBackend {
    type Session: DisplaySession;
    fn id(&self) -> &str;
    fn create(&self, config: &HostConfig) -> Result<Self::Session, String>;
}

/// Select a deployed backend before any guest module executes. SDL is the
/// implemented backend today; a different backend supplies its own adapter
/// without changing the registry, Unity boot chain, or title capabilities.
pub fn bring_up_host_session(
    capabilities: &[LoadedCapability],
    config: &HostConfig,
) -> Result<Option<HostSessionReport>, LoaderError> {
    let Some(backend) = crate::platform_sdl::Backend::find(capabilities) else {
        eprintln!("[PLATFORM] no supported display backend deployed; host display bring-up skipped explicitly (no window will exist)");
        crate::runtime::log_audio_policy_status();
        return Ok(None);
    };
    start_backend(
        &backend,
        config,
        crate::host_api::install_display_dimensions,
    )
    .map(Some)
}

fn start_backend<B: DisplayBackend>(
    backend: &B,
    config: &HostConfig,
    publish: impl FnOnce(RenderDimensions) -> Result<(), &'static str>,
) -> Result<HostSessionReport, LoaderError> {
    let failure = |reason: String| LoaderError::HostSession {
        id: backend.id().to_owned(),
        reason,
    };
    let mut session = backend.create(config).map_err(failure)?;
    let report = session.report().map_err(failure)?;
    if report.graphics_api != config.graphics_api().map_err(|e|failure(e.into()))? {
        return Err(failure("display backend did not grant the requested graphics API".into()));
    }
    if report.graphics_api == GraphicsApi::Gles && report.profile_mask != 0x0004 {
        return Err(failure(format!(
            "granted profile mask {:#x} is not ES",
            report.profile_mask
        )));
    }
    if report.graphics_api == GraphicsApi::Gles && report.swap_count == 0 {
        return Err(failure("verification frame never swapped".into()));
    }
    if report.graphics_api == GraphicsApi::Gles && !report.readback_non_background {
        return Err(failure(
            "verification frame drew no non-background pixel".into(),
        ));
    }
    if report.graphics_api == GraphicsApi::Vulkan && (report.profile_mask != 0 || report.major < 1
        || (report.major == 1 && report.minor < 1)
        || report.swap_count != 0 || report.readback_non_background) {
        return Err(failure("invalid Vulkan preflight contract; no guest frame has been presented yet".into()));
    }
    eprintln!(
        "[PLATFORM] session: backend={} driver={} window={}x{} {} {}.{} profile=0x{:x} swaps={}",
        backend.id(),
        report.video_driver,
        report.window_w,
        report.window_h,
        match report.graphics_api { GraphicsApi::Gles => "GLES", GraphicsApi::Vulkan => "Vulkan" },
        report.major,
        report.minor,
        report.profile_mask,
        report.swap_count
    );
    eprintln!(
        "[PLATFORM] {}: version={:?} renderer={:?} vendor={:?}",
        match report.graphics_api { GraphicsApi::Gles => "gl", GraphicsApi::Vulkan => "vk" },
        report.gl_version, report.gl_renderer, report.gl_vendor
    );
    crate::runtime::log_audio_policy_status();
    let geometry = config
        .render_dimensions_for_output(i64::from(report.window_w), i64::from(report.window_h))
        .map_err(|reason| failure(reason.into()))?;
    publish(geometry).map_err(|reason| failure(reason.into()))?;
    session.activate(geometry).map_err(failure)?;
    session.pin();
    Ok(report)
}

#[cfg(test)]
#[path = "../tests/unit/host_platform.rs"]
mod tests;
