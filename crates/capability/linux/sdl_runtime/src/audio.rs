//! INPUT:  reference `RudroidAudioPolicy` bounds and guest-produced PCM frames
//! OUTPUT:  `AudioPolicyParams`, `AudioError` and the device-independent `AudioPump`
//! POS:    audio policy validation and pumping that runs without an audio device
#![forbid(unsafe_code)]

/// Real-device output selection; copied from the loader registry before guest preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputPolicy {
    pub default_attempts: u32,
    pub named_attempts: u32,
    pub retry_ms: u32,
    pub enumerate: bool,
    pub prefer_non_hdmi: bool,
    pub reject_silent: bool,
}
impl Default for OutputPolicy {
    fn default() -> Self {
        Self {
            default_attempts: 4,
            named_attempts: 5,
            retry_ms: 250,
            enumerate: true,
            prefer_non_hdmi: true,
            reject_silent: true,
        }
    }
}
static OUTPUT_POLICY: std::sync::OnceLock<OutputPolicy> = std::sync::OnceLock::new();
pub fn output_policy() -> OutputPolicy {
    OUTPUT_POLICY.get().copied().unwrap_or_default()
}
pub fn configure_output_policy(
    policy: Option<&rudroid_common::registry::RudroidAudioPolicy>,
) -> Result<(), String> {
    let p = match policy {
        Some(p) => {
            if !(1..=100).contains(&p.default_open_attempts)
                || !(1..=100).contains(&p.named_open_attempts)
                || p.retry_delay_ms > 10000
            {
                return Err("invalid audio output policy".into());
            }
            OutputPolicy {
                default_attempts: p.default_open_attempts,
                named_attempts: p.named_open_attempts,
                retry_ms: p.retry_delay_ms,
                enumerate: p.enumerate_outputs != 0,
                prefer_non_hdmi: p.prefer_non_hdmi != 0,
                reject_silent: p.reject_silent_drivers != 0,
            }
        }
        None => OutputPolicy::default(),
    };
    publish_output_policy(&OUTPUT_POLICY, p)
}
fn publish_output_policy(
    slot: &std::sync::OnceLock<OutputPolicy>,
    policy: OutputPolicy,
) -> Result<(), String> {
    match slot.set(policy) {
        Ok(()) => Ok(()),
        Err(policy) if slot.get() == Some(&policy) => Ok(()),
        Err(_) => Err("audio output policy already configured with different values".into()),
    }
}
pub fn silent_driver(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "dummy" | "disk" | "nosound"
    )
}
pub fn order_output_names(names: &mut [std::ffi::CString], prefer_non_hdmi: bool) {
    if prefer_non_hdmi {
        names.sort_by_key(|n| n.to_string_lossy().to_ascii_lowercase().contains("hdmi"));
    }
}

/// Policy-consumer parameters mirroring the reference `RudroidAudioPolicy`
/// (platform/common/audio_policy.h) and the validation bounds of its
/// `bd_register_audio_policy` (1..=100 attempts, retry <= 10000 ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPolicyParams {
    pub sample_rate: u32,
    pub channels: u8,
    pub open_attempts: u32,
    pub retry_delay_ms: u32,
    pub reject_silent_drivers: u32,
}

#[derive(Debug)]
pub enum AudioError {
    /// Format values out of range (sample rate, channel count).
    InvalidSpec(String),
    /// Policy values violating the reference registration bounds.
    InvalidPolicy(String),
    /// `reject_silent_drivers` set but only the dummy (silent) sink is
    /// available — the policy explicitly forbids that stand-in.
    SilentDriverRejected,
    Io(std::io::Error),
    /// Buffer length is not a whole number of frames (samples % channels).
    PartialFrame {
        samples: usize,
        channels: u8,
    },
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::InvalidSpec(reason) => write!(f, "invalid audio spec: {reason}"),
            AudioError::InvalidPolicy(reason) => {
                write!(f, "audio policy invalid: {reason}")
            }
            AudioError::SilentDriverRejected => write!(
                f,
                "policy rejects silent drivers (`reject_silent_drivers`) but no file \
                     sink path was provided; the in-memory dummy sink is exactly the \
                     silent stand-in this policy forbids — pass a file sink or clear \
                     the flag"
            ),
            AudioError::Io(err) => write!(f, "file audio sink failed: {err}"),
            AudioError::PartialFrame { samples, channels } => write!(
                f,
                "{samples} samples is not a whole number of frames at \
                     channels={channels}; buffers are sample counts, frames = \
                     samples / channels"
            ),
        }
    }
}

impl std::error::Error for AudioError {}

enum AudioSink {
    /// In-memory counting sink (the documented silent stand-in, the
    /// analogue of the reference's `dummy`/`disk` silent drivers).
    Dummy,
    /// Raw interleaved f32 LE samples appended to a file.
    File(std::fs::File),
}

/// Frame-accurate pump under [`AudioPolicyParams`]: write calls are
/// counted individually, buffers are SAMPLE counts, and frames are the
/// accounting unit (frames = samples / channels).
pub struct AudioPump {
    params: AudioPolicyParams,
    sink: AudioSink,
    frames_written: u64,
    write_calls: u64,
}

impl std::fmt::Debug for AudioPump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPump")
            .field("params", &self.params)
            .field(
                "sink",
                &match self.sink {
                    AudioSink::Dummy => "dummy",
                    AudioSink::File(_) => "file",
                },
            )
            .field("frames_written", &self.frames_written)
            .field("write_calls", &self.write_calls)
            .finish()
    }
}

impl AudioPump {
    /// Open a sink under the registered policy values. Validation mirrors
    /// the reference: format bounds, policy bounds, and the silent-driver
    /// rejection.
    pub fn open(
        params: AudioPolicyParams,
        file_path: Option<&std::path::Path>,
    ) -> Result<AudioPump, AudioError> {
        if params.sample_rate == 0 || params.sample_rate > 384_000 {
            return Err(AudioError::InvalidSpec(format!(
                "sample_rate {} out of range 1..=384000",
                params.sample_rate
            )));
        }
        if params.channels == 0 || params.channels > 8 {
            return Err(AudioError::InvalidSpec(format!(
                "channels {} out of range 1..=8",
                params.channels
            )));
        }
        if params.open_attempts == 0 || params.open_attempts > 100 {
            return Err(AudioError::InvalidPolicy(format!(
                "open_attempts {} out of the reference bounds 1..=100",
                params.open_attempts
            )));
        }
        if params.retry_delay_ms > 10_000 {
            return Err(AudioError::InvalidPolicy(format!(
                "retry_delay_ms {} exceeds the reference bound of 10000",
                params.retry_delay_ms
            )));
        }
        let sink = match file_path {
            Some(path) => AudioSink::File(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(AudioError::Io)?,
            ),
            None => {
                if params.reject_silent_drivers != 0 {
                    return Err(AudioError::SilentDriverRejected);
                }
                AudioSink::Dummy
            }
        };
        Ok(AudioPump {
            params,
            sink,
            frames_written: 0,
            write_calls: 0,
        })
    }

    /// Push interleaved samples through the policy into the sink. Returns
    /// the number of FRAMES written. An empty buffer is a no-op: no
    /// frames, no sink call. A partial frame is rejected explicitly — the
    /// sink never sees a fractional frame.
    pub fn write(&mut self, samples: &[f32]) -> Result<u64, AudioError> {
        if samples.is_empty() {
            return Ok(0);
        }
        if samples.len() % self.params.channels as usize != 0 {
            return Err(AudioError::PartialFrame {
                samples: samples.len(),
                channels: self.params.channels,
            });
        }
        if let AudioSink::File(file) = &mut self.sink {
            let mut bytes = Vec::with_capacity(samples.len() * 4);
            for sample in samples {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            use std::io::Write;
            file.write_all(&bytes).map_err(AudioError::Io)?;
        }
        let frames = (samples.len() / self.params.channels as usize) as u64;
        self.frames_written += frames;
        self.write_calls += 1;
        Ok(frames)
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Number of non-empty write calls that reached the sink.
    pub fn write_calls(&self) -> u64 {
        self.write_calls
    }

    pub fn params(&self) -> &AudioPolicyParams {
        &self.params
    }
}

#[cfg(test)]
#[path = "../tests/unit/audio.rs"]
mod tests;
