//! INPUT:  guest-plan builder helpers and ModuleKind deferral state
//! OUTPUT:  path rejection, defer dedupe and empty-defer contract checks
//! POS:    keeps unit bodies out of `src/guest_plan.rs`
use super::*;
use rudroid_common::registry::HostStr;
use std::ffi::CString;

#[test]
fn valid_preload_path_rejects_escapes() {
    assert!(valid_preload_path("lib/arm64-v8a/libmain.so"));
    assert!(!valid_preload_path("../escape.so"));
    assert!(!valid_preload_path("/abs/lib.so"));
    assert!(!valid_preload_path("a/./b.so"));
    assert!(!valid_preload_path(""));
}

#[test]
fn builder_add_entry_rejects_unsafe_paths() {
    let mut ctx = PlanBuildCtx {
        entries: Vec::new(),
        reserved_paths: Vec::new(),
        defer_jni_on_load: Vec::new(),
        error: None,
    };
    for bad in ["../escape.so", "/abs/lib.so", "a/./b.so"] {
        let path = CString::new(bad).unwrap();
        assert_eq!(
            builder_add_entry(
                &mut ctx as *mut _ as *mut c_void,
                HostStr::new(&path),
                0,
                1,
                0,
                0,
            ),
            -1
        );
        assert!(ctx.error.as_ref().is_some_and(|e| e.contains("relative")));
        ctx.error = None;
    }
    let ok = CString::new("lib/arm64-v8a/libgame.so").unwrap();
    assert_eq!(
        builder_add_entry(
            &mut ctx as *mut _ as *mut c_void,
            HostStr::new(&ok),
            ModuleKind::Main as u32,
            1,
            0,
            0,
        ),
        0
    );
    assert_eq!(ctx.entries.len(), 1);
}

#[test]
fn defer_jni_on_load_dedupes_multiple_roles() {
    let mut ctx = PlanBuildCtx {
        entries: Vec::new(),
        reserved_paths: Vec::new(),
        defer_jni_on_load: Vec::new(),
        error: None,
    };
    assert_eq!(
        builder_set_defer_jni_on_load(
            &mut ctx as *mut _ as *mut c_void,
            ModuleKind::Unity as u32
        ),
        0
    );
    assert_eq!(
        builder_set_defer_jni_on_load(
            &mut ctx as *mut _ as *mut c_void,
            ModuleKind::Il2cpp as u32
        ),
        0
    );
    assert_eq!(
        builder_set_defer_jni_on_load(
            &mut ctx as *mut _ as *mut c_void,
            ModuleKind::Unity as u32
        ),
        0
    );
    assert_eq!(
        ctx.defer_jni_on_load,
        vec![ModuleKind::Unity, ModuleKind::Il2cpp]
    );
}

#[test]
fn guest_plan_defers_only_listed_kinds_even_if_libunity_exists_on_disk() {
    // Host guest bring-up must consult plan.defer_jni_on_load only — a
    // non-Unity engine with a stray libunity.so must not inherit deferral.
    let plan = GuestPlan {
        game_files: PathBuf::from("/tmp"),
        preload: Vec::new(),
        entries: Vec::new(),
        reserved_paths: Vec::new(),
        defer_jni_on_load: Vec::new(),
    };
    assert!(!plan.defer_jni_on_load.contains(&ModuleKind::Unity));
    assert!(!plan.defer_jni_on_load.contains(&ModuleKind::Il2cpp));
}
