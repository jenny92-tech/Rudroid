//! INPUT:  existing Host Config TOML and optional measured SDL output dimensions
//! OUTPUT:  HostConfig typed views, getters and validated presentation/display/memory/Unity startup policies
//! POS:    resolves compatible configuration while keeping output and guest rendering separate
//! Compatible Host Config TOML reader.
//!
//! Reads the exact TOML dialect the C++ reference already uses
//! (`[paths]`, `[package]`, `[device]`, `[plugins]`, `[input]`, `[unity]`,
//! `[gpu]`, `[sdk_fmod]`, `[game_patches.*]`, ...). Unknown keys and sections
//! are preserved, never a hard failure: the full document stays available as
//! a table and dotted-key accessors mirror the reference `RudroidPluginApi`
//! config getters (`config_get_bool` semantics included). Dotted walks
//! accept numeric array segments, so legacy arrays of tables
//! (`[[il2cpp_patch]]` → `il2cpp_patch.<index>.<field>`) read exactly like
//! the reference `config_node` lexer, and leaf reads apply the same
//! permissive type conversions as toml++ `value<T>()`.
//!
//! This crate never rewrites the schema and never merges the Reply Catalog
//! format in: Host Config and Reply Catalog stay separate artifacts.
#![forbid(unsafe_code)]

use std::path::Path;
use rudroid_common::runtime_policy::{MemoryMode, MemoryPolicy};
mod unity_startup;
pub use unity_startup::{UnityRenderThread, UnityStartupPolicy, MAX_BOOT_CONFIG_BYTES};

/// Parsed Host Config document.
#[derive(Debug, Clone)]
pub struct HostConfig {
    table: toml::Table,
}

/// Optional presentation owner. Internal keeps the existing sharp/linear GL pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationBackend {
    Internal,
    WaylandNative,
}

/// Explicit startup selection. GLES remains the compatible default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsApi { Gles, Vulkan }

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    /// Parsed, but the root is not a TOML table.
    NotATable,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "cannot read host config: {e}"),
            ConfigError::Parse(e) => write!(f, "invalid host config TOML: {e}"),
            ConfigError::NotATable => write!(f, "host config root must be a table"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

/// Base-10 unsigned index for one dotted-key array segment, matching the
/// reference `config_node` lexer (`std::from_chars` into `size_t`): digits
/// only, the whole segment consumed, no sign, no whitespace — `"00"` is 0,
/// while `""`, `"-1"`, `"+1"`, `"x"` and `"1x"` all miss.
fn array_segment_index(segment: &str) -> Option<usize> {
    if segment.is_empty() {
        return None;
    }
    let mut index: usize = 0;
    for byte in segment.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        index = index.checked_mul(10)?.checked_add((byte - b'0') as usize)?;
    }
    Some(index)
}

/// Leaf value conversions with the reference oracle's permissive semantics
/// (`toml::node::value<T>()`, toml++ v3.4): some native types promote to
/// others, and everything that does not promote keeps the caller's fallback.
mod convert {
    /// bool -> bool; integer -> bool via `static_cast<bool>(int64)` (nonzero
    /// is true). Floats and strings never become bools.
    pub fn to_bool(value: &toml::Value) -> Option<bool> {
        match value {
            toml::Value::Boolean(v) => Some(*v),
            toml::Value::Integer(v) => Some(*v != 0),
            _ => None,
        }
    }

    /// integer -> i64 direct; float -> i64 only when the float IS an exact
    /// integer within range (toml++ checks the round-trip is lossless;
    /// out-of-range doubles that are UB in C++ are a safe miss here); bool
    /// -> i64 as 0/1; strings never.
    pub fn to_i64(value: &toml::Value) -> Option<i64> {
        match value {
            toml::Value::Integer(v) => Some(*v),
            toml::Value::Float(v) => {
                let truncated = v.trunc();
                let in_range = truncated >= -(2f64.powi(63)) && truncated < 2f64.powi(63);
                if v.is_finite() && truncated == *v && in_range {
                    Some(truncated as i64)
                } else {
                    None
                }
            }
            toml::Value::Boolean(v) => Some(*v as i64),
            _ => None,
        }
    }

    /// `value<int32_t>()`: the `to_i64` conversions, then the toml++
    /// `node_integer_cast` range check — out-of-range values miss.
    pub fn to_i32(value: &toml::Value) -> Option<i32> {
        to_i64(value).and_then(|v| i32::try_from(v).ok())
    }

    /// float -> f64 direct; integer -> f64 only while losslessly
    /// representable (|v| <= 2^53, the toml++ `largest_whole_float` bound);
    /// bools and strings never.
    pub fn to_f64(value: &toml::Value) -> Option<f64> {
        match value {
            toml::Value::Float(v) => Some(*v),
            toml::Value::Integer(v) => {
                // |v| <= 2^53 fits u64 comfortably (i64 max is 2^63).
                if v.unsigned_abs() <= 1u64 << 53 {
                    Some(*v as f64)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// `[paths]` keys shared by every device config (`configs/unity.toml` schema).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Paths {
    pub game_files: Option<String>,
    pub support_files: Option<String>,
    pub android_package_code: Option<String>,
    pub android_external_files: Option<String>,
    pub android_files: Option<String>,
    pub android_cache: Option<String>,
    pub android_obb_dirs: Vec<String>,
    pub android_source_dirs: Vec<String>,
}

/// `[package]` identity used by gamepatch package gates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Package {
    pub package_name: Option<String>,
    pub version_name: Option<String>,
}

/// `[device]` display block. Defaults mirror the reference:
/// 640x480, no rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub display_width: i64,
    pub display_height: i64,
    pub display_rotation: i64,
}

impl Default for Device {
    fn default() -> Self {
        Device {
            display_width: 640,
            display_height: 480,
            display_rotation: 0,
        }
    }
}

impl HostConfig {
    pub fn graphics_api(&self) -> Result<GraphicsApi, &'static str> {
        match self.node("gpu.graphicsApi") {
            None => Ok(GraphicsApi::Gles),
            Some(toml::Value::String(value)) if value == "gles" => Ok(GraphicsApi::Gles),
            Some(toml::Value::String(value)) if value == "vulkan" => Ok(GraphicsApi::Vulkan),
            _ => Err("gpu.graphicsApi must be gles or vulkan"),
        }
    }
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let table = text
            .parse::<toml::Table>()
            .map_err(ConfigError::Parse)?;
        Ok(HostConfig { table })
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    /// Full document, including unknown sections (forward compatible).
    pub fn table(&self) -> &toml::Table {
        &self.table
    }

    /// Physical output and guest render geometry; explicit malformed policies are rejected.
    pub fn render_dimensions(&self) -> Result<rudroid_common::display_dimensions::RenderDimensions, &'static str> {
        let device = self.device();
        self.render_dimensions_for_output(device.display_width, device.display_height)
    }

    /// Resolve the original exact/percentage policy against the real drawable,
    /// which fullscreen SDL may grant at a different size from the request.
    pub fn render_dimensions_for_output(&self, width: i64, height: i64) -> Result<rudroid_common::display_dimensions::RenderDimensions, &'static str> {
        for key in ["gpu.renderWidth","gpu.renderHeight","gpu.renderScalePercent"] {
            if self.node(key).is_some_and(|value|convert::to_i64(value).is_none()) {
                return Err("gpu render dimensions/percentage must be integers");
            }
        }
        if self.node("gpu.renderScaleSharp").is_some_and(|value|convert::to_bool(value).is_none()) {
            return Err("gpu.renderScaleSharp must be a boolean or integer");
        }
        // Preserve the GLES filter when switching APIs. Vulkan has its own
        // explicit linear-only policy; it never implements the GLES sharp pass.
        let sharp = if self.graphics_api()? == GraphicsApi::Vulkan {
            match self.node("gpu.vulkanRenderFilter") {
                None => {},
                Some(toml::Value::String(value)) if value == "linear" => {},
                Some(_) => return Err("gpu.vulkanRenderFilter must be linear; Vulkan sharp scaling is unsupported"),
            }
            false
        } else { self.get_bool("gpu.renderScaleSharp", true) };
        if self.presentation_backend()? == PresentationBackend::WaylandNative {
            if self.get_i64("gpu.renderWidth", 0) <= 0 || self.get_i64("gpu.renderHeight", 0) <= 0 {
                return Err("wayland_native requires explicit gpu.renderWidth/renderHeight; percentages alone cannot select a native buffer");
            }
            if sharp {
                return Err("wayland_native requires gpu.renderScaleSharp=false; compositor scaling does not implement the sharp filter");
            }
        }
        rudroid_common::display_dimensions::RenderDimensions::resolve(
            width, height,
            self.get_i64("gpu.renderWidth", 0), self.get_i64("gpu.renderHeight", 0),
            self.get_i64("gpu.renderScalePercent", 100), sharp)
    }

    /// Cold backend selection; never implicitly changes an existing filter or render size.
    pub fn presentation_backend(&self) -> Result<PresentationBackend, &'static str> {
        match self.node("gpu.presentationBackend") {
            None => Ok(PresentationBackend::Internal),
            Some(toml::Value::String(value)) if value == "internal" => Ok(PresentationBackend::Internal),
            Some(toml::Value::String(value)) if value == "wayland_native" => Ok(PresentationBackend::WaylandNative),
            Some(_) => Err("gpu.presentationBackend must be internal or wayland_native"),
        }
    }

    /// Resolve once before capability execution; malformed explicit memory limits fail visibly.
    pub fn memory_policy(&self) -> Result<MemoryPolicy, &'static str> {
        let mode = match self.node("runtime.memory_mode") {
            None => MemoryMode::Balanced,
            Some(toml::Value::String(value)) => MemoryMode::parse(value)?,
            Some(_) => return Err("runtime.memory_mode must be a string"),
        };
        let mut policy = MemoryPolicy::for_mode(mode);
        for (key, value, maximum) in [
            ("runtime.stream_buffer_bytes", &mut policy.stream_buffer_bytes, 1024 * 1024),
            ("runtime.stream_buffer_budget_mib", &mut policy.stream_buffer_budget_mib, 1024),
        ] {
            if let Some(node) = self.node(key) {
                let Some(number) = node.as_integer().filter(|n| (0..=maximum).contains(n)) else {
                    return Err("runtime stream buffer limits must be integers: bytes 0..1048576, budget_mib 0..1024");
                };
                *value = number as usize;
            }
        }
        if let Some(value) = self.node("runtime.preload_audio") {
            policy.preload_audio = convert::to_bool(value)
                .ok_or("runtime.preload_audio must be a boolean or integer")?;
        }
        Ok(policy)
    }

    /// Optional, title-validated opt-in. Existing configs retain engine defaults.
    pub fn unity_startup_policy(&self) -> Result<UnityStartupPolicy, &'static str> {
        let memory = self.memory_policy()?;
        let render_thread = match self.node("unity.render_thread_mode") {
            None => UnityRenderThread::Engine,
            Some(toml::Value::String(value)) => match value.as_str() {
                "engine" => UnityRenderThread::Engine,
                "single" => UnityRenderThread::Single,
                "multi" => UnityRenderThread::Multi,
                "memory_mode" => match memory.mode {
                    MemoryMode::LowMemory => UnityRenderThread::Single,
                    MemoryMode::Balanced => UnityRenderThread::Engine,
                    MemoryMode::Performance => UnityRenderThread::Multi,
                },
                _ => return Err("unity.render_thread_mode must be engine, single, multi or memory_mode"),
            },
            Some(_) => return Err("unity.render_thread_mode must be a string"),
        };
        if render_thread == UnityRenderThread::Engine { return Ok(UnityStartupPolicy::default()); }
        let args = match self.node("package.mainIntentBundle.unity") {
            None => "",
            Some(toml::Value::String(value)) => value,
            Some(_) => return Err("selected Unity render-thread policy requires string player arguments"),
        };
        UnityStartupPolicy::new(render_thread, args)
    }

    /// Overwrite one leaf value behind a dotted key. Returns `false` when an
    /// intermediate segment is missing or not a table (parity with the
    /// reference `config_set_string`, which only walks tables — array
    /// segments like `il2cpp_patch.0.class` are an explicit failure) and
    /// never creates or litters intermediate nodes on the way down.
    pub fn set_string(&mut self, dotted_key: &str, value: &str) -> bool {
        let mut segments: Vec<&str> = dotted_key.split('.').collect();
        let Some(leaf) = segments.pop() else {
            return false;
        };
        if leaf.is_empty() {
            // A trailing dot leaves no writable leaf (reference: the parent
            // walk ends on the empty segment and fails).
            return false;
        }
        let mut table = &mut self.table;
        for part in segments {
            match table.get_mut(part) {
                Some(toml::Value::Table(inner)) => table = inner,
                _ => return false,
            }
        }
        table.insert(
            leaf.to_owned(),
            toml::Value::String(value.to_owned()),
        );
        true
    }

    // ---- typed views over the well-known sections ----

    pub fn paths(&self) -> Paths {
        let section = self.table.get("paths").and_then(|v| v.as_table());
        let string = |key: &str| {
            section
                .and_then(|s| s.get(key))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let array = |key: &str| -> Vec<String> {
            section
                .and_then(|s| s.get(key))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Paths {
            game_files: string("game_files"),
            support_files: string("support_files"),
            android_package_code: string("android_package_code"),
            android_external_files: string("android_external_files"),
            android_files: string("android_files"),
            android_cache: string("android_cache"),
            android_obb_dirs: array("android_obb_dirs"),
            android_source_dirs: array("android_source_dirs"),
        }
    }

    pub fn package(&self) -> Package {
        let section = self.table.get("package").and_then(|v| v.as_table());
        let string = |key: &str| {
            section
                .and_then(|s| s.get(key))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        Package {
            package_name: string("packageName"),
            version_name: string("versionName"),
        }
    }

    /// Reference `config["device"][key].value_or<int32_t>(fallback)`: the
    /// permissive toml++ conversions (an exact float such as `1280.0`, a
    /// bool as 0/1) within the `int32_t` range; anything else keeps the
    /// default.
    pub fn device(&self) -> Device {
        let section = self.table.get("device").and_then(|v| v.as_table());
        let int = |key: &str, fallback: i64| {
            section
                .and_then(|s| s.get(key))
                .and_then(convert::to_i32)
                .map_or(fallback, i64::from)
        };
        Device {
            display_width: int("displayWidth", 640),
            display_height: int("displayHeight", 480),
            display_rotation: int("displayRotation", 0),
        }
    }

    /// `[plugins].enabled` allowlist. `None` means "no allowlist configured":
    /// the loader then loads every capability found in the deploy dir, which
    /// matches the reference behavior (`PLUGIN_ARCHITECTURE.md`, load order).
    pub fn enabled_plugins(&self) -> Option<Vec<String>> {
        self.table
            .get("plugins")
            .and_then(|v| v.as_table())
            .and_then(|s| s.get("enabled"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(str::to_owned)
                    .collect()
            })
    }

    // ---- dotted-key getters (RudroidPluginApi config_* parity) ----

    /// The reference `config_node` walk. An empty key misses, and a trailing
    /// dot ends the walk on the node before it (`"a.b."` reads `a.b`): the
    /// C++ lexer stops once nothing follows the last dot.
    fn node(&self, dotted_key: &str) -> Option<&toml::Value> {
        if dotted_key.is_empty() {
            return None;
        }
        let walked = dotted_key.strip_suffix('.').unwrap_or(dotted_key);
        let mut parts = walked.split('.');
        let mut node = self.table.get(parts.next()?)?;
        for part in parts {
            node = match node {
                toml::Value::Table(table) => table.get(part)?,
                // Numeric segments index into arrays of tables — how the
                // reference reaches `[[il2cpp_patch]]` entries
                // (`il2cpp_patch.<index>.<field>`).
                toml::Value::Array(array) => {
                    let index = array_segment_index(part)?;
                    array.get(index)?
                }
                _ => return None,
            };
        }
        Some(node)
    }

    pub fn get_bool(&self, dotted_key: &str, fallback: bool) -> bool {
        match self.node(dotted_key) {
            Some(v) => convert::to_bool(v).unwrap_or(fallback),
            None => fallback,
        }
    }

    /// `None` when the key carries NO usable boolean value — missing, or
    /// present with a non-boolean, non-integer value (the exact condition
    /// under which `get_bool` falls through to its fallback, ticket 87
    /// Phase C). This is the reader shape the shared `override_setting`
    /// primitive consumes for typed legacy/new key chains.
    pub fn get_bool_opt(&self, dotted_key: &str) -> Option<bool> {
        self.node(dotted_key).and_then(convert::to_bool)
    }

    pub fn get_i64(&self, dotted_key: &str, fallback: i64) -> i64 {
        match self.node(dotted_key) {
            Some(v) => convert::to_i64(v).unwrap_or(fallback),
            None => fallback,
        }
    }

    pub fn get_f64(&self, dotted_key: &str, fallback: f64) -> f64 {
        match self.node(dotted_key) {
            Some(v) => convert::to_f64(v).unwrap_or(fallback),
            None => fallback,
        }
    }

    /// `Some` only for a present STRING value — the reference getter returns
    /// 1 only when `value<std::string>()` has a value. A missing key or a
    /// present non-string value is `None`; callers apply `fallback` (kept in
    /// the signature for the C ABI shape).
    pub fn get_string(&self, dotted_key: &str, _fallback: &str) -> Option<String> {
        self.node(dotted_key)
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    pub fn array_size(&self, dotted_key: &str) -> usize {
        self.node(dotted_key)
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
