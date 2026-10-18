//! INPUT:  validated extracted boot.config path and pure UnityStartupPolicy
//! OUTPUT:  process-owned unlinked boot-file view and exact-path redirection
//! POS:    resource ownership boundary; preserves original games and adds no native patch or frame task
#![forbid(unsafe_code)]

use rudroid_host_config::{UnityStartupPolicy, MAX_BOOT_CONFIG_BYTES};
use std::{
    ffi::{CStr, CString, OsStr},
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(1);
struct Temporary {
    path: PathBuf,
    linked: bool,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if self.linked {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(crate) struct BootView {
    source: PathBuf,
    path: CString,
    // The proc-fd path is borrowed only while this owner stays published.
    // Each open gets an independent file cursor; OS exit closes the last owner.
    _file: File,
}

impl BootView {
    pub(crate) fn prepare(
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
        let mut created = None;
        for _ in 0..16 {
            let serial = NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| "boot view identities exhausted")?;
            let path =
                std::env::temp_dir().join(format!("rudroid-boot-{}-{serial}", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => {
                    created = Some((Temporary { path, linked: true }, file));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("cannot create private Unity boot view"),
            }
        }
        let (mut temporary, mut writer) =
            created.ok_or("cannot reserve private Unity boot view")?;
        writer
            .write_all(data.as_bytes())
            .map_err(|_| "cannot write private Unity boot view")?;
        writer
            .set_permissions(std::fs::Permissions::from_mode(0o400))
            .map_err(|_| "cannot protect private Unity boot view")?;
        let file =
            File::open(&temporary.path).map_err(|_| "cannot reopen private Unity boot view")?;
        drop(writer);
        std::fs::remove_file(&temporary.path)
            .map_err(|_| "cannot unlink private Unity boot view")?;
        temporary.linked = false;
        let path = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
        Ok(Self {
            source,
            path,
            _file: file,
        })
    }

    /// Only the selected file is projected; unrelated native files and aliases
    /// retain the resource resolver's ordinary behavior.
    pub(crate) fn redirect(&self, candidate: &CStr) -> Option<Vec<u8>> {
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
