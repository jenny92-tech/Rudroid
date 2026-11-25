//! INPUT:  fresh loader processes, three memory modes and immutable extracted boot fixtures
//! OUTPUT:  shared Java/NDK/native policy projection, retry and concurrent publication checks
//! POS:    verifies prepare-path overlay ownership without executing game code
use rudroid_common::android_assets::{BASE, MERGED, PATH};
use rudroid_host_config::HostConfig;
use rudroid_unity_common::{
    unity_startup_policy, unity_startup_policy_from_api, BootView, UnityRenderThread,
};
use rudroid_unity_loader::host_api;
use std::io::Write;
use std::{ffi::CString, path::PathBuf};

// Identity-only sparse fixture; contains no executable Unity code.
fn native_fixture(root: &std::path::Path, known: bool) {
    let p = root.join("lib/arm64-v8a");
    std::fs::create_dir_all(&p).unwrap();
    let mut b = vec![0; 152];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16..18].copy_from_slice(&3u16.to_le_bytes());
    b[18..20].copy_from_slice(&183u16.to_le_bytes());
    b[20..24].copy_from_slice(&1u32.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&1u16.to_le_bytes());
    b[64..68].copy_from_slice(&4u32.to_le_bytes());
    b[72..80].copy_from_slice(&128u64.to_le_bytes());
    b[96..104].copy_from_slice(&24u64.to_le_bytes());
    b[128..132].copy_from_slice(&4u32.to_le_bytes());
    b[132..136].copy_from_slice(&8u32.to_le_bytes());
    b[136..140].copy_from_slice(&3u32.to_le_bytes());
    b[140..144].copy_from_slice(b"GNU\0");
    if known {
        b[144..152].copy_from_slice(&[0x30, 0x9e, 0x28, 0x4d, 0x38, 0xa6, 0x8f, 0x71]);
    }
    let mut f = std::fs::File::create(p.join("libunity.so")).unwrap();
    f.write_all(&b).unwrap();
    f.set_len(23_298_160).unwrap();
}

const BOOT: &str =
    "force-gfx-direct=1\ngfx-threading-mode=0\ngfx-disable-mt-rendering=1\njob-worker-count=1\n";
fn config(root: &std::path::Path, mode: &str, rendering: &str) -> HostConfig {
    HostConfig::parse(&format!("[paths]\ngame_files={:?}\n[runtime]\nmemory_mode={mode:?}\n[unity]\nrender_thread_mode={rendering:?}\n[package.mainIntentBundle]\nunity='-force-gles30'\n",root.to_str().unwrap())).unwrap()
}

/// Mirror player_lifecycle prepare: resolve via `unity_startup_policy_from_api`,
/// register FILE_OVERLAY first, then publish command-line override.
fn publish_boot_overlay(
    api: &rudroid_common::host_api::HostApiV0,
    source: &std::path::Path,
) -> Result<Option<BootView>, &'static str> {
    let policy = unity_startup_policy_from_api(api)?;
    if policy.render_thread() == UnityRenderThread::Engine {
        return Ok(None);
    }
    let view = BootView::prepare(source, &policy)?;
    if !api.register_file_overlay(view.source_cstr(), view.replacement_cstr()) {
        return Err("overlay registration refused");
    }
    if let Some(cmd) = policy.command_line() {
        let value = CString::new(cmd.replace('\0', "")).map_err(|_| "command line NUL")?;
        if !api.set_config_string(c"package.mainIntentBundle.unity", &value) {
            return Err("set_config_string refused");
        }
    }
    Ok(Some(view))
}

#[test]
fn startup_modes_reach_all_resource_entries_and_publish_one_owner() {
    let Ok(case) = std::env::var("RUDROID_TEST_STARTUP_MODE") else {
        for case in [
            "low_memory",
            "balanced",
            "performance",
            "override",
            "race",
            "unknown-engine",
            "conflict",
        ] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "startup_modes_reach_all_resource_entries_and_publish_one_owner",
                ])
                .env("RUDROID_TEST_STARTUP_MODE", case)
                .status()
                .unwrap();
            assert!(status.success(), "{case}");
        }
        return;
    };
    let root = std::env::temp_dir().join(format!("rudroid-startup-{}", std::process::id()));
    std::fs::create_dir_all(root.join("assets/bin/Data")).unwrap();
    let source = root.join("assets/bin/Data/boot.config");
    let api = host_api::table();
    if case == "conflict" {
        let conflicting = HostConfig::parse(&format!(
            "[paths]\ngame_files={:?}\n[unity]\nrender_thread_mode='multi'\n[package.mainIntentBundle]\nunity='-force-gfx-direct'\n",
            root.to_str().unwrap()
        ))
        .unwrap();
        host_api::install_config(Some(conflicting)).unwrap();
        assert!(unity_startup_policy_from_api(api).is_err());
        // HostConfig stays immutable outside package.mainIntentBundle.<key>.
        assert!(!api.set_config_string(c"paths.game_files", c"/evil"));
        assert_eq!(
            api.config_string(c"paths.game_files", 4096).as_deref(),
            Some(root.to_str().unwrap())
        );
        std::fs::remove_dir_all(root).unwrap();
        return;
    }
    // Prepare-path overlay rejects bad boot before the Host Config is frozen.
    std::fs::write(&source, "unrecognized=1\n").unwrap();
    let perf_policy = unity_startup_policy(&config(&root, "performance", "memory_mode")).unwrap();
    native_fixture(&root, false);
    assert!(BootView::prepare(&source, &perf_policy).is_err());
    native_fixture(&root, true);
    assert!(BootView::prepare(&source, &perf_policy).is_err());
    std::fs::write(&source, BOOT).unwrap();
    let published;
    if case == "race" {
        let start = std::sync::Arc::new(std::sync::Barrier::new(12));
        let threads: Vec<_> = (0..12)
            .map(|n| {
                let start = start.clone();
                let root = root.clone();
                std::thread::spawn(move || {
                    start.wait();
                    host_api::install_config(Some(config(
                        &root,
                        if n % 2 == 0 {
                            "performance"
                        } else {
                            "low_memory"
                        },
                        "memory_mode",
                    )))
                    .unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        published = publish_boot_overlay(api, &source).unwrap();
    } else {
        let (mode, rendering) = if case == "unknown-engine" {
            native_fixture(&root, false);
            ("performance", "engine")
        } else if case == "override" {
            ("performance", "single")
        } else {
            (case.as_str(), "memory_mode")
        };
        let cfg = config(&root, mode, rendering);
        host_api::install_config(Some(cfg)).unwrap();
        published = publish_boot_overlay(api, &source).unwrap();
    }
    // Second publish must be rejected (first-write-wins overlay + cmdline).
    let thread = unity_startup_policy_from_api(api).unwrap().render_thread();
    if thread != UnityRenderThread::Engine {
        assert!(publish_boot_overlay(api, &source).is_err());
        assert!(!api.set_config_string(c"package.mainIntentBundle.unity", c"-clobber"));
    }
    let prefix = format!("rudroid-boot-{}-", std::process::id());
    assert_eq!(
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .count(),
        0
    );
    let wanted = match thread {
        UnityRenderThread::Engine => BOOT.to_owned(),
        UnityRenderThread::Single => BOOT.to_owned(),
        UnityRenderThread::Multi => BOOT
            .replace("force-gfx-direct=1", "force-gfx-direct=0")
            .replace("gfx-threading-mode=0", "gfx-threading-mode=1")
            .replace("gfx-disable-mt-rendering=1", "gfx-disable-mt-rendering=0"),
    };
    let owners = std::fs::read_dir("/proc/self/fd").unwrap()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|fd| std::fs::read_link(fd).is_ok_and(|path|
            path.as_os_str().as_encoded_bytes().starts_with(b"/memfd:rudroid-snapshot ")))
        .filter(|fd| std::fs::read(fd).is_ok_and(|bytes| bytes == wanted.as_bytes()))
        .count();
    assert_eq!(owners, usize::from(thread != UnityRenderThread::Engine));
    assert_eq!(
        published.is_some(),
        thread != UnityRenderThread::Engine
    );
    for kind in [BASE, MERGED] {
        let path = api
            .asset_path(kind, c"bin/Data/boot.config", None, 0)
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(path.to_str().unwrap()).unwrap(),
            wanted
        );
    }
    let abs = CString::new(source.to_str().unwrap()).unwrap();
    let native = api
        .asset_path(PATH, &abs, None, 0)
        .unwrap()
        .map(|s| PathBuf::from(s.to_str().unwrap()))
        .unwrap_or_else(|| source.clone());
    assert_eq!(std::fs::read_to_string(native).unwrap(), wanted);
    let jar = CString::new(format!(
        "jar:file:{}!/assets/bin/Data/boot.config",
        root.join("base.apk").display()
    ))
    .unwrap();
    let alias = api.asset_path(PATH, &jar, None, 0).unwrap().unwrap();
    assert_eq!(
        std::fs::read_to_string(alias.to_str().unwrap()).unwrap(),
        wanted
    );
    assert!(api
        .asset_path(PATH, c"/proc/cpuinfo", None, 0)
        .unwrap()
        .is_none());
    let args = api
        .config_string(c"package.mainIntentBundle.unity", 4096)
        .unwrap();
    assert!(args.starts_with("-force-gles30"));
    match thread {
        UnityRenderThread::Engine => assert_eq!(args, "-force-gles30"),
        UnityRenderThread::Single => {
            assert!(args.ends_with("-force-gfx-direct"))
        }
        UnityRenderThread::Multi => assert!(args.ends_with("-force-gfx-mt")),
    }
    if case == "race" {
        assert_eq!(
            thread == UnityRenderThread::Multi,
            host_api::memory_policy().mode
                == rudroid_common::runtime_policy::MemoryMode::Performance
        );
    }
    assert_eq!(std::fs::read_to_string(&source).unwrap(), BOOT);
    drop(published);
    std::fs::remove_dir_all(root).unwrap();
}
