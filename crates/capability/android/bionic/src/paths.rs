//! INPUT:  Android asset URI bytes, extracted asset directories and host cwd
//! OUTPUT:  owned native paths for extracted Android asset reads
//! POS:    one safe resolver shared by guest fd and stdio ABI adapters
#![forbid(unsafe_code)]

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
static HOST: std::sync::OnceLock<rudroid_common::host_api::HostApiV0> = std::sync::OnceLock::new();
pub(crate) fn install(host: &rudroid_common::host_api::HostApiV0) { HOST.get_or_init(||*host); }

/// Ordinary native paths are unchanged. Only the Android asset namespace is
/// mapped; suffixes may not escape the extracted assets directory.
pub(crate) fn extracted_asset(input: &CStr) -> Option<CString> {
    if let Some(host)=HOST.get() {
        return host.asset_path(rudroid_common::android_assets::PATH,input,None,0).ok().flatten();
    }
    let bytes = input.to_bytes();
    let mut prefixed_root = None;
    let (archive, relative) = if let Some(split) = bytes.windows(b"jar:file:".len()).position(|w| w == b"jar:file:") {
        if split != 0 {
            let prefix = PathBuf::from(OsStr::from_bytes(&bytes[..split]));
            if bytes[split - 1] != b'/' || !prefix.is_dir() { return None; }
            prefixed_root = Some(if prefix.is_absolute() { prefix } else { std::env::current_dir().ok()?.join(prefix) });
        }
        let uri = &bytes[split + b"jar:file:".len()..];
        let uri = uri.strip_prefix(b"//").unwrap_or(uri);
        let split = uri.iter().position(|b| *b == b'!')?;
        (&uri[..split], uri[split + 1..].strip_prefix(b"/assets/")?)
    } else {
        // Unity's jar handler also emits /assets/... after stripping the
        // empty archive. Preserve any existing ordinary absolute host file.
        let base_asset = bytes.strip_prefix(b"/assets/").filter(|_| !Path::new(OsStr::from_bytes(bytes)).exists());
        let relative = bytes.strip_prefix(b"/android_asset/")
            .or(base_asset)
            .or_else(|| bytes.strip_prefix(b"file:///android_asset/"))?;
        (&b""[..], relative)
    };
    if relative.is_empty() || relative.contains(&b'\\') {
        return None;
    }
    let relative = Path::new(OsStr::from_bytes(relative));
    if relative.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return None;
    }
    let root = if archive.iter().all(|b| *b == b'/') {
        prefixed_root.or_else(|| std::env::current_dir().ok())?
    } else {
        let path = PathBuf::from(OsStr::from_bytes(archive));
        let path = if path.extension() == Some(OsStr::new("apk")) && !path.is_dir() {
            path.parent()?.to_owned()
        } else { path };
        if path.is_absolute() { path } else { std::env::current_dir().ok()?.join(path) }
    };
    CString::new(root.join("assets").join(relative).as_os_str().as_bytes()).ok()
}

#[cfg(test)]
#[path = "../tests/unit/paths.rs"]
mod tests;
