//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `window_config_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
// Config paths are display-independent and always exercised (both
// matrices); the session itself is verified under Xvfb (window::
// es_session_tests, `sdl` builds only).
use super::*;

#[cfg(not(feature = "sdl"))]
#[test]
#[ignore = "built without the `sdl` feature: the real ES session test runs under `docker-rust.sh test --sdl` (Xvfb)"]
fn requested_es_context_is_actually_granted_es_and_drawing_verifies() {}

#[test]
fn window_config_reads_host_config() {
    let config = HostConfig::parse(
        "[device]\ndisplayWidth = 960\ndisplayHeight = 720\ndisplayRotation = 0\n",
    )
    .unwrap();
    assert_eq!(
        WindowConfig::from_config(&config),
        WindowConfig {
            width: 960,
            height: 720,
            rotation: 0
        }
    );
}

#[test]
fn gles_version_uses_gpu_overrides() {
    let config =
        HostConfig::parse("[gpu]\nglMajorVersionOverride = 3\nglMinorVersionOverride = 2\n")
            .unwrap();
    assert_eq!(WindowConfig::gles_version(&config), (3, 2));
    let default_config = HostConfig::parse("").unwrap();
    assert_eq!(WindowConfig::gles_version(&default_config), (3, 2));
}

fn last_error() -> String {
    // SAFETY: two-phase contract with a buffer sized by the first call.
    unsafe {
        let need = c_api::rudroid_sdl_last_error(std::ptr::null_mut(), 0);
        let mut buffer = vec![0 as std::ffi::c_char; need];
        c_api::rudroid_sdl_last_error(buffer.as_mut_ptr(), need);
        std::ffi::CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned()
    }
}

#[test]
fn c_api_handles_resolve_through_tables() {
    use rudroid_common::registry::HostStr;
    use c_api::*;
    use std::ffi::c_void;
    let pump = rudroid_sdl_audio_open(48_000, 2, 4, 250, 0, HostStr::null());
    assert!(!pump.is_null(), "dummy sink opens");
    let samples = [0.25f32; 4];
    // SAFETY: four readable samples.
    assert_eq!(unsafe { rudroid_sdl_audio_write(pump, samples.as_ptr(), 4) }, 2);
    assert_eq!((rudroid_sdl_audio_frames(pump), rudroid_sdl_audio_calls(pump)), (2, 1));
    rudroid_sdl_audio_destroy(pump);
    // Released, foreign and null handles take the old null-pointer path.
    for handle in [pump, 0xdead as *mut c_void, std::ptr::null_mut()] {
        // SAFETY: four readable samples.
        assert_eq!(unsafe { rudroid_sdl_audio_write(handle, samples.as_ptr(), 4) }, 0);
        assert_eq!(last_error(), "audio_write requires an open pump");
        assert_eq!((rudroid_sdl_audio_frames(handle), rudroid_sdl_audio_calls(handle)), (0, 0));
        rudroid_sdl_audio_destroy(handle);
    }
    let foreign = 0xdead as *mut c_void;
    // SAFETY: no callback is passed; an unknown session is rejected.
    assert_eq!(unsafe { rudroid_sdl_guest_graphics_activate(foreign, None) }, -1);
    #[cfg(all(feature = "sdl", target_os = "linux"))]
    assert_eq!(last_error(), "guest graphics requires a live SDL session");
    unsafe { rudroid_sdl_session_destroy(foreign) }; // Unknown token owns no native resources.
    let mut stats = std::mem::MaybeUninit::<CRudroidAudioStats>::zeroed();
    // SAFETY: all-zero is a valid stats POD.
    let stats = unsafe { stats.assume_init_mut() };
    assert_eq!(rudroid_sdl_audio_device_stats(foreign, Some(stats)), -1);
    assert_eq!(rudroid_sdl_audio_device_release(foreign), -1);
    #[cfg(feature = "sdl")]
    assert_eq!(last_error(), "audio_close_final requires a bridge");
}

#[test]
fn version_ladder_falls_back_like_the_reference_policy() {
    // Default request 3.2: the full reference ladder (platform_egl_sdl).
    let config = HostConfig::parse("").unwrap();
    assert_eq!(
        WindowConfig::gles_version_ladder(&config),
        vec![(3, 2), (3, 1), (3, 0), (2, 0)]
    );
    // A lower request never steps above itself.
    let config =
        HostConfig::parse("[gpu]\nglMajorVersionOverride = 3\nglMinorVersionOverride = 0\n")
            .unwrap();
    assert_eq!(
        WindowConfig::gles_version_ladder(&config),
        vec![(3, 0), (2, 0)]
    );
    let config = HostConfig::parse("[gpu]\nglMajorVersionOverride = 2\n").unwrap();
    assert_eq!(
        WindowConfig::gles_version_ladder(&config),
        vec![(2, 2), (2, 0)]
    );
}

#[cfg(feature = "sdl")]
#[test]
fn trigger_key_synthesis_can_be_disabled_without_changing_other_controls() {
    use rudroid_common::{registry::HostStr, host_api::HostApiV0};
    use crate::input_policy::InputPolicy;
    extern "C" fn boolean(key: HostStr<'_>, default: i32) -> i32 {
        if key.to_cstr() == Some(c"input.synthesize_trigger_keys") { 0 } else { default }
    }
    extern "C" fn integer(_: HostStr<'_>, default: i64) -> i64 { default }
    extern "C" fn missing(_: HostStr<'_>, _: HostStr<'_>, _: *mut std::ffi::c_char, _: u32) -> i32 { 0 }
    let mut api = HostApiV0::stubbed();
    api.config_get_bool = boolean;
    api.config_get_i64 = integer;
    // SAFETY: an absent-value callback never reads or writes the output pointer.
    api.config_get_string = unsafe { rudroid_common::host_api::ConfigGetStringFn::new(missing) };
    let policy = InputPolicy::read(&api, |_| Err("no key remap was supplied".into())).unwrap();
    let defaults = InputPolicy::default();
    assert_eq!(&policy.axes[..8], &defaults.axes[..8]);
    assert_eq!(&policy.axes[8..], &[None, None]);
    assert_eq!(policy.buttons, defaults.buttons);
    assert_eq!(policy.android_xbox360_identity, defaults.android_xbox360_identity);
}
