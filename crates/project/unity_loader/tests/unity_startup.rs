//! INPUT:  fresh loader processes, three memory modes and immutable extracted boot fixtures
//! OUTPUT:  shared Java/NDK/native policy projection, retry and concurrent publication checks
//! POS:    verifies startup ownership and effective values without executing game code
use rudroid_common::android_assets::{BASE, MERGED, PATH};
use rudroid_host_config::HostConfig;
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
    // A failed explicit projection must not freeze a partial config/resource service.
    std::fs::write(&source, "unrecognized=1\n").unwrap();
    assert!(host_api::install_config(Some(config(&root, "performance", "memory_mode"))).is_err());
    native_fixture(&root, false);
    assert!(host_api::install_config(Some(config(&root, "performance", "memory_mode"))).is_err());
    native_fixture(&root, true);
    assert!(host_api::install_config(Some(config(&root, "performance", "memory_mode"))).is_err());
    std::fs::write(&source, BOOT).unwrap();
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
    } else {
        let (mode, rendering) = if case == "unknown-engine" {
            native_fixture(&root, false);
            ("performance", "engine")
        } else if case == "override" {
            ("performance", "single")
        } else {
            (case.as_str(), "memory_mode")
        };
        host_api::install_config(Some(config(&root, mode, rendering))).unwrap();
    }
    let api = host_api::table();
    let thread = host_api::unity_render_thread();
    let prefix = format!("rudroid-boot-{}-", std::process::id());
    let owners = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|p| p.to_string_lossy().contains(&prefix))
        .count();
    assert_eq!(
        owners,
        usize::from(thread != rudroid_host_config::UnityRenderThread::Engine)
    );
    assert_eq!(
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .count(),
        0
    );
    let wanted = match thread {
        rudroid_host_config::UnityRenderThread::Engine => BOOT.to_owned(),
        rudroid_host_config::UnityRenderThread::Single => BOOT.to_owned(),
        rudroid_host_config::UnityRenderThread::Multi => BOOT
            .replace("force-gfx-direct=1", "force-gfx-direct=0")
            .replace("gfx-threading-mode=0", "gfx-threading-mode=1")
            .replace("gfx-disable-mt-rendering=1", "gfx-disable-mt-rendering=0"),
    };
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
        rudroid_host_config::UnityRenderThread::Engine => assert_eq!(args, "-force-gles30"),
        rudroid_host_config::UnityRenderThread::Single => {
            assert!(args.ends_with("-force-gfx-direct"))
        }
        rudroid_host_config::UnityRenderThread::Multi => assert!(args.ends_with("-force-gfx-mt")),
    }
    if case == "race" {
        assert_eq!(
            thread == rudroid_host_config::UnityRenderThread::Multi,
            host_api::memory_policy().mode
                == rudroid_common::runtime_policy::MemoryMode::Performance
        );
    }
    assert_eq!(std::fs::read_to_string(&source).unwrap(), BOOT);
    std::fs::remove_dir_all(root).unwrap();
}
