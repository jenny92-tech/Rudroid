//! INPUT:  resolved host memory mode, buffer limits and frame duration
//! OUTPUT:  MemoryMode, MemoryPolicy and remaining_frame_time
//! POS:    pure policy values shared by the loader and capabilities; owns no runtime state
#![forbid(unsafe_code)]

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMode {
    LowMemory,
    Balanced,
    Performance,
}

impl MemoryMode {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "low_memory" => Ok(Self::LowMemory),
            "balanced" => Ok(Self::Balanced),
            "performance" => Ok(Self::Performance),
            _ => Err("runtime.memory_mode must be low_memory, balanced or performance"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LowMemory => "low_memory",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }
}

/// Only optional JNI stream buffers are budgeted here, not Unity's heap,
/// graphics resources, required JNI metadata or the OS page cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPolicy {
    pub mode: MemoryMode,
    pub stream_buffer_bytes: usize,
    pub stream_buffer_budget_mib: usize,
    /// A default for title plugins that can control their audio preloader.
    pub preload_audio: bool,
}

impl MemoryPolicy {
    pub fn for_mode(mode: MemoryMode) -> Self {
        let (stream_buffer_bytes, stream_buffer_budget_mib) = match mode {
            MemoryMode::LowMemory => (0, 0),
            MemoryMode::Balanced => (8 * 1024, 1),
            MemoryMode::Performance => (64 * 1024, 16),
        };
        Self {
            mode,
            stream_buffer_bytes,
            stream_buffer_budget_mib,
            preload_audio: mode != MemoryMode::LowMemory,
        }
    }
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self::for_mode(MemoryMode::Balanced)
    }
}

pub const DEFAULT_TARGET_FPS: u32 = 90;

pub fn resolve_target_fps(value: i64) -> u32 {
    match value {
        0..=240 => value as u32,
        _ => DEFAULT_TARGET_FPS,
    }
}

/// Cap includes input, render and any driver/vsync waiting. A slow frame
/// gets no extra delay; no catch-up bursts or accumulated sleep debt.
pub fn remaining_frame_time(target_fps: u32, elapsed: Duration) -> Duration {
    if target_fps == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos(1_000_000_000_u64.div_ceil(u64::from(target_fps))).saturating_sub(elapsed)
}

#[cfg(test)]
#[path = "../tests/unit/runtime_policy.rs"]
mod tests;
