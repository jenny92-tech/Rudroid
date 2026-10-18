//! Loader test fixture (ticket 55): a platform-kind capability whose early
//! init registers the reference SDL audio policy through the Host API v0
//! table — the ticket-45 dynamic registration model, exactly like the C++
//! `platform_audio_sdl` plugin ("sdl-adaptive": 4 default attempts, 5 named
//! attempts, 250 ms retry, enumerate + prefer-non-HDMI + reject-silent all
//! set). Deployed as `libplatform_audio_sdl.so` it proves the audio policy
//! crosses the C ABI from a provider library into the shared registry; the
//! sdl_runtime capability then consumes the values for its sink experiments.
//! This is a test fixture, not an audio backend: it opens no device.

use std::ffi::c_int;

use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::HostStr;

rudroid_common::declare_descriptor!(
    "platform_audio_sdl",
    rudroid_common::Kind::Platform
);

/// Reference `platform_audio_sdl` policy values (read-only oracle).
struct Policy {
    id: &'static str,
    default_open_attempts: u32,
    named_open_attempts: u32,
    retry_delay_ms: u32,
    enumerate_outputs: i32,
    prefer_non_hdmi: i32,
    reject_silent_drivers: i32,
}

const POLICY: Policy = Policy {
    id: "sdl-adaptive",
    default_open_attempts: 4,
    named_open_attempts: 5,
    retry_delay_ms: 250,
    enumerate_outputs: 1,
    prefer_non_hdmi: 1,
    reject_silent_drivers: 1,
};

#[no_mangle]
extern "C" fn rudroid_plugin_early_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != rudroid_common::host_api::HOST_API_VERSION {
        return -2;
    }
    // The registry copies the POD but keeps the id: process-lifetime string.
    let policy = rudroid_common::registry::RudroidAudioPolicy {
        id: HostStr::new(rudroid_common::descriptor::leak_cstr(POLICY.id)),
        default_open_attempts: POLICY.default_open_attempts,
        named_open_attempts: POLICY.named_open_attempts,
        retry_delay_ms: POLICY.retry_delay_ms,
        enumerate_outputs: POLICY.enumerate_outputs,
        prefer_non_hdmi: POLICY.prefer_non_hdmi,
        reject_silent_drivers: POLICY.reject_silent_drivers,
    };
    let registered = (api.register_audio_policy)(Some(&policy));
    let tag = c"audio_policy";
    if registered != 0 {
        // First-write-wins: a second provider in the same process is
        // rejected explicitly and this fixture reports the phase failure
        // (deploy only one audio policy provider per kit).
        api.log_line(
            tag,
            &format!(
                "policy registration rejected with code {registered} \
                 (an audio policy is already registered)"
            ),
        );
        return -3;
    }
    api.log_line(
        tag,
        &format!(
            "policy registered: id={} default_open_attempts={} named_open_attempts={} \
             retry_delay_ms={}",
            POLICY.id, POLICY.default_open_attempts, POLICY.named_open_attempts,
            POLICY.retry_delay_ms
        ),
    );
    0
}
