//! INPUT:  immutable Host API GLES version overrides
//! OUTPUT:  process-pinned version strings and scalar version query policy
//! POS:    safe identity policy; guest_graphics owns the native query boundary
#![forbid(unsafe_code)]
use rudroid_common::host_api::HostApiV0;
use std::{
    ffi::{CStr, CString},
    sync::OnceLock,
};

#[derive(Clone, Default, PartialEq, Eq)]
struct Identity {
    version: Option<CString>,
    major: Option<i32>,
    minor: Option<i32>,
}
static IDENTITY: OnceLock<Identity> = OnceLock::new();
impl Identity {
    fn new(version: Vec<u8>, major: i64, minor: i64) -> Result<Self, String> {
        let scalar = |value: i64| -> Result<Option<i32>, String> {
            if (-1..=9).contains(&value) {
                Ok((value >= 0).then_some(value as i32))
            } else {
                Err("GLES identity version override must be between -1 and 9".into())
            }
        };
        let version = if version.is_empty() {
            None
        } else {
            Some(CString::new(version).map_err(|_| "GLES version override contains NUL")?)
        };
        Ok(Self {
            version,
            major: scalar(major)?,
            minor: scalar(minor)?,
        })
    }
    fn integer(&self, name: u32) -> Option<i32> {
        match name {
            0x821b => self.major,
            0x821c => self.minor,
            _ => None,
        }
    }
}
pub(super) fn configure(api: &HostApiV0) -> Result<(), String> {
    let identity = Identity::new(
        api.config_string(c"gpu.glVersionOverride", 512)
            .unwrap_or_default().into_bytes(),
        api.config_i64(c"gpu.glMajorVersionOverride", -1),
        api.config_i64(c"gpu.glMinorVersionOverride", -1),
    )?;
    let installed = IDENTITY.get_or_init(|| identity.clone());
    if installed != &identity {
        return Err("GLES identity is already configured differently".into());
    }
    if installed.version.is_some() {
        eprintln!("[SDL] guest GLES version identity override enabled");
    }
    Ok(())
}
pub(super) fn version() -> Option<&'static CStr> {
    IDENTITY.get()?.version.as_deref()
}
pub(super) fn integer(name: u32) -> Option<i32> {
    IDENTITY.get()?.integer(name)
}

#[cfg(test)]
#[path = "../tests/unit/gles_identity.rs"]
mod tests;
