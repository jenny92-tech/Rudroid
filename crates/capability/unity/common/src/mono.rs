//! INPUT:  opt-in Unity Mono settings and a prepared Android package manifest
//! OUTPUT:  checked, absolute paths for the native bridge and Android player
//! POS:    fails before loading trusted native code if the declared identities differ
#![forbid(unsafe_code)]

use crate::mono_abi::{
    FRAME_PACING_ANDROID, FRAME_PACING_DESKTOP, FRAME_PACING_OBSERVE, MAX_DISPLAY_REFRESH_HZ,
    MIN_DISPLAY_REFRESH_HZ,
};
use rudroid_common::host_api::HostApiV0;
use rudroid_host_config::HostConfig;
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const ABI: &str = "arm64-v8a";
const BRIDGE: &str = "libil2cpp.so";
const RUNTIME: &str = "libmonobdwgc-2.0.so";

#[derive(Debug, Clone)]
pub struct MonoConfig {
    pub bridge: PathBuf,
    pub runtime: PathBuf,
    pub managed_dir: PathBuf,
    pub guest_lib_dir: PathBuf,
    pub unity_version: String,
    pub plugins_map: Option<PathBuf>,
    pub frame_pacing: u32,
    pub display_refresh_hz: u32,
}

struct Manifest {
    package: String,
    version_code: u32,
    unity_version: String,
    abi: String,
    scripting: String,
    apk: String,
}

/// Raw Mono section fields before absolute-path / manifest validation.
struct MonoSection<'a> {
    bridge: &'a str,
    runtime: &'a str,
    managed_dir: &'a str,
    manifest: &'a str,
    unity_version: &'a str,
    abi: Option<&'a str>,
    /// `None` means the key was omitted (allow sibling `plugins.map` fallback).
    plugins_map: Option<&'a str>,
    plugins_map_key_present: bool,
    frame_pacing: u32,
    display_refresh_hz: u32,
    package_name: Option<&'a str>,
    version_code: Option<i64>,
    android_package_code: &'a str,
    game_files: &'a str,
    has_il2cpp_patch: bool,
    has_il2cpp_hooks: bool,
}

/// Opt-in prepared Mono path resolution over an unchanged Host Config schema.
pub fn mono_config(config: &HostConfig) -> Result<Option<MonoConfig>, String> {
    let Some(section) = config.node("unity.mono") else {
        return Ok(None);
    };
    let section = section.as_table().ok_or("unity.mono must be a table")?;
    let result = (|| {
        for key in section.keys() {
            if !matches!(
                key.as_str(),
                "bridge"
                    | "runtime"
                    | "managed_dir"
                    | "manifest"
                    | "unity_version"
                    | "abi"
                    | "plugins_map"
                    | "frame_pacing"
                    | "display_refresh_hz"
            ) {
                return Err(format!("unknown field {key}"));
            }
        }
        let required = |key: &str| {
            section
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{key} must be a nonempty string"))
        };
        let frame_pacing = match section.get("frame_pacing") {
            None => FRAME_PACING_ANDROID,
            Some(value) => match value.as_str() {
                Some("android") => FRAME_PACING_ANDROID,
                Some("observe") => FRAME_PACING_OBSERVE,
                Some("desktop") => FRAME_PACING_DESKTOP,
                _ => return Err("frame_pacing must be android, observe or desktop".into()),
            },
        };
        let display_refresh_hz = match (frame_pacing, section.get("display_refresh_hz")) {
            (FRAME_PACING_DESKTOP, value) => value
                .and_then(|value| value.as_integer())
                .and_then(|hz| u32::try_from(hz).ok())
                .filter(|hz| (MIN_DISPLAY_REFRESH_HZ..=MAX_DISPLAY_REFRESH_HZ).contains(hz))
                .ok_or("desktop pacing needs display_refresh_hz in 24..=360")?,
            (_, None) => 0,
            _ => return Err("display_refresh_hz is only used with desktop pacing".into()),
        };
        let package_name = config
            .node("package.packageName")
            .and_then(|v| v.as_str());
        let version_code = config
            .node("package.versionCode")
            .and_then(|v| v.as_integer());
        let android_package_code = config
            .get_string("paths.android_package_code", "")
            .filter(|v| !v.is_empty())
            .ok_or("requires paths.android_package_code")?;
        let game_files = config
            .get_string("paths.game_files", "")
            .filter(|v| !v.is_empty())
            .ok_or("requires paths.game_files")?;
        let bridge = required("bridge")?;
        let runtime = required("runtime")?;
        let managed_dir = required("managed_dir")?;
        let manifest = required("manifest")?;
        let unity_version = required("unity_version")?;
        let abi = section
            .get("abi")
            .map(|_| required("abi"))
            .transpose()?;
        let plugins_map_key_present = section.contains_key("plugins_map");
        let plugins_map = if plugins_map_key_present {
            Some(required("plugins_map")?)
        } else {
            None
        };
        resolve_mono(MonoSection {
            bridge,
            runtime,
            managed_dir,
            manifest,
            unity_version,
            abi,
            plugins_map,
            plugins_map_key_present,
            frame_pacing,
            display_refresh_hz,
            package_name,
            version_code,
            android_package_code: &android_package_code,
            game_files: &game_files,
            has_il2cpp_patch: config.array_size("il2cpp_patch") > 0,
            has_il2cpp_hooks: config
                .node("il2cpp_hooks")
                .and_then(|v| v.as_table())
                .is_some_and(|t| !t.is_empty()),
        })
    })();
    result.map(Some).map_err(|e| format!("unity.mono: {e}"))
}

/// Same validation as [`mono_config`], parsing the authoritative Host Config
/// TOML exposed via `rudroid.host.config_toml.v1` so both paths share one
/// schema/unknown-key/type parser.
pub fn mono_config_from_api(api: &HostApiV0) -> Result<Option<MonoConfig>, String> {
    let Some(toml) = api.config_toml() else {
        return Ok(None);
    };
    let config =
        HostConfig::parse(toml).map_err(|e| format!("unity.mono: invalid Host Config TOML: {e}"))?;
    let Some(mono) = mono_config(&config)? else {
        return Ok(None);
    };
    if api.has_capability(c"il2cpp_patch") {
        return Err(
            "unity.mono: unity.mono cannot be combined with il2cpp_patch or il2cpp_hooks".into(),
        );
    }
    Ok(Some(mono))
}

/// True when the authoritative Host Config carries a `[unity.mono]` node
/// (matching [`mono_config`]'s activation predicate exactly).
pub fn mono_section_present_in_api(api: &HostApiV0) -> bool {
    let Some(toml) = api.config_toml() else {
        return false;
    };
    let Ok(config) = HostConfig::parse(toml) else {
        return false;
    };
    config.node("unity.mono").is_some()
}

fn resolve_mono(section: MonoSection<'_>) -> Result<MonoConfig, String> {
    let display_refresh_hz = section.display_refresh_hz;
    let abi = section.abi.unwrap_or(ABI);
    if abi != ABI {
        return Err(format!("unsupported ABI {abi}, expected {ABI}"));
    }
    if !cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "gnu"
    )) {
        return Err("requires ARM64 Linux with glibc".into());
    }
    if !valid_version(section.unity_version) {
        return Err("invalid unity_version".into());
    }
    let manifest_path = absolute(section.manifest, false)?;
    let manifest = read_manifest(&manifest_path)?;
    if manifest.scripting != "mono"
        || manifest.abi != abi
        || manifest.unity_version != section.unity_version
    {
        return Err(
            "manifest scripting, ABI or Unity version disagrees with unity.mono".into(),
        );
    }
    if section.package_name != Some(manifest.package.as_str())
        || manifest.package.is_empty()
        || section.version_code != Some(i64::from(manifest.version_code))
        || manifest.version_code == 0
    {
        return Err(
            "manifest package or version code disagrees with package configuration".into(),
        );
    }
    let manifest_dir = manifest_path
        .parent()
        .ok_or("manifest has no parent directory")?;
    if !basename(&manifest.apk) || !manifest.apk.ends_with(".apk") {
        return Err("manifest apk must be a single APK basename".into());
    }
    let expected_apk = canonical(&manifest_dir.join(&manifest.apk), false)?;
    if absolute(section.android_package_code, false)? != expected_apk {
        return Err("paths.android_package_code differs from the manifest APK".into());
    }
    if section.has_il2cpp_patch || section.has_il2cpp_hooks {
        return Err("unity.mono cannot be combined with il2cpp_patch or il2cpp_hooks".into());
    }
    let bridge = absolute(section.bridge, false)?;
    let runtime = absolute(section.runtime, false)?;
    if bridge.file_name().and_then(|n| n.to_str()) != Some(BRIDGE)
        || runtime.file_name().and_then(|n| n.to_str()) != Some(RUNTIME)
        || bridge.parent() != runtime.parent()
    {
        return Err(format!(
            "bridge and runtime must be sibling {BRIDGE} and {RUNTIME} files"
        ));
    }
    let managed_dir = absolute(section.managed_dir, true)?;
    let core = canonical(&managed_dir.join("mscorlib.dll"), false)?;
    if !core.starts_with(&managed_dir) {
        return Err("mscorlib.dll escapes managed_dir".into());
    }
    let root = canonical(Path::new(section.game_files), true)?;
    let guest_lib_dir = canonical(&root.join("lib").join(ABI), true)?;
    if !guest_lib_dir.starts_with(&root) {
        return Err("the Android player directory must stay under game_files".into());
    }
    let alias_path = guest_lib_dir.join(BRIDGE);
    match alias_path.symlink_metadata() {
        Ok(_) => {
            let player_alias = canonical(&alias_path, false)?;
            if player_alias != bridge {
                return Err(format!(
                    "the Android player's {BRIDGE} must resolve to the configured bridge"
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Handheld SD cards (FAT32/exFAT) cannot create symlinks to a shared
            // Mono bridge on internal storage; when absent, native_alias maps the
            // guest path directly to `bridge`. A stray Android libil2cpp.so is still
            // rejected by the Ok(_) arm above.
        }
        Err(error) => return Err(format!("{}: {error}", alias_path.display())),
    }
    for name in ["libmain.so", "libunity.so"] {
        let player = canonical(&guest_lib_dir.join(name), false)?;
        if !player.starts_with(&guest_lib_dir) {
            return Err(format!("{name} escapes the Android player directory"));
        }
    }
    let declared_map = manifest_dir.join("plugins.map");
    let plugins_map = if section.plugins_map_key_present {
        Some(absolute(
            section
                .plugins_map
                .ok_or("plugins_map must be a nonempty string")?,
            false,
        )?)
    } else if declared_map
        .symlink_metadata()
        .map(|_| true)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(error)
            }
        })
        .map_err(|e| format!("{}: {e}", declared_map.display()))?
    {
        let resolved = canonical(&declared_map, false)?;
        if !resolved.starts_with(manifest_dir) {
            return Err("plugins.map escapes the manifest directory".into());
        }
        Some(resolved)
    } else {
        None
    };
    Ok(MonoConfig {
        bridge,
        runtime,
        managed_dir,
        guest_lib_dir,
        unity_version: section.unity_version.into(),
        plugins_map,
        frame_pacing: section.frame_pacing,
        display_refresh_hz,
    })
}

#[cfg(test)]
#[path = "../tests/unit/mono.rs"]
mod tests;

fn basename(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\', '\0'])
}

fn valid_version(version: &str) -> bool {
    let mut parts = version.split('.');
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let valid_revision = |rev: &str| {
        if let Some((main, china)) = rev.split_once('c') {
            digits(main) && digits(china)
        } else {
            digits(rev)
        }
    };
    version.len() <= 64
        && parts.next().is_some_and(digits)
        && parts.next().is_some_and(digits)
        && parts.next().is_some_and(|patch| {
            patch
                .split_once(['a', 'b', 'f', 'p'])
                .is_some_and(|(number, revision)| digits(number) && valid_revision(revision))
        })
        && parts.next().is_none()
}

fn absolute(value: &str, directory: bool) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(format!("{} must be an absolute path", path.display()));
    }
    canonical(path, directory)
}

fn canonical(path: &Path, directory: bool) -> Result<PathBuf, String> {
    let resolved = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = fs::metadata(&resolved).map_err(|e| format!("{}: {e}", path.display()))?;
    if !(if directory {
        meta.is_dir()
    } else {
        meta.is_file()
    }) {
        return Err(format!("{} has the wrong file type", resolved.display()));
    }
    if !resolved
        .to_str()
        .is_some_and(|text| text.len() < 4096 && !text.contains('\0'))
    {
        return Err(format!(
            "{} requires a UTF-8 path shorter than 4096 bytes",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    const LIMIT: u64 = 64 * 1024;
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > LIMIT {
        return Err(format!(
            "{} exceeds the manifest size limit",
            path.display()
        ));
    }
    let doc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let string = |key: &str| {
        doc.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("{}: {key} must be a string", path.display()))
    };
    let version_code = doc
        .get("version_code")
        .and_then(|v| v.as_u64())
        .and_then(|code| u32::try_from(code).ok())
        .ok_or_else(|| {
            format!(
                "{}: version_code must be an unsigned 32-bit integer",
                path.display()
            )
        })?;
    Ok(Manifest {
        package: string("package")?,
        version_code,
        unity_version: string("unity_version")?,
        abi: string("abi")?,
        scripting: string("scripting")?,
        apk: string("apk")?,
    })
}
