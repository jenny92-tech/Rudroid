//! INPUT:  validated extracted boot.config path and pure UnityStartupPolicy
//! OUTPUT:  process-owned sealed boot-file view and exact-path redirection
//! POS:    resource ownership boundary; preserves original games and adds no native patch or frame task
#![forbid(unsafe_code)]

use crate::{UnityStartupPolicy, MAX_BOOT_CONFIG_BYTES};
use std::{
    ffi::{CStr, CString, OsStr},
    fs::File,
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::MetadataExt,
        },
    },
    path::{Path, PathBuf},
};

pub struct BootView {
    source: PathBuf,
    source_c: CString,
    path: CString,
    // The proc-fd path is borrowed only while this owner stays published.
    // Each open gets an independent file cursor; OS exit closes the last owner.
    _file: File,
}

impl BootView {
    pub fn prepare(
        source: &Path,
        policy: &UnityStartupPolicy,
    ) -> Result<Self, &'static str> {
        let source = source
            .canonicalize()
            .map_err(|_| "cannot resolve Unity boot.config")?;
        let mut input = File::open(&source).map_err(|_| "cannot open Unity boot.config")?;
        let before = input
            .metadata()
            .map_err(|_| "cannot inspect Unity boot.config")?;
        if !before.is_file() || before.len() > MAX_BOOT_CONFIG_BYTES as u64 {
            return Err("Unity boot.config must be a regular file of at most 64 KiB");
        }
        let mut text = String::new();
        (&mut input)
            .take(MAX_BOOT_CONFIG_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|_| "cannot read UTF-8 Unity boot.config")?;
        let after = input
            .metadata()
            .map_err(|_| "cannot recheck Unity boot.config")?;
        if before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || text.len() as u64 != after.len()
        {
            return Err("Unity boot.config changed during startup");
        }
        let data = policy
            .boot_config(&text)?
            .ok_or("engine-default startup does not need a boot view")?;
        let file = rudroid_common::filesystem::sealed_snapshot(data.as_bytes())
            .map_err(|_| "cannot create sealed Unity boot view")?;
        let path = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
        let source_c = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| "Unity boot.config path contains NUL")?;
        Ok(Self {
            source,
            source_c,
            path,
            _file: file,
        })
    }

    pub fn source_path(&self) -> &Path {
        &self.source
    }

    pub fn source_cstr(&self) -> &CStr {
        &self.source_c
    }

    pub fn replacement_cstr(&self) -> &CStr {
        &self.path
    }

    /// Only the selected file is projected; unrelated native files and aliases
    /// retain the resource resolver's ordinary behavior.
    pub fn redirect(&self, candidate: &CStr) -> Option<Vec<u8>> {
        let path = Path::new(OsStr::from_bytes(candidate.to_bytes()));
        if !path.ends_with("bin/Data/boot.config") || path.canonicalize().ok()? != self.source {
            return None;
        }
        Some(self.path.as_bytes_with_nul().to_vec())
    }
}

#[cfg(test)]
#[path = "../tests/unit/boot_view.rs"]
mod tests;
