//! INPUT:  extracted game root, installed pack directories and typed asset queries
//! OUTPUT:  AssetResolver, AssetRequest and bounded owned resource replies
//! POS:    one safe filesystem model shared by the loader's Java/NDK/native resource service
#![forbid(unsafe_code)]

use crate::registry::HostStr;
use std::collections::BTreeSet;
use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

pub const MAX_REPLY: usize = 1024 * 1024;
pub const MAX_PACKS: usize = 64;
pub const PATH: u32 = 0;
pub const MERGED: u32 = 1;
pub const BASE: u32 = 2;
pub const PACK: u32 = 3;
pub const BASE_ROOT: u32 = 4;
pub const PACK_ROOT: u32 = 5;
pub const SPLIT: u32 = 6;
pub const DIRECTORY: u32 = 7;

/// Borrowed request strings are valid only for this synchronous query.
#[repr(C)]
pub struct AssetRequest<'a> {
    pub kind: u32,
    pub path: HostStr<'a>,
    pub pack: HostStr<'a>,
    pub index: u32,
}
impl<'a> AssetRequest<'a> {
    pub fn new(kind: u32, path: &'a CStr, pack: Option<&'a CStr>, index: u32) -> Self {
        Self {
            kind,
            path: HostStr::new(path),
            pack: pack.map(HostStr::new).unwrap_or(HostStr::null()),
            index,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetError {
    Missing,
    Invalid,
    TooLarge,
    Changed,
}

/// Immutable snapshot. The host publishes pack discovery once before JNI/game startup.
#[derive(Clone)]
pub struct AssetResolver {
    root: PathBuf,
    packs: Vec<(CString, PathBuf)>,
    packs_enabled: bool,
    shader_cache: Option<PathBuf>,
    block_analytics: bool,
}

fn pack_name(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes != b"."
        && !bytes.windows(2).any(|w| w == b"..")
        && !bytes.iter().any(|b| matches!(b, b'/' | b'\\' | 0))
}
fn relative(bytes: &[u8], empty: bool) -> Result<PathBuf, AssetError> {
    if bytes.contains(&b'\\') || bytes.contains(&0) || (!empty && bytes.is_empty()) {
        return Err(AssetError::Invalid);
    }
    let path = Path::new(OsStr::from_bytes(bytes));
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(p) => normalized.push(p),
            Component::CurDir => {}
            _ => return Err(AssetError::Invalid),
        }
    }
    if !empty && normalized.as_os_str().is_empty() {
        return Err(AssetError::Invalid);
    }
    Ok(normalized)
}
fn encoded(path: &Path) -> Result<Vec<u8>, AssetError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= MAX_REPLY || bytes.contains(&0) {
        return Err(AssetError::TooLarge);
    }
    let mut out = bytes.to_vec();
    out.push(0);
    Ok(out)
}

impl AssetResolver {
    pub fn new(root: PathBuf, shader_cache: Option<PathBuf>, block_analytics: bool) -> Self {
        let absolute = |p: PathBuf| if p.is_absolute() { p } else { root.join(p) };
        Self {
            shader_cache: shader_cache.map(absolute),
            root,
            packs: Vec::new(),
            packs_enabled: false,
            block_analytics,
        }
    }
    pub fn with_packs(&self) -> std::io::Result<Self> {
        let mut next = self.clone();
        next.packs_enabled = true;
        next.packs.clear();
        let directory = self.root.join("assetpacks");
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(next),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if pack_name(name.as_bytes()) && entry.path().join("assets").is_dir() {
                next.packs.push((
                    CString::new(name.as_bytes()).map_err(std::io::Error::other)?,
                    entry.path().join("assets"),
                ));
            }
        }
        next.packs
            .sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        next.packs.truncate(MAX_PACKS);
        Ok(next)
    }
    pub fn pack_count(&self) -> usize {
        self.packs.len()
    }
    fn base_roots(&self) -> [PathBuf; 2] {
        [self.root.join("gamedata/assets"), self.root.join("assets")]
    }
    fn pack_root(&self, requested: Option<&CStr>) -> Result<Option<&Path>, AssetError> {
        let name = requested
            .filter(|s| !s.is_empty())
            .map(CStr::to_bytes)
            .unwrap_or(b"UnityDataAssetPack");
        if !pack_name(name) {
            return Err(AssetError::Invalid);
        }
        Ok(self
            .packs
            .iter()
            .find(|p| p.0.as_bytes() == name)
            .map(|p| p.1.as_path()))
    }
    fn resolve(
        &self,
        path: &Path,
        kind: u32,
        pack: Option<&CStr>,
    ) -> Result<Option<PathBuf>, AssetError> {
        if kind != PACK {
            for root in self.base_roots() {
                let candidate = root.join(path);
                if candidate.exists() {
                    return Ok(Some(candidate));
                }
            }
        }
        if kind != BASE {
            if let Some(root) = self.pack_root(pack)? {
                let candidate = root.join(path);
                if candidate.exists() {
                    return Ok(Some(candidate));
                }
            }
        }
        Ok(None)
    }
    fn path(&self, input: &CStr) -> Result<Option<PathBuf>, AssetError> {
        let bytes = input.to_bytes();
        if self.block_analytics
            && bytes
                .windows(b"/Analytics/".len())
                .any(|w| w == b"/Analytics/")
        {
            return Ok(Some(PathBuf::from("/dev/null")));
        }
        let font = bytes
            .strip_prefix(b"/system/fonts/")
            .or_else(|| {
                if bytes == b"/system/fonts" {
                    Some(&b""[..])
                } else {
                    None
                }
            })
            .or_else(|| match bytes {
                b"/etc/fonts.xml" => Some(&b"fonts.xml"[..]),
                b"/etc/system_fonts.xml" => Some(&b"system_fonts.xml"[..]),
                b"/vendor/etc/fallback_fonts.xml" => Some(&b"fallback_fonts.xml"[..]),
                _ => None,
            });
        if let Some(suffix) = font {
            return Ok(Some(self.root.join("fonts").join(relative(suffix, true)?)));
        }
        if let Some(root) = &self.shader_cache {
            let original = Path::new(OsStr::from_bytes(bytes));
            if !original.starts_with(root) {
                if let Some(at) = bytes
                    .windows(b"UnityShaderCache".len())
                    .position(|w| w == b"UnityShaderCache")
                {
                    let suffix = &bytes[at + b"UnityShaderCache".len()..];
                    let suffix = suffix.strip_prefix(b"/").unwrap_or(suffix);
                    return Ok(Some(root.join(relative(suffix, true)?)));
                }
            }
        }
        // Explicit split identities take precedence over base aliases, including when the APK was removed after extraction.
        let split = bytes.windows(b"split_".len()).position(|w| w == b"split_");
        if let Some(at) = split {
            let tail = &bytes[at + b"split_".len()..];
            for marker in [&b".apk!/assets/"[..], &b".apk/assets/"[..]] {
                if let Some(end) = tail.windows(marker.len()).position(|w| w == marker) {
                    if !self.packs_enabled {
                        return Err(AssetError::Missing);
                    }
                    let pack = CString::new(&tail[..end]).map_err(|_| AssetError::Invalid)?;
                    let rel = relative(&tail[end + marker.len()..], false)?;
                    if let Some(path) = self.resolve(&rel, PACK, Some(&pack))? {
                        return Ok(Some(path));
                    }
                    if matches!(
                        rel.as_os_str().as_bytes(),
                        b"bin/Data/datapack.unity3d"
                            | b"bin/Data/data.unity3d"
                            | b"bin/Data/resources.resource"
                    ) {
                        return Err(AssetError::Missing);
                    }
                    return self
                        .resolve(&rel, BASE, None)?
                        .map(Some)
                        .ok_or(AssetError::Missing);
                }
            }
        }
        if let Some(at) = bytes
            .windows(b"jar:file:".len())
            .position(|w| w == b"jar:file:")
        {
            if at != 0
                && (bytes[at - 1] != b'/' || !Path::new(OsStr::from_bytes(&bytes[..at])).is_dir())
            {
                return Err(AssetError::Invalid);
            }
            let uri = &bytes[at + b"jar:file:".len()..];
            let uri = uri.strip_prefix(b"//").unwrap_or(uri);
            let end = uri
                .iter()
                .position(|b| *b == b'!')
                .ok_or(AssetError::Invalid)?;
            let suffix = uri[end + 1..]
                .strip_prefix(b"/assets/")
                .ok_or(AssetError::Invalid)?;
            let rel = relative(suffix, false)?;
            let archive = PathBuf::from(OsStr::from_bytes(&uri[..end]));
            if archive.as_os_str().as_bytes().iter().all(|b| *b == b'/') {
                return self
                    .resolve(&rel, MERGED, None)?
                    .map(Some)
                    .ok_or(AssetError::Missing);
            }
            let archive = if archive.is_absolute() {
                archive
            } else {
                self.root.join(archive)
            };
            let root = if archive.extension() == Some(OsStr::new("apk")) && !archive.is_dir() {
                archive.parent().ok_or(AssetError::Invalid)?.to_owned()
            } else {
                archive
            };
            let candidate = root.join("assets").join(&rel);
            if candidate.exists() {
                return Ok(Some(candidate));
            }
            return self
                .resolve(&rel, MERGED, None)?
                .map(Some)
                .ok_or(AssetError::Missing);
        }
        let native = Path::new(OsStr::from_bytes(bytes));
        let base = bytes.strip_prefix(b"/assets/").filter(|_| !native.exists());
        if let Some(suffix) = bytes
            .strip_prefix(b"/android_asset/")
            .or(base)
            .or_else(|| bytes.strip_prefix(b"android_asset/"))
            .or_else(|| bytes.strip_prefix(b"file:///android_asset/"))
            .or_else(|| bytes.strip_prefix(b"file://android_asset/"))
        {
            let rel = relative(suffix, false)?;
            return self
                .resolve(&rel, MERGED, None)?
                .map(Some)
                .ok_or(AssetError::Missing);
        }
        Ok(None)
    }
    /// Path replies are single C strings; directory replies are sorted C strings followed by an empty terminator.
    pub fn query(&self, request: &AssetRequest<'_>) -> Result<Option<Vec<u8>>, AssetError> {
        let path = request.path.to_cstr().ok_or(AssetError::Invalid)?;
        let pack = request.pack.to_cstr();
        let result = match request.kind {
            PATH => return self.path(path)?.map(|p| encoded(&p)).transpose(),
            BASE | MERGED | PACK => {
                self.resolve(&relative(path.to_bytes(), false)?, request.kind, pack)?
            }
            BASE_ROOT => self.base_roots().into_iter().find(|p| p.is_dir()),
            PACK_ROOT => self.pack_root(pack)?.map(Path::to_owned),
            SPLIT => self.packs.get(request.index as usize).map(|p| {
                self.root
                    .join(format!("split_{}.apk", p.0.to_string_lossy()))
            }),
            DIRECTORY => {
                let rel = relative(path.to_bytes(), true)?;
                let mut roots = self.base_roots().to_vec();
                if let Some(root) = self.pack_root(pack)? {
                    roots.push(root.to_owned());
                }
                let mut names = BTreeSet::new();
                for root in roots {
                    let items = match std::fs::read_dir(root.join(&rel)) {
                        Ok(items) => items,
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                            ) =>
                        {
                            continue
                        }
                        Err(_) => return Err(AssetError::Missing),
                    };
                    for item in items {
                        let item = item.map_err(|_| AssetError::Missing)?;
                        if item.path().is_file() {
                            let name = item.file_name().as_bytes().to_vec();
                            if name.contains(&0) || names.len() >= 4096 {
                                return Err(AssetError::TooLarge);
                            }
                            names.insert(name);
                        }
                    }
                }
                let mut bytes = Vec::new();
                for name in names {
                    if bytes.len() + name.len() + 2 > MAX_REPLY {
                        return Err(AssetError::TooLarge);
                    }
                    bytes.extend(name);
                    bytes.push(0);
                }
                bytes.push(0);
                return Ok(Some(bytes));
            }
            _ => return Err(AssetError::Invalid),
        };
        result.map(|p| encoded(&p)).transpose()
    }
}

#[cfg(test)]
#[path = "../tests/unit/android_assets.rs"]
mod tests;
