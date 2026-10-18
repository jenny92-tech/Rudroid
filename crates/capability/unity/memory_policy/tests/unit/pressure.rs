//! INPUT:  private items of `src/pressure.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/pressure.rs`; declared there with `#[path]`
use super::*;
#[test]
fn idle_sampling_reads_only_allocation_limits_and_preserves_v1_memsw() {
    let source = MemorySource {
        limits: vec![CgroupLimit {
            memory: ("/cg/memory.limit".into(), "/cg/memory.current".into()),
            swap: Some(("/cg/swap.max".into(), "/cg/swap.current".into())),
            memsw: Some(("/cg/memsw.max".into(), "/cg/memsw.current".into())),
        }],
        cgroup_psi: Some("/cg/memory.pressure".into()),
    };
    let mut reads = Vec::new();
    let sample = source
        .sample_with(|path| {
            reads.push(path.to_owned());
            Ok(match path.to_str().unwrap() {
                "/proc/meminfo" => "MemAvailable: 1024 kB\nSwapFree: 4096 kB\n",
                "/cg/memory.limit" => "262144",
                "/cg/memory.current" => "0",
                "/cg/memsw.max" => "65536",
                "/cg/memsw.current" => "32768",
                other => panic!("unexpected idle diagnostic I/O: {other}"),
            }
            .into())
        })
        .unwrap();
    assert_eq!(reads.len(), 5);
    assert_eq!(sample.available_kib, 32);
    assert_eq!(sample.cgroup_headroom_kib, Some(256));
    assert_eq!(sample.cgroup_memsw_headroom_kib, Some(32));
    assert_eq!(sample.host_swap_free_kib, Some(4096));
    assert!(sample.cgroup_swap_headroom_kib.is_none() && sample.psi.is_none());
}
#[test]
fn warmup_hysteresis_cooldown_and_single_pending_request() {
    let mut gate = PressureGate::new(64, 128, Duration::from_secs(30), Duration::from_secs(5));
    assert!(!gate.should_start(256, Duration::from_secs(10)));
    assert!(!gate.should_start(32, Duration::from_secs(1)));
    assert!(gate.should_start(32, Duration::from_secs(5)));
    gate.started();
    assert!(!gate.should_start(0, Duration::from_secs(100)));
    gate.completed(Duration::from_secs(101));
    assert!(!gate.should_start(0, Duration::from_secs(130)));
    assert!(gate.should_start(80, Duration::from_secs(131)));
    assert!(!gate.should_start(128, Duration::from_secs(131)));
    assert!(!gate.should_start(80, Duration::from_secs(140)));
    assert!(gate.should_start(64, Duration::from_secs(140)));
}
#[test]
fn older_kernel_fallback_and_invalid_units() {
    assert_eq!(meminfo("MemFree: 40 kB\nMemAvailable: 120 kB\n"), Some(120));
    assert_eq!(meminfo("MemFree: 40 kB\n"), Some(40));
    assert_eq!(meminfo("MemAvailable: invalid kB\n"), None);
    assert_eq!(meminfo("MemAvailable: 100 MB\n"), None);
}
#[test]
fn cgroups_include_ancestors_and_handle_subtree_mounts() {
    let mounts = "1 0 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n";
    let limits = cgroup_limits(mounts, "0::/a/b\n");
    assert_eq!(limits.len(), 3);
    assert_eq!(
        limits[0].memory.0,
        Path::new("/sys/fs/cgroup/a/b/memory.max")
    );
    assert_eq!(limits[2].memory.0, Path::new("/sys/fs/cgroup/memory.max"));
    assert_eq!(
        limits[0].swap.as_ref().unwrap().0,
        Path::new("/sys/fs/cgroup/a/b/memory.swap.max")
    );
    assert_eq!(
        cgroup_limits("1 0 0:1 /a /cg rw - cgroup2 cgroup rw\n", "0::/a/b\n").len(),
        2
    );
    assert_eq!(
        cgroup_limits(
            "1 0 0:1 / /cg rw - cgroup cg rw,memory\n",
            "5:memory:/game\n"
        )[0]
        .memory
        .1,
        Path::new("/cg/game/memory.usage_in_bytes")
    );
    assert!(cgroup_limits(mounts, "0::/../../secret\n").is_empty());
}
#[test]
fn swap_is_separate_from_ram_and_v1_combined_limits() {
    let text = "MemAvailable: 40 kB\nSwapTotal: 2048 kB\nSwapFree: 1024 kB\n";
    assert_eq!(meminfo(text), Some(40));
    assert_eq!(kib_field(text, "SwapFree:"), Some(1024));
    assert_eq!(kib_field("SwapFree: 0 kB\n", "SwapFree:"), Some(0));
    assert_eq!(kib_field("MemAvailable: 40 kB\n", "SwapFree:"), None);
    assert_eq!(headroom("max", "123"), None);
    assert_eq!(headroom("0", "0"), Some(0));
    assert_eq!(headroom("1024", "2048"), Some(0));
    assert_eq!(minimum([2048, 512, 1024].into_iter()), Some(512));
    let levels = cgroup_limits(
        "1 0 0:1 / /cg rw - cgroup cg rw,memory\n",
        "5:memory:/game\n",
    );
    assert!(levels[0].swap.is_none());
    assert_eq!(
        levels[0].memsw.as_ref().unwrap().0,
        Path::new("/cg/game/memory.memsw.limit_in_bytes")
    );
}
#[test]
fn combined_v1_budget_can_block_allocations_while_ram_has_room() {
    assert_eq!(allocation_headroom(1024, Some(256), Some(32)), 32);
    assert_eq!(allocation_headroom(1024, Some(256), Some(0)), 0);
    assert_eq!(allocation_headroom(1024, Some(256), None), 256);
    assert_eq!(allocation_headroom(128, Some(256), Some(512)), 128);
    assert_eq!(allocation_headroom(128, None, None), 128);
}
#[test]
fn psi_keys_units_missing_interfaces_and_old_kernel_residency() {
    let sample = psi(
        "some total=42 avg10=2.50 avg60=0.00\nfull avg10=1.25 total=9\n",
        "cgroup",
    )
    .unwrap();
    assert_eq!(sample.scope, "cgroup");
    assert_eq!(sample.some_avg10, Some(2.5));
    assert_eq!(sample.full_avg10, Some(1.25));
    assert!(psi("full avg10=NaN\nsome avg10=101\n", "host").is_none());
    assert!(psi("", "host").is_none());
    let old = parse_residency("", "VmRSS: 100 kB\nVmSwap: 50 kB\n");
    assert_eq!(old.rss_kib, Some(100));
    assert_eq!(old.swap_kib, Some(50));
    assert_eq!(old.pss_kib, None);
    assert_eq!(old.swap_pss_kib, None);
    let current = parse_residency("Rss: 100 kB\nPss: 80 kB\nSwap: 50 kB\nSwapPss: 25 kB\n", "");
    assert_eq!(current.pss_kib, Some(80));
    assert_eq!(current.swap_pss_kib, Some(25));
    assert_eq!(kib_field("VmSwap: 50 MB\n", "VmSwap:"), None);
}
