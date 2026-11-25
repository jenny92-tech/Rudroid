//! INPUT: verified title inputs, explicit save paths and Linux network namespace
//! OUTPUT: checked manifest bytes and an isolated offline profile
//! POS: Soul Knight preflight before hook subscription or guest execution
#![forbid(unsafe_code)]
use super::{HostApiV0, Inputs, LIB_SHA, MANIFEST_PATH, MANIFEST_SHA, MARKER};
use crate::sha256;
use std::{ffi::{CStr, CString}, io::{Read, Write}, path::{Path, PathBuf}};

/// A failed ready-stage install precedes the first profile frame, so merely
/// returning a failed managed value cannot prevent all original startup I/O.
/// The launcher must create a fresh network namespace before guest bootstrap.
/// This gate is independent of phone firewall/radio experiments and fails
/// closed even when metadata, native patching or the shutdown path fails.
pub(super) fn require_network_isolation() -> Result<(), String> {
    let current = std::fs::read_link("/proc/self/ns/net").map_err(|_| "cannot verify offline network namespace")?;
    let initial = std::fs::read_link("/proc/1/ns/net").map_err(|_| "cannot verify initial network namespace")?;
    let devices = std::fs::read_to_string("/proc/net/dev").map_err(|_| "cannot inspect offline network interfaces")?;
    if isolated_network(&current.to_string_lossy(), &initial.to_string_lossy(), &devices) {
        Ok(())
    } else {
        Err("offline mode requires an independent network namespace containing only loopback".to_owned())
    }
}

pub(super) fn isolated_network(current: &str, initial: &str, devices: &str) -> bool {
    fn identity(value: &str) -> Option<u64> {
        let value = value.strip_prefix("net:[")?.strip_suffix(']')?;
        if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) { return None; }
        value.parse().ok().filter(|value| *value != 0)
    }
    let (Some(current), Some(initial)) = (identity(current), identity(initial)) else { return false; };
    if current == initial { return false; }
    let mut lines = devices.lines();
    if !lines.next().is_some_and(|line| line.contains("Inter-") && line.contains("Receive"))
        || !lines.next().is_some_and(|line| line.contains("face") && line.contains("bytes")) {
        return false;
    }
    let mut loopback = false;
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let Some((name, counters)) = line.split_once(':') else { return false; };
        if name.trim() != "lo" || loopback
            || counters.split_whitespace().count() != 16
            || !counters.split_whitespace().all(|v| v.bytes().all(|c| c.is_ascii_digit())) {
            return false;
        }
        loopback = true;
    }
    loopback
}

pub(super) fn prepare_inputs(host: &HostApiV0) -> Result<Inputs, String> {
    let game = configured_path(host, c"paths.game_files")?.canonicalize().map_err(|e| e.to_string())?;
    let library = game.join("lib/arm64-v8a/libil2cpp.so");
    if sha256::hex(&sha256::file(&library).map_err(|e| e.to_string())?) != LIB_SHA {
        return Err("offline libil2cpp identity mismatch".to_owned());
    }
    let raw = bounded_file(&game.join(MANIFEST_PATH), 203694)?;
    let mut hash = sha256::Sha256::new(); hash.update(&raw);
    if sha256::hex(&hash.finish()) != MANIFEST_SHA {
        return Err("offline original packaged manifest identity mismatch".to_owned());
    }
    let manifest = CString::new(String::from_utf8(raw).map_err(|_| "manifest is not UTF-8")?)
        .map_err(|_| "manifest contains NUL")?;
    let id = host.config_string(c"game_patches.soul_knight.local_profile_id", 128)
        .ok_or("offline local_profile_id is required")?;
    if !local_id(&id) { return Err("offline local_profile_id must use its own 32-hex namespace".to_owned()); }
    let root = configured_path(host, c"game_patches.soul_knight.profile_root")?;
    let writable = [c"paths.android_data", c"paths.android_files", c"paths.android_external_files", c"paths.android_cache"]
        .map(|key| configured_path(host, key)).into_iter().collect::<Result<Vec<_>, _>>()?;
    prepare_profile(&root, &game, &id, &writable)?;
    Ok(Inputs { manifest, profile_id: CString::new(id).expect("validated profile ID") })
}

fn configured_path(host: &HostApiV0, key: &CStr) -> Result<PathBuf, String> {
    let value = host.config_string(key, 4096).filter(|v| !v.is_empty())
        .ok_or_else(|| format!("offline mode needs explicit {}", key.to_string_lossy()))?;
    let path = PathBuf::from(value);
    Ok(if path.is_absolute() { path } else {
        std::env::current_dir().map_err(|e| e.to_string())?.join(path)
    })
}

pub(super) fn local_id(id: &str) -> bool {
    id.strip_prefix("rudroid-offline-80700-").is_some_and(|suffix|
        suffix.len() == 32 && suffix.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
}

pub(super) fn bounded_file(path: &Path, size: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() != size as u64 { return Err("offline input size/type mismatch".to_owned()); }
    let mut bytes = Vec::with_capacity(size);
    file.take(size as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() != size { return Err("offline input changed during read".to_owned()); }
    Ok(bytes)
}

pub(super) fn prepare_profile(root: &Path, game: &Path, id: &str, writable: &[PathBuf]) -> Result<(), String> {
    // A marker prevents accidentally adopting an existing game/profile tree.
    // Only this small generated identity is read; no account or save is inspected.
    if writable.len() != 4 || root.components().any(|part| part == std::path::Component::ParentDir) {
        return Err("offline profile needs four normalized writable roots".to_owned());
    }
    // Resolve an existing ancestor before creating anything, including when
    // a supplied root traverses a symlink. Original game inputs stay read-only.
    let prospective = prospective_path(root)?;
    if game.starts_with(&prospective) || prospective.starts_with(game) {
        return Err("offline profile must be independent of original game inputs".to_owned());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let expected = format!("rudroid-soul-knight-offline-v1:{id}\n");
    let marker = root.join(MARKER);
    if !marker.exists() {
        if std::fs::read_dir(&root).map_err(|e| e.to_string())?.next().is_some() {
            return Err("offline profile directory is not empty and has no ownership marker".to_owned());
        }
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&marker)
            .map_err(|e| e.to_string())?;
        file.write_all(expected.as_bytes()).map_err(|e| e.to_string())?;
    }
    if std::fs::symlink_metadata(&marker).map_err(|e| e.to_string())?.file_type().is_symlink()
        || bounded_file(&marker, expected.len())? != expected.as_bytes() {
        return Err("offline profile ownership marker mismatch".to_owned());
    }
    for path in writable {
        let prospective = prospective_path(path)?;
        if prospective == root || !prospective.starts_with(&root) {
            return Err("all four Android writable roots must be isolated profile subdirectories".to_owned());
        }
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        if path == root || !path.starts_with(&root) {
            return Err("Android writable root changed outside the isolated profile".to_owned());
        }
    }
    Ok(())
}

fn prospective_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|part| part == std::path::Component::ParentDir) {
        return Err("offline paths must be normalized absolute paths".to_owned());
    }
    let mut ancestor = path.to_owned();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(ancestor.file_name().ok_or("offline path has no existing ancestor")?.to_owned());
        if !ancestor.pop() { return Err("offline path has no existing ancestor".to_owned()); }
    }
    let mut resolved = ancestor.canonicalize().map_err(|e| e.to_string())?;
    for component in tail.into_iter().rev() { resolved.push(component); }
    Ok(resolved)
}
