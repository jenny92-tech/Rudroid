//! INPUT:  immutable HostConfig paths, startup policy and common's safe asset resolver
//! OUTPUT:  bounded shared resource replies, optional owned boot view and pack publication
//! POS:    one loader-owned resource authority, published atomically with configuration
use rudroid_common::android_assets::{AssetError, AssetRequest, AssetResolver};
use rudroid_host_config::{HostConfig, UnityRenderThread, UnityStartupPolicy};
use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

pub(crate) struct Service {
    base: Arc<AssetResolver>,
    packs: OnceLock<Arc<AssetResolver>>,
    boot: Option<crate::boot_view::BootView>,
    cpuinfo: Option<crate::cpuinfo_view::CpuInfoView>,
}

impl Service {
    pub(crate) fn new(config: Option<&HostConfig>, startup: &UnityStartupPolicy) -> Result<Self, &'static str> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let root = config
            .and_then(|c| c.paths().game_files)
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.clone());
        let root = if root.is_absolute() {
            root
        } else {
            cwd.join(root)
        };
        let cache = config
            .and_then(|c| c.get_string("paths.unity_shader_cache_redirect", ""))
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let block = config.map_or(true, |c| c.get_bool("unity.block_analytics", true));
        if startup.render_thread() != UnityRenderThread::Engine {
            let native = [root.join("lib/arm64-v8a/libunity.so"),root.join("gamedata/lib/arm64-v8a/libunity.so")]
                .into_iter().find(|p| p.is_file()).ok_or("render-thread override requires a verified native Unity player")?;
            let file=std::fs::File::open(native).map_err(|_| "cannot open native Unity player for startup preflight")?;
            let identity=rudroid_common::elf_identity::aarch64_identity(&file).map_err(|_| "cannot validate native Unity player identity")?;
            if !startup.supports_native_player(&identity) {
                return Err("render-thread override is not verified for this native Unity build; retain render_thread_mode=engine");
            }
        }
        let base = Arc::new(AssetResolver::new(root, cache, block));
        let boot = if startup.render_thread() == UnityRenderThread::Engine { None } else {
            let request = AssetRequest::new(rudroid_common::android_assets::BASE, c"bin/Data/boot.config", None, 0);
            let encoded = base.query(&request).map_err(|_| "cannot resolve extracted Unity boot.config")?.ok_or("render-thread policy requires extracted Unity boot.config")?;
            let path = CStr::from_bytes_with_nul(&encoded).map_err(|_| "invalid resolved Unity boot.config path")?;
            Some(crate::boot_view::BootView::prepare(std::path::Path::new(OsStr::from_bytes(path.to_bytes())), startup)?)
        };
        let cpuinfo = if config.is_some_and(|c| c.get_bool("runtime.cpuinfo_snapshot", false)) {
            match crate::cpuinfo_view::CpuInfoView::prepare() {
                Ok(view) => { super::host_api::loader_log("CPUINFO", "prepared bounded sealed process snapshot; guest topology remains fixed for this run"); Some(view) }
                Err(reason) => { super::host_api::loader_log("CPUINFO", &format!("snapshot unavailable ({reason}); retaining native procfs reads")); None }
            }
        } else { None };
        Ok(Service {
            base,
            packs: OnceLock::new(),
            boot,
            cpuinfo,
        })
    }

    fn query(&self, request: &AssetRequest<'_>) -> Result<Option<Vec<u8>>, AssetError> {
        if request.kind == rudroid_common::android_assets::PATH {
            if let Some(reply) = self.cpuinfo.as_ref().and_then(|view| request.path.to_cstr().and_then(|path| view.redirect(path))) {
                return Ok(Some(reply));
            }
        }
        let resolver = self.packs.get().unwrap_or(&self.base);
        let reply = resolver.query(request)?;
        if let Some(boot) = &self.boot {
            use rudroid_common::android_assets::{PATH, BASE, MERGED, PACK};
            if matches!(request.kind, PATH | BASE | MERGED | PACK) {
                let candidate = reply.as_deref().and_then(|b| CStr::from_bytes_with_nul(b).ok())
                    .or_else(|| (request.kind == PATH).then(|| request.path.to_cstr()).flatten());
                if let Some(projected) = candidate.and_then(|path| boot.redirect(path)) { return Ok(Some(projected)); }
            }
        }
        Ok(reply)
    }
}

pub(crate) extern "C" fn enable_packs() -> i32 {
    let Some(service) = super::host_api::resource_service() else {
        return -1;
    };
    if service.packs.get().is_some() {
        return -1;
    }
    match service.base.with_packs() {
        Ok(resolver) => {
            let count = resolver.pack_count();
            if service.packs.set(Arc::new(resolver)).is_err() {
                return -1;
            }
            super::host_api::loader_log(
                "Assets",
                &format!("extracted pack overlay ready: installed={count}"),
            );
            0
        }
        Err(error) => {
            super::host_api::loader_log("Assets", &format!("pack scan failed: {error}"));
            -1
        }
    }
}

/// # Safety
/// A non-null output is writable for capacity bytes; request strings are valid
/// for this call. No output/request is retained, and only bounded copies occur.
pub(crate) unsafe extern "C" fn query(
    request: Option<&AssetRequest<'_>>,
    output: *mut u8,
    capacity: u32,
) -> i32 {
    let Some(request) = request else {
        return -2;
    };
    if output.is_null() && capacity != 0 {
        return -2;
    }
    let Some(service) = super::host_api::resource_service() else {
        return 0;
    };
    match service.query(request) {
        Ok(Some(bytes)) => {
            let size = bytes.len();
            if size > rudroid_common::android_assets::MAX_REPLY {
                return -3;
            }
            if capacity != 0 {
                // SAFETY: the caller owns capacity bytes; the immutable reply is disjoint and live for this copy.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        output,
                        size.min(capacity as usize),
                    );
                }
            }
            size as i32
        }
        Ok(None) => 0,
        Err(AssetError::Missing) => -1,
        Err(AssetError::Invalid) => -2,
        Err(AssetError::TooLarge | AssetError::Changed) => -3,
    }
}
