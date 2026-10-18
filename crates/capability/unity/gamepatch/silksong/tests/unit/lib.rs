//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  config/gate regressions and nested/unwinding viewport scope checks
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::registry::HostStr;
use std::ffi::c_char;

#[test]
fn memory_mode_controls_audio_preloader_default_but_title_override_wins() {
    let low = HostConfig::parse("[runtime]\nmemory_mode='low_memory'\n").unwrap();
    assert!(SilksongFeatures::from_config(&low).disable_audio_preloader);
    let fast = HostConfig::parse("[runtime]\nmemory_mode='performance'\n").unwrap();
    assert!(!SilksongFeatures::from_config(&fast).disable_audio_preloader);
    let explicit = HostConfig::parse("[runtime]\nmemory_mode='low_memory'\n[game_patches.silksong_unity6]\ndisable_audio_preloader=false\n").unwrap();
    assert!(!SilksongFeatures::from_config(&explicit).disable_audio_preloader);
}

fn device_config() -> HostConfig {
    // Keep the exact repository fixture while allowing the compiled native
    // test to run on a handheld without the Docker /work source tree.
    HostConfig::parse(include_str!(
        "../../../../../../../work/config.silksong.device.toml"
    ))
    .expect("in-repo Silksong device config")
}

#[test]
fn package_gate_matches_reference_target() {
    assert!(applies_to(&device_config()));
    let other =
        HostConfig::parse("[package]\npackageName = \"com.teamcherry.hollowknight\"\n").unwrap();
    assert!(!applies_to(&other));
    let no_package = HostConfig::parse("").unwrap();
    assert!(!applies_to(&no_package));
}

#[test]
fn device_config_flags_are_read() {
    let features = SilksongFeatures::from_config(&device_config());
    // Values from the checked-in Silksong device TOML.
    assert!(features.disable_touch_camera);
    assert!(features.disable_touch_controls);
    assert!(features.disable_watermark_canvas);
    assert!(features.destroy_bootstrap_objects);
    assert!(features.freeze_decorative_behaviours);
    assert_eq!(features.decorative_sprite_percent, 50);
    assert!(features.disable_ambient_particles);
    assert!(features.disable_heavy_postprocess);
    assert!(features.disable_audio_preloader);
    assert_eq!(features.assetbundle_unload_delay_frames, 180);
    assert!(!features.resource_census);
    assert!(!features.trace_audio_clip_records);
    // The device config does not set the new viewport key: the merged
    // reference patch stays presence-activated (default on).
    assert!(features.viewport_enable);
    // The reference ON-by-default launch group is untouched by the
    // device config.
    assert!(features.suppress_application_quit);
    assert!(features.skip_launcher_disclaimer);
}

#[test]
fn defaults_are_conservative() {
    let features = SilksongFeatures::from_config(&HostConfig::parse("").unwrap());
    assert!(!features.disable_touch_camera);
    assert_eq!(features.decorative_sprite_percent, 100);
    // Reference defaults: the launch/QoL group ON, the pruning group OFF.
    assert!(features.suppress_application_quit);
    assert!(features.skip_launcher_disclaimer);
    assert!(features.skip_apkvision_signature_check);
    assert!(features.load_main_menu_directly);
    assert!(features.skip_unsupported_cinematics);
    assert!(!features.disable_ambient_particles);
    assert!(!features.disable_heavy_postprocess);
    assert!(!features.freeze_decorative_behaviours);
    assert!(!features.disable_audio_preloader);
    assert_eq!(features.assetbundle_unload_delay_frames, 600);
    // Merged viewport patch: present = active.
    assert!(features.viewport_enable);
}

/// Reference parity: `decorative_sprite_percent` only accepts 25..=100;
/// anything else (out of window OR wrong type) keeps the 100 default.
/// (The pre-54 crate clamped 0..=100 — a divergence, fixed here.)
#[test]
fn decorative_sprite_percent_window_matches_reference() {
    let config = |value: String| {
        HostConfig::parse(&format!(
            "[game_patches.silksong_unity6]\ndecorative_sprite_percent = {value}\n"
        ))
        .unwrap()
    };
    assert_eq!(
        SilksongFeatures::from_config(&config("25".to_owned())).decorative_sprite_percent,
        25
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("100".to_owned())).decorative_sprite_percent,
        100
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("24".to_owned())).decorative_sprite_percent,
        100,
        "below the reference window keeps the default (no pruning)"
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("0".to_owned())).decorative_sprite_percent,
        100
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("101".to_owned())).decorative_sprite_percent,
        100
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("\"half\"".to_owned())).decorative_sprite_percent,
        100,
        "wrong-typed leaves keep the getter fallback"
    );
}

#[test]
fn assetbundle_delay_window_matches_reference() {
    let config = |value: String| {
        HostConfig::parse(&format!(
            "[game_patches.silksong_unity6]\nassetbundle_unload_delay_frames = {value}\n"
        ))
        .unwrap()
    };
    assert_eq!(
        SilksongFeatures::from_config(&config("60".to_owned())).assetbundle_unload_delay_frames,
        60
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("6000".to_owned())).assetbundle_unload_delay_frames,
        6000
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("10".to_owned())).assetbundle_unload_delay_frames,
        600
    );
    assert_eq!(
        SilksongFeatures::from_config(&config("6001".to_owned())).assetbundle_unload_delay_frames,
        600
    );
}

/// The reference OR-chain: disable_heavy_postprocess turns on the four
/// individual prune switches as well (light_blurred_background keeps its
/// own independent key, exactly as the reference builds it).
#[test]
fn heavy_postprocess_ors_into_the_individual_switches() {
    let config =
        HostConfig::parse("[game_patches.silksong_unity6]\ndisable_heavy_postprocess = true\n")
            .unwrap();
    let features = SilksongFeatures::from_config(&config);
    assert!(features.postprocess_pruning_enabled());
    assert!(features.disable_bloom);
    assert!(features.disable_camera_blur_plane);
    assert!(features.disable_light_blur);
    assert!(features.disable_uber_postprocess);
    assert!(!features.disable_light_blurred_background);
    let single =
        HostConfig::parse("[game_patches.silksong_unity6]\ndisable_bloom = true\n").unwrap();
    assert!(SilksongFeatures::from_config(&single).postprocess_pruning_enabled());
}

/// The behavior registry carries the full reference set with exact
/// triggers/targets/defaults (ticket 54 acceptance item 1).
#[test]
fn plan_registers_every_reference_behavior() {
    let features = SilksongFeatures::from_config(&device_config());
    let entries = plan(&features);
    let behaviors: Vec<&str> = entries.iter().map(|p| p.entry.behavior).collect();
    for expected in [
        "suppress_application_quit",
        "disable_ambient_particles",
        "disable_audio_preloader",
        "freeze_decorative_behaviours",
        "silksong_viewport (merged reference patch)",
        "skip_launcher_disclaimer + load_main_menu_directly",
        "skip_unsupported_cinematics",
        "skip_apkvision_signature_check",
        "touch-manager controls compatibility",
        "postprocess component pruning",
        "decorative sprite pruning",
        "bootstrap object teardown (touch camera/controls, watermark)",
        "resource census / method traces / audio record trace",
        "asset bundle unload",
        "unused-asset unload after prune",
    ] {
        assert!(behaviors.contains(&expected), "plan misses {expected}");
    }
    let quit = entries
        .iter()
        .find(|p| p.entry.behavior == "suppress_application_quit")
        .unwrap();
    assert!(quit.entry.reference_default);
    assert_eq!(quit.entry.trigger, Trigger::Il2cppPostInit);
    assert!(quit.entry.target.contains("Application.Quit"));
    let viewport = entries
        .iter()
        .find(|p| p.entry.behavior.starts_with("silksong_viewport"))
        .unwrap();
    assert!(viewport.entry.target.contains("AutoScaleViewport"));
    assert!(viewport.entry.target.contains("GetClampedBetween"));
    assert!(viewport.enabled, "device config keeps the viewport on");
    let particles = entries
        .iter()
        .find(|p| p.entry.behavior == "disable_ambient_particles")
        .unwrap();
    assert!(particles.enabled, "device config enables particles");
    // A conservative config disables the pruning group while the launch
    // group stays registered (reference defaults).
    let off = SilksongFeatures::from_config(&HostConfig::parse("").unwrap());
    let plan_off = plan(&off);
    let particles_off = plan_off
        .iter()
        .find(|p| p.entry.behavior == "disable_ambient_particles")
        .unwrap();
    assert!(!particles_off.enabled);
    let quit_off = plan_off
        .iter()
        .find(|p| p.entry.behavior == "suppress_application_quit")
        .unwrap();
    assert!(quit_off.enabled, "the quit hook installs unconditionally");
    for entry in &plan_off {
        if let Action::RegisteredOnly(reason) = entry.entry.action {
            assert!(!reason.is_empty(), "registered-only rows carry a reason");
        }
    }
}

/// Both consumption dialects agree: the in-process HostConfig read and
/// the Host-API-getter read (the C++ plugin's path) produce the same
/// feature table. The getters answer through a Mutex-backed HostConfig.
#[test]
fn host_api_read_matches_host_config_read() {
    use std::sync::Mutex;

    static BACKING: Mutex<Option<HostConfig>> = Mutex::new(None);

    fn backing() -> std::sync::MutexGuard<'static, Option<HostConfig>> {
        BACKING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    extern "C" fn get_bool(key: HostStr<'_>, fallback: c_int) -> c_int {
        let key = key
            .to_cstr()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let guard = backing();
        let config = guard.as_ref().expect("test config installed");
        c_int::from(config.get_bool(&key, fallback != 0))
    }
    extern "C" fn get_i64(key: HostStr<'_>, fallback: i64) -> i64 {
        let key = key
            .to_cstr()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let guard = backing();
        let config = guard.as_ref().expect("test config installed");
        config.get_i64(&key, fallback)
    }
    extern "C" fn get_f64(_key: HostStr<'_>, fallback: f64) -> f64 {
        fallback
    }
    unsafe extern "C" fn get_string(
        _key: HostStr<'_>,
        fallback: HostStr<'_>,
        out: *mut c_char,
        out_size: u32,
    ) -> c_int {
        let fallback = fallback.to_cstr().unwrap_or_default().to_bytes_with_nul();
        // SAFETY: the caller's buffer holds `out_size` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                fallback.as_ptr().cast(),
                out,
                fallback.len().min(out_size as usize),
            )
        };
        0
    }

    extern "C" fn log_fn(_tag: HostStr<'_>, _message: HostStr<'_>) {}
    extern "C" fn register_module_loaded(
        _kind: u32,
        _callback: Option<rudroid_common::registry::RudroidModuleLoadedCallback>,
        _user: *mut c_void,
    ) -> c_int {
        0
    }

    let api = HostApiV0 {
        config_get_bool: get_bool,
        config_get_i64: get_i64,
        config_get_f64: get_f64,
        // SAFETY: the static getter bounds writes, NUL-terminates successes and retains no borrows.
        config_get_string: unsafe {
            rudroid_common::host_api::ConfigGetStringFn::new(get_string)
        },
        log: log_fn,
        register_module_loaded,
        ..HostApiV0::stubbed()
    };
    *backing() = Some(device_config());
    let from_api = SilksongFeatures::from_host_api(&api);
    *backing() = None;
    assert_eq!(SilksongFeatures::from_config(&device_config()), from_api);
}

/// The hooks wait for `Il2cppReady`; without il2cpp_patch in the kit the
/// patch would arm silently and never install, so the loader must refuse it.
#[test]
fn descriptor_requires_il2cpp_patch() {
    // SAFETY: the descriptor is a process-lifetime static with C strings.
    let descriptor = unsafe { &*crate::rudroid_plugin_descriptor() };
    let required = unsafe { std::ffi::CStr::from_ptr(descriptor.required_plugins) };
    assert_eq!(required.to_str().unwrap(), "il2cpp_patch");
}

#[test]
fn viewport_scope_restores_the_outer_scope_on_return_and_unwind() {
    VIEWPORT_SCOPE.with(|scope| scope.set(Some((0.75, true))));
    let failure = std::panic::catch_unwind(|| {
        with_viewport_scope(640. / 480., || {
            VIEWPORT_SCOPE.with(|scope| assert_eq!(scope.get(), Some((640. / 480., false))));
            panic!("simulated managed exception");
        })
    });
    assert!(failure.is_err());
    VIEWPORT_SCOPE.with(|scope| assert_eq!(scope.get(), Some((0.75, true))));
    let value = with_viewport_scope(1.5, || {
        let nested = with_viewport_scope(2.0, || {
            VIEWPORT_SCOPE.with(|scope| assert_eq!(scope.get(), Some((2.0, false))));
            42
        });
        VIEWPORT_SCOPE.with(|scope| assert_eq!(scope.get(), Some((1.5, false))));
        nested
    });
    assert_eq!(value, 42);
    VIEWPORT_SCOPE.with(|scope| {
        assert_eq!(scope.get(), Some((0.75, true)));
        scope.set(None);
    });
}

#[test]
fn confirmed_menu_quit_scope_is_thread_local_nested_and_unwind_safe() {
    MENU_QUIT_SCOPE.with(|scope| assert!(!scope.get()));
    let failure = std::panic::catch_unwind(|| with_menu_quit_scope(|| {
        MENU_QUIT_SCOPE.with(|scope| assert!(scope.get()));
        std::thread::spawn(|| MENU_QUIT_SCOPE.with(|scope| assert!(!scope.get())))
            .join().unwrap();
        with_menu_quit_scope(|| MENU_QUIT_SCOPE.with(|scope| assert!(scope.get())));
        MENU_QUIT_SCOPE.with(|scope| assert!(scope.get()));
        panic!("simulated managed unwind from the original menu action");
    }));
    assert!(failure.is_err());
    MENU_QUIT_SCOPE.with(|scope| assert!(!scope.get()));
}
