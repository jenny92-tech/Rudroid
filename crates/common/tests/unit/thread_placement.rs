//! INPUT:  asymmetric CPU/status fixtures and an owned Linux test thread
//! OUTPUT:  topology, caller-ownership, fallback and native self-affinity checks
//! POS:    verifies that placement never targets the leader or an external TID
use super::*;

#[test]
fn measured_topology_is_selected_without_fixed_cpu_numbers() {
    let plan = ThreadPlacement::from_topology(&[(9, 1024), (5, 604), (2, 604)]).unwrap();
    assert_eq!(plan.primary_cpu(), 9);
    assert_eq!(plan.worker_cpus(), &[2, 5]);
    for unsupported in [vec![], vec![(0, 604), (1, 604)],
        vec![(0, 604), (1, 604), (4, 604)],
        vec![(0, 604), (4, 1024), (5, 1024)],
        vec![(0, 500), (1, 604), (4, 1024)],
        vec![(0, 604), (1, 604), (2, 604), (4, 1024)],
        vec![(0, 0), (1, 604), (4, 1024)],
        vec![(0, 604), (0, 604), (4, 1024)],
        vec![(0, 604), (1, 604), (CPU_LIMIT, 1024)]] {
        assert!(ThreadPlacement::from_topology(&unsupported).is_none(), "{unsupported:?}");
    }
}

#[test]
fn only_a_named_worker_sharing_the_leader_mask_is_eligible() {
    let plan = ThreadPlacement::from_topology(&[(2, 604), (5, 604), (9, 1024)]).unwrap();
    let worker = "Name:\tUnityGfxDeviceW\nTgid:\t31\nPid:\t37\n";
    let leader = "Pid:\t31\nCpus_allowed_list:\t9\n";
    assert!(plan.matches(worker, leader, &[9], "UnityGfxDeviceW"));
    assert!(!plan.matches(worker, leader, &[9], "AudioWorker"));
    assert!(!plan.matches(worker, leader, &[2, 5, 9], "UnityGfxDeviceW"));
    assert!(!plan.matches(worker, leader, &[2, 5], "UnityGfxDeviceW"));
    assert!(!plan.matches(&worker.replace("Pid:\t37", "Pid:\t31"), leader, &[9], "UnityGfxDeviceW"));
    assert!(!plan.matches(worker, &leader.replace("Pid:\t31", "Pid:\t32"), &[9], "UnityGfxDeviceW"));
    assert!(!plan.matches(worker, &leader.replace("\t9", "\t2,5,9"), &[9], "UnityGfxDeviceW"));
    assert!(!plan.matches("Name:\tUnityGfxDeviceW\n", leader, &[9], "UnityGfxDeviceW"));
}

#[cfg(target_os = "linux")]
#[test]
fn native_affinity_changes_only_the_owned_calling_thread() {
    let parent = current_allowed_cpus().unwrap();
    assert!(!parent.is_empty());
    std::thread::spawn(|| {
        struct Restore(Vec<usize>);
        impl Drop for Restore {
            fn drop(&mut self) { set_current_allowed_cpus(&self.0).expect("restore owned worker affinity"); }
        }
        let old = Restore(current_allowed_cpus().unwrap());
        assert!(set_current_allowed_cpus(&[]).is_err());
        assert!(set_current_allowed_cpus(&[CPU_LIMIT]).is_err());
        assert_eq!(current_allowed_cpus().unwrap(), old.0);
        set_current_allowed_cpus(&old.0[..1]).unwrap();
        assert_eq!(current_allowed_cpus().unwrap(), old.0[..1]);
        drop(old);
    }).join().unwrap();
    assert_eq!(current_allowed_cpus().unwrap(), parent);
}

#[test]
fn kernel_snapshots_are_bounded_and_missing_data_is_not_invented() {
    let path = std::env::temp_dir().join(format!("rudroid-cpu-status-{}", std::process::id()));
    std::fs::write(&path, vec![b'a'; 8193]).unwrap();
    assert!(bounded_text(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(bounded_text(&path).is_err());
}
