//! INPUT:  fresh loader processes and synthetic runtime-mode configurations
//! OUTPUT:  shared Host API defaults, overrides and rejected malformed policies
//! POS:    verifies capabilities observe the loader's one resolved memory policy

use rudroid_common::registry::HostStr;
use rudroid_host_config::HostConfig;
use rudroid_unity_loader::host_api;

#[test]
fn memory_policy_reaches_host_api() {
    if let Ok(mode) = std::env::var("RUDROID_TEST_MEMORY_MODE") {
        let (bytes, mib, preload) = match mode.as_str() {
            "low_memory" => (0, 0, 0),
            "balanced" => (8192, 1, 1),
            "performance" => (65536, 16, 1),
            "override" => (4096, 0, 0),
            _ => unreachable!(),
        };
        let text = if mode == "override" {
            "[runtime]\nmemory_mode='performance'\nstream_buffer_bytes=4096\nstream_buffer_budget_mib=0\npreload_audio=false\n".to_owned()
        } else {
            format!("[runtime]\nmemory_mode={mode:?}\n")
        };
        assert!(host_api::install_config(Some(
            HostConfig::parse("[runtime]\nmemory_mode='typo'\n").unwrap()
        ))
        .is_err());
        host_api::install_config(Some(HostConfig::parse(&text).unwrap())).unwrap();
        let api = host_api::table();
        assert_eq!(
            (api.config_get_i64)(HostStr::new(c"runtime.stream_buffer_bytes"), -1),
            bytes
        );
        assert_eq!(
            (api.config_get_i64)(HostStr::new(c"runtime.stream_buffer_budget_mib"), -1),
            mib
        );
        assert_eq!(
            (api.config_get_bool)(HostStr::new(c"runtime.preload_audio"), -1),
            preload
        );
        return;
    }
    for mode in ["low_memory", "balanced", "performance", "override"] {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "memory_policy_reaches_host_api"])
            .env("RUDROID_TEST_MEMORY_MODE", mode)
            .status()
            .unwrap();
        assert!(status.success(), "{mode}");
    }
}
