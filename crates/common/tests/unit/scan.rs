//! INPUT:  private items of `src/scan.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/scan.rs`; declared there with `#[path]`
use super::*;

fn write(path: &Path) {
    std::fs::write(path, b"elf").unwrap();
}

#[test]
fn stem_strips_lib_prefix_and_extension() {
    assert_eq!(library_stem("liblog_stderr.so"), "log_stderr");
    assert_eq!(library_stem("log_stderr.dylib"), "log_stderr");
    assert_eq!(library_stem("loader"), "loader");
}

#[test]
fn is_capability_library_accepts_so_and_dylib() {
    let dir = std::env::temp_dir().join(format!("rudroid-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let so = dir.join("liba.so");
    write(&so);
    assert!(is_capability_library(&so));
    let dylib = dir.join("liba.dylib");
    write(&dylib);
    assert!(is_capability_library(&dylib));
    let txt = dir.join("catalog.bin");
    write(&txt);
    assert!(!is_capability_library(&txt));
    assert!(!is_capability_library(&dir.join("noext")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scan_lists_root_and_convention_subdirs_sorted() {
    let dir = std::env::temp_dir().join(format!("rudroid-scan2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    std::fs::create_dir_all(dir.join("gamepatchs")).unwrap();
    std::fs::create_dir_all(dir.join("ignored")).unwrap();

    write(&dir.join("libzeta.dylib"));
    write(&dir.join("catalog.bin"));
    write(&dir.join("plugins/libandroid_base.so"));
    write(&dir.join("plugins/libalpha.so"));
    write(&dir.join("gamepatchs/libgame_patch.dylib"));
    std::fs::write(dir.join("ignored/libskip.dylib"), b"x").unwrap();

    let entries = scan_deploy_dir(&dir).unwrap();
    // Root first, then convention subdirs; alphabetical within each
    // location. Final load order is decided by descriptor validation
    // (dependency sort), not by the scan.
    let stems: Vec<&str> = entries.iter().map(|e| e.stem.as_str()).collect();
    assert_eq!(stems, ["zeta", "alpha", "android_base", "game_patch"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_dir_is_an_error() {
    let err = scan_deploy_dir(Path::new("/definitely/not/here")).unwrap_err();
    assert!(matches!(err, DeployScanError::MissingDir(_)));
    assert!(err.to_string().contains("deploy dir"));
}
