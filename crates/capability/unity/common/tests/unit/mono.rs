//! INPUT:  Host Config Mono sections and prepared package path fixtures
//! OUTPUT:  opt-in absence, pacing rejection and plugins.map containment checks
//! POS:    keeps Mono config tests beside mono.rs; declared there with `#[path]`
use super::*;
use rudroid_host_config::HostConfig;

#[test]
fn mono_is_opt_in_and_rejects_invalid_pacing_before_loading_files() {
    assert!(mono_config(&HostConfig::parse("").unwrap()).unwrap().is_none());
    for (settings, expected) in [
        ("typo = true", "unknown field"),
        ("frame_pacing = 'other'", "frame_pacing"),
        ("frame_pacing = 'desktop'\ndisplay_refresh_hz = 361", "24..=360"),
        ("frame_pacing = 'android'\ndisplay_refresh_hz = 60", "only used with desktop"),
    ] {
        let config = HostConfig::parse(&format!("[unity.mono]\n{settings}")).unwrap();
        assert!(mono_config(&config).unwrap_err().contains(expected));
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
#[test]
fn mono_implicit_plugin_map_must_stay_beside_the_prepared_manifest() {
    use std::{fs, os::unix::fs::symlink, path::PathBuf};
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    let dir = std::env::temp_dir().join(format!("rudroid-mono-config-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let temp = Temp(dir);
    let root = &temp.0;
    for dir in ["package", "lib/arm64-v8a", "managed"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    for name in [
        "package/base.apk",
        "managed/mscorlib.dll",
        "outside.map",
        "lib/arm64-v8a/libmain.so",
        "lib/arm64-v8a/libunity.so",
        "lib/arm64-v8a/libil2cpp.so",
        "lib/arm64-v8a/libmonobdwgc-2.0.so",
    ] {
        fs::write(root.join(name), []).unwrap();
    }
    fs::write(
        root.join("package/manifest.json"),
        r#"{"package":"test.mono","version_code":1,"unity_version":"6000.0.1f1","abi":"arm64-v8a","scripting":"mono","apk":"base.apk"}"#,
    )
    .unwrap();
    let config = HostConfig::parse(&format!(
        r#"
[paths]
game_files = '{p}'
android_package_code = '{p}/package/base.apk'
[package]
packageName = 'test.mono'
versionCode = 1
[unity.mono]
unity_version = '6000.0.1f1'
manifest = '{p}/package/manifest.json'
bridge = '{p}/lib/arm64-v8a/libil2cpp.so'
runtime = '{p}/lib/arm64-v8a/libmonobdwgc-2.0.so'
managed_dir = '{p}/managed'
"#,
        p = root.display()
    ))
    .unwrap();
    assert!(mono_config(&config).unwrap().unwrap().plugins_map.is_none());
    let map = root.join("package/plugins.map");
    symlink("../outside.map", &map).unwrap();
    assert!(mono_config(&config)
        .unwrap_err()
        .contains("plugins.map escapes"));
    fs::remove_file(&map).unwrap();
    fs::write(&map, []).unwrap();
    assert_eq!(
        mono_config(&config).unwrap().unwrap().plugins_map,
        Some(map.canonicalize().unwrap())
    );
}

#[test]
fn mono_config_from_api_matches_host_config_on_unknown_and_partial_sections() {
    use rudroid_common::host_api::{
        HostApiV0, HostConfigTomlV1, HOST_CONFIG_TOML_SERVICE_ABI, HOST_CONFIG_TOML_SERVICE_NAME,
    };
    use rudroid_common::registry::HostStr;
    use std::ffi::{c_int, c_void, CString};
    use std::sync::Mutex;

    static ACTIVE_TOML: Mutex<Option<&'static std::ffi::CStr>> = Mutex::new(None);
    static HAS_IL2CPP_PATCH: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    extern "C" fn toml_utf8() -> HostStr<'static> {
        ACTIVE_TOML
            .lock()
            .unwrap()
            .map(HostStr::new)
            .unwrap_or_else(HostStr::null)
    }

    static TOML_SVC: HostConfigTomlV1 = HostConfigTomlV1 {
        abi_version: HOST_CONFIG_TOML_SERVICE_ABI,
        toml_utf8,
    };

    extern "C" fn query_service(name: HostStr<'_>, abi: u32) -> *const c_void {
        if name.to_cstr() == Some(HOST_CONFIG_TOML_SERVICE_NAME)
            && abi == HOST_CONFIG_TOML_SERVICE_ABI
        {
            std::ptr::from_ref(&TOML_SVC).cast()
        } else {
            std::ptr::null()
        }
    }

    extern "C" fn has_capability(id: HostStr<'_>) -> c_int {
        c_int::from(
            id.to_cstr() == Some(c"il2cpp_patch")
                && HAS_IL2CPP_PATCH.load(std::sync::atomic::Ordering::Acquire),
        )
    }

    let api = HostApiV0 {
        query_service,
        has_capability,
        ..HostApiV0::stubbed()
    };

    // Absent config: neither section probe nor parser activates Mono.
    *ACTIVE_TOML.lock().unwrap() = None;
    assert!(!mono_section_present_in_api(&api));
    assert!(mono_config_from_api(&api).unwrap().is_none());

    // Empty config (no [unity.mono]): stays inactive.
    *ACTIVE_TOML.lock().unwrap() = Some(c"[paths]\ngame_files = '/tmp'\n");
    assert!(!mono_section_present_in_api(&api));
    assert!(mono_config_from_api(&api).unwrap().is_none());

    // Parity on bad/partial sections: unknown keys, empty abi, non-desktop hz=0,
    // runtime-only partial table, and non-table unity.mono must all be detected
    // by mono_section_present_in_api and rejected identically by mono_config_from_api.
    for (toml, expected) in [
        ("[unity.mono]\ntypo = true\n", "unknown field"),
        ("[unity.mono]\nruntime = '/tmp/libmonobdwgc-2.0.so'\n", "requires paths.android_package_code"),
        (
            "[paths]\ngame_files = '/tmp'\nandroid_package_code = '/tmp/a.apk'\n[unity.mono]\nbridge = '/a'\nruntime = '/b'\nmanaged_dir = '/c'\nmanifest = '/d'\nunity_version = '6000.0.1f1'\nabi = ''\n",
            "abi must be a nonempty string",
        ),
        (
            "[unity.mono]\nframe_pacing = 'android'\ndisplay_refresh_hz = 0\n",
            "only used with desktop",
        ),
        ("unity = { mono = 42 }\n", "must be a table"),
    ] {
        let leaked: &'static std::ffi::CStr = Box::leak(CString::new(toml).unwrap().into_boxed_c_str());
        *ACTIVE_TOML.lock().unwrap() = Some(leaked);
        assert!(
            mono_section_present_in_api(&api),
            "section probe missed: {toml}"
        );
        let api_err = mono_config_from_api(&api).unwrap_err();
        let cfg_err = mono_config(&HostConfig::parse(toml).unwrap()).unwrap_err();
        assert_eq!(api_err, cfg_err, "dual-track drift for: {toml}");
        assert!(api_err.contains(expected), "{api_err}");
    }
}
