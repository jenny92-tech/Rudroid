//! INPUT:  safe filesystem snapshots and temporary paths/files on Linux
//! OUTPUT:  real capacity, owned-file lifetime and explicit invalid-path regressions
//! POS:    verifies shared filesystem boundary without Android, SDL or synthetic capacities
use super::*;

#[test]
fn capacity_snapshot_has_real_blocks_and_distinguishes_reserved_space() {
    let stats = FilesystemStats::for_path(Path::new("/")).unwrap();
    assert!(stats.block_size > 0 && stats.blocks > 0);
    assert!(stats.available_blocks <= stats.free_blocks);
    assert!(stats.free_blocks <= stats.blocks);
}
#[test]
fn file_snapshot_survives_unlink_and_matches_the_same_filesystem() {
    let path = std::env::temp_dir().join(format!("rudroid-fs-owner-{}", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    let first = FilesystemStats::for_path(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let still_owned = FilesystemStats::for_file(&file).unwrap();
    assert_eq!(still_owned.block_size, first.block_size);
    assert_eq!(still_owned.blocks, first.blocks);
    assert_eq!(still_owned.noexec, first.noexec);
    assert!(FilesystemStats::for_path(&path).is_err());
}
#[test]
fn missing_or_nul_paths_fail_instead_of_reporting_a_fake_capacity() {
    let missing = std::env::temp_dir().join(format!("rudroid-missing-fs-{}", std::process::id()));
    assert!(!missing.exists());
    assert_eq!(
        FilesystemStats::for_path(&missing).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(
        FilesystemStats::for_path(Path::new("/bad\0path"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
}
