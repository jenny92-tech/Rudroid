//! INPUT:  selected Unity render-thread policy, preserved player arguments and bounded boot text
//! OUTPUT:  UnityRenderThread and validated UnityStartupPolicy projections
//! POS:    pure startup configuration; no native offsets, resource ownership or frame work
#![forbid(unsafe_code)]

pub const MAX_BOOT_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnityRenderThread {
    #[default]
    Engine,
    Single,
    Multi,
}

impl UnityRenderThread {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Single => "single",
            Self::Multi => "multi",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnityStartupPolicy {
    render_thread: UnityRenderThread,
    command_line: Option<String>,
}

impl UnityStartupPolicy {
    /// Unity 6000.0.61f1 ARM64, observed libunity SHA256
    /// ae2af629e1426b5cd8d64f93280c55f12d208b7856942f4d2d6ebe766c92b229.
    /// This is a compatibility allowlist for trusted game inputs, not authenticity verification.
    pub fn supports_native_player(
        &self,
        identity: &rudroid_common::elf_identity::ElfIdentity,
    ) -> bool {
        self.render_thread == UnityRenderThread::Engine
            || (identity.file_len == 23_298_160
                && identity.build_id.as_deref()
                    == Some(&[0x30, 0x9e, 0x28, 0x4d, 0x38, 0xa6, 0x8f, 0x71]))
    }
    pub(crate) fn new(
        render_thread: UnityRenderThread,
        original: &str,
    ) -> Result<Self, &'static str> {
        if render_thread == UnityRenderThread::Engine {
            return Ok(Self::default());
        }
        let flag = match render_thread {
            UnityRenderThread::Single => "-force-gfx-direct",
            UnityRenderThread::Multi => "-force-gfx-mt",
            UnityRenderThread::Engine => unreachable!(),
        };
        let mut present = false;
        for token in original
            .split_ascii_whitespace()
            .map(|s| s.trim_matches(['\'', '"']))
        {
            for known in [
                "-force-gfx-direct",
                "-force-gfx-st",
                "-force-gfx-mt",
                "-force-gfx-jobs",
            ] {
                if token == known
                    || token
                        .strip_prefix(known)
                        .is_some_and(|tail| tail.starts_with('='))
                {
                    if token != flag {
                        return Err("Unity render-thread policy conflicts with existing player graphics arguments");
                    }
                    present = true;
                }
            }
        }
        let mut command_line = original.to_owned();
        if !present {
            if !command_line.is_empty() {
                command_line.push(' ');
            }
            command_line.push_str(flag);
        }
        if command_line.len() >= 4096 || command_line.contains('\0') {
            return Err("resolved Unity player arguments must fit 4095 bytes without NUL");
        }
        Ok(Self {
            render_thread,
            command_line: Some(command_line),
        })
    }

    pub fn render_thread(&self) -> UnityRenderThread {
        self.render_thread
    }
    pub fn command_line(&self) -> Option<&str> {
        self.command_line.as_deref()
    }

    /// Rewrite only an existing, unambiguous three-key rendering contract.
    /// Missing/duplicate/unknown values reject rather than guessing a Unity build.
    pub fn boot_config(&self, source: &str) -> Result<Option<String>, &'static str> {
        if self.render_thread == UnityRenderThread::Engine {
            return Ok(None);
        }
        if source.len() > MAX_BOOT_CONFIG_BYTES || source.contains('\0') {
            return Err("Unity boot.config exceeds 64 KiB or contains NUL");
        }
        let keys = [
            "force-gfx-direct",
            "gfx-threading-mode",
            "gfx-disable-mt-rendering",
        ];
        let values = if self.render_thread == UnityRenderThread::Multi {
            ["0", "1", "0"]
        } else {
            ["1", "0", "1"]
        };
        let mut seen = [false; 3];
        let mut out = String::with_capacity(source.len());
        for line in source.split_inclusive('\n') {
            let (body, ending) = if let Some(body) = line.strip_suffix("\r\n") {
                (body, "\r\n")
            } else if let Some(body) = line.strip_suffix('\n') {
                (body, "\n")
            } else {
                (line, "")
            };
            let field = body.split_once('=').and_then(|(key, value)| {
                keys.iter()
                    .position(|k| *k == key.trim())
                    .map(|i| (i, value.trim()))
            });
            if let Some((i, value)) = field {
                if seen[i] || !matches!(value, "0" | "1") {
                    return Err("Unity boot.config has duplicate or unsupported rendering keys");
                }
                seen[i] = true;
                out.push_str(keys[i]);
                out.push('=');
                out.push_str(values[i]);
                out.push_str(ending);
            } else {
                // A bare known key is malformed, not an absent override.
                if keys.contains(&body.trim()) {
                    return Err("Unity boot.config has a malformed rendering key");
                }
                out.push_str(line);
            }
        }
        if !seen.into_iter().all(|v| v) {
            return Err(
                "Unity render-thread policy requires all three existing boot rendering keys",
            );
        }
        Ok(Some(out))
    }
}

fn render_thread_from_mode(
    mode: &str,
    memory: rudroid_common::runtime_policy::MemoryMode,
) -> Result<UnityRenderThread, &'static str> {
    match mode {
        "engine" => Ok(UnityRenderThread::Engine),
        "single" => Ok(UnityRenderThread::Single),
        "multi" => Ok(UnityRenderThread::Multi),
        "memory_mode" => Ok(match memory {
            rudroid_common::runtime_policy::MemoryMode::LowMemory => UnityRenderThread::Single,
            rudroid_common::runtime_policy::MemoryMode::Balanced => UnityRenderThread::Engine,
            rudroid_common::runtime_policy::MemoryMode::Performance => UnityRenderThread::Multi,
        }),
        _ => Err("unity.render_thread_mode must be engine, single, multi or memory_mode"),
    }
}

/// Optional, title-validated opt-in. Existing configs retain engine defaults.
pub fn unity_startup_policy(
    config: &rudroid_host_config::HostConfig,
) -> Result<UnityStartupPolicy, &'static str> {
    let memory = config.memory_policy()?;
    let render_thread = match config.node("unity.render_thread_mode") {
        None => UnityRenderThread::Engine,
        Some(toml::Value::String(value)) => render_thread_from_mode(value, memory.mode)?,
        Some(_) => return Err("unity.render_thread_mode must be a string"),
    };
    if render_thread == UnityRenderThread::Engine {
        return Ok(UnityStartupPolicy::default());
    }
    let args = match config.node("package.mainIntentBundle.unity") {
        None => "",
        Some(toml::Value::String(value)) => value,
        Some(_) => {
            return Err("selected Unity render-thread policy requires string player arguments")
        }
    };
    UnityStartupPolicy::new(render_thread, args)
}

/// Resolve the exact same policy from the authoritative Host Config TOML
/// exposed through `rudroid.host.config_toml.v1`, preserving existing
/// `[package.mainIntentBundle].unity` arguments and conflict detection.
pub fn unity_startup_policy_from_api(
    api: &rudroid_common::host_api::HostApiV0,
) -> Result<UnityStartupPolicy, &'static str> {
    let Some(toml) = api.config_toml() else {
        return Ok(UnityStartupPolicy::default());
    };
    let config = rudroid_host_config::HostConfig::parse(toml)
        .map_err(|_| "invalid Host Config TOML from host service")?;
    unity_startup_policy(&config)
}

#[cfg(test)]
#[path = "../tests/unit/unity_startup.rs"]
mod tests;
