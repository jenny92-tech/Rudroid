//! INPUT:  immutable Host API GPU policy and copied driver extension/string snapshots
//! OUTPUT:  validated capability limits, stable filtered extensions and binary-cache policy
//! POS:    safe GLES compatibility decisions; guest_graphics owns all native pointer access
#![forbid(unsafe_code)]
use rudroid_common::host_api::HostApiV0;
use std::{
    ffi::{CStr, CString},
    sync::OnceLock,
};

const BINARY_EXTENSIONS: [&str; 4] = [
    "GL_OES_get_program_binary",
    "GL_ARB_get_program_binary",
    "GL_ARM_mali_program_binary",
    "GL_MESA_program_binary_formats",
];
#[derive(Clone, Debug, PartialEq, Eq)]
struct Policy {
    binary_disabled: bool,
    cache_strings: bool,
    sharp_half_precision: bool,
    limits: Vec<(u32, i32)>,
    replace: Vec<String>,
    append: Vec<String>,
    remove: Vec<String>,
}
static POLICY: OnceLock<Policy> = OnceLock::new();
static EXTENSIONS: OnceLock<Extensions> = OnceLock::new();
// Five immutable driver strings, initialized separately. Warm GL queries do
// not take a mutex, allocate, hash or retain additional copies. Static ownership
// preserves the process-pinned pointer contract, including concurrent misses.
struct StringCache([OnceLock<CString>; 5]);
impl StringCache {
    const fn new() -> Self {
        Self([const { OnceLock::new() }; 5])
    }
    fn slot(&self, name: u32) -> Option<&OnceLock<CString>> {
        let index = match name {
            0x1f00..=0x1f03 => (name - 0x1f00) as usize,
            0x8b8c => 4,
            _ => return None,
        };
        Some(&self.0[index])
    }
    fn get(&self, name: u32) -> Option<&CStr> {
        self.slot(name)?.get().map(CString::as_c_str)
    }
    fn remember(&self, name: u32, value: &CStr) -> Option<&CStr> {
        Some(self.slot(name)?.get_or_init(|| value.to_owned()).as_c_str())
    }
}
static STRINGS: StringCache = StringCache::new();
pub(super) struct Extensions {
    words: Vec<CString>,
    joined: CString,
}
impl Policy {
    fn extension_dirty(&self) -> bool {
        self.binary_disabled
            || !self.replace.is_empty()
            || !self.append.is_empty()
            || !self.remove.is_empty()
    }
    fn extensions(&self, native: &[String]) -> Result<Extensions, String> {
        let mut words = if self.replace.is_empty() {
            native.to_vec()
        } else {
            self.replace.clone()
        };
        words.retain(|word| !self.remove.contains(word));
        for word in &self.append {
            if !words.contains(word) {
                words.push(word.clone());
            }
        }
        // A disabled feature must remain absent even if an append list repeats it.
        if self.binary_disabled {
            words.retain(|s| !BINARY_EXTENSIONS.contains(&s.as_str()));
        }
        let joined = CString::new(words.join(" ")).map_err(|_| "driver extensions contain NUL")?;
        let words = words
            .into_iter()
            .map(CString::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "driver extension contains NUL")?;
        Ok(Extensions { words, joined })
    }
    fn integer(&self, name: u32) -> Option<i32> {
        if self.binary_disabled && matches!(name, 0x87fe | 0x87ff) {
            return Some(0);
        }
        self.limits
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    }
}
fn words(value: String) -> Result<Vec<String>, String> {
    let words: Vec<_> = value.split_ascii_whitespace().map(str::to_owned).collect();
    if words.len() > 4096
        || words
            .iter()
            .any(|s| s.len() > 256 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    {
        return Err("invalid or excessive GLES extension policy".into());
    }
    Ok(words)
}
pub(super) fn configure(api: &HostApiV0) -> Result<(), String> {
    let mut limits = Vec::new();
    for (name, key, default, max) in [
        (0x0d33, c"gpu.maxTextureSizeOverride", 0, 32768),
        (0x8d57, c"gpu.maxSamplesOverride", -1, 64),
        (0x8824, c"gpu.maxDrawBuffersOverride", -1, 64),
        (0x8cdf, c"gpu.maxColorAttachmentsOverride", -1, 64),
        (0x88ff, c"gpu.maxArrayTextureLayersOverride", -1, 32768),
    ] {
        let value = api.config_i64(key, default).clamp(default, max) as i32;
        if (name == 0x0d33 && value > 0) || (name != 0x0d33 && value >= 0) {
            limits.push((name, if name == 0x0d33 { value.max(64) } else { value }));
        }
    }
    let policy = Policy {
        binary_disabled: api.config_bool(c"gpu.disableProgramBinary", false),
        cache_strings: api.config_bool(c"gpu.cacheGlStrings", true),
        sharp_half_precision: api.config_bool(c"gpu.renderScaleHalfPrecision", false),
        limits,
        replace: words(
            api.config_string(c"gpu.glExtensionsOverride", 65536)
                .unwrap_or_default(),
        )?,
        append: words(
            api.config_string(c"gpu.glExtensionsAppend", 65536)
                .unwrap_or_default(),
        )?,
        remove: words(
            api.config_string(c"gpu.glExtensionsRemove", 65536)
                .unwrap_or_default(),
        )?,
    };
    if POLICY.get_or_init(|| policy.clone()) != &policy {
        return Err("GLES capability policy already differs".into());
    }
    eprintln!(
        "[SDL] GLES capability policy: program_binary_disabled={} extension_filter={} limits={} sharp_half_precision={}",
        policy.binary_disabled,
        policy.extension_dirty(),
        policy.limits.len(),
        policy.sharp_half_precision
    );
    Ok(())
}
pub(super) fn binary_disabled() -> bool {
    POLICY.get().is_some_and(|s| s.binary_disabled)
}
// Read only while creating the private sharp program, never on a frame/draw path.
pub(super) fn sharp_half_precision() -> bool {
    POLICY.get().is_some_and(|s| s.sharp_half_precision)
}
pub(super) fn extension_dirty() -> bool {
    POLICY.get().is_some_and(Policy::extension_dirty)
}
pub(super) fn initialize_extensions(native: &[String]) -> Result<(), String> {
    let Some(policy) = POLICY.get() else {
        return Ok(());
    };
    if !policy.extension_dirty() {
        return Ok(());
    }
    let result = policy.extensions(native)?;
    let _ = EXTENSIONS.set(result);
    Ok(())
}
pub(super) fn extension_string() -> Option<&'static CStr> {
    EXTENSIONS.get().map(|s| s.joined.as_c_str())
}
pub(super) fn extension(index: u32) -> Option<&'static CStr> {
    EXTENSIONS
        .get()?
        .words
        .get(index as usize)
        .map(CString::as_c_str)
}
pub(super) fn integer(name: u32) -> Option<i32> {
    if name == 0x821d && extension_dirty() {
        return Some(EXTENSIONS.get()?.words.len() as i32);
    }
    POLICY.get()?.integer(name)
}
pub(super) fn cached_string(name: u32) -> Option<&'static CStr> {
    STRINGS.get(name)
}
pub(super) fn remember_string(name: u32, value: &CStr) -> Option<&'static CStr> {
    if !POLICY.get().is_some_and(|s| s.cache_strings) || !matches!(name, 0x1f00..=0x1f03 | 0x8b8c) {
        return None;
    }
    STRINGS.remember(name, value)
}
#[cfg(test)]
#[path = "../tests/unit/gles_policy.rs"]
mod tests;
