//! Ticket 72: the guest bring-up resolves `rudroid_elf_last_error` like
//! every other elf_load C API entry.
//!
//! Kits are same-build only (ticket 57): a deployed `android_elf_load` has
//! already passed the loader's build-id check by the time bring-up runs, so
//! a kit member without the ticket-65 error channel is a foreign or stale
//! build. It must fail bring-up on the same `GuestError::Symbol` channel as
//! any other missing C API symbol — never a silent degrade to the generic
//! load-failure reason (the R2 Standards-4 gap).

use std::path::{Path, PathBuf};
use rudroid_common::symbols;
use rudroid_test_support::cargo::{ensure_workspace_cdylib, lib_name};
use rudroid_unity_loader::{guest, load_deploy_dir, Selection};

/// Build the workspace elf_load cdylib into THIS run's target dir (shared
/// 73/75 helper: matching profile, always rebuilt so a stale kit member is
/// never copied into a fixture).
fn build_elf_load_capability() -> PathBuf {
    ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load")
}

struct Kit {
    dir: PathBuf,
}

/// Deploy a kit whose gamedata holds unparseable guest modules: `from_path`
/// fails on libmain.so, so the ticket-65 error channel is the only source of
/// a specific reason (and the Symbol resolution happens before any mapping).
fn deploy_kit(name: &str, elf_load_source: &Path) -> Kit {
    let dir = std::env::temp_dir().join(format!("rudroid-t72-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(elf_load_source, dir.join(lib_name("android_elf_load"))).unwrap();
    let lib_dir = dir.join("gamedata/lib/arm64-v8a");
    std::fs::create_dir_all(&lib_dir).unwrap();
    std::fs::write(lib_dir.join("libmain.so"), [0u8; 512]).unwrap();
    std::fs::write(lib_dir.join("libil2cpp.so"), [0u8; 512]).unwrap();
    Kit { dir }
}

fn bring_up_kit(kit: &Kit) -> Result<Vec<guest::ModuleEvent>, guest::GuestError> {
    let config = rudroid_host_config::HostConfig::parse(&format!(
        "[device]\ndisplayWidth = 1234\n[paths]\ngame_files = \"{}\"\n",
        kit.dir.join("gamedata").display()
    ))
    .unwrap();
    let plan = guest::GuestPlan::from_config(&config).expect("plan");
    let capabilities = unsafe { load_deploy_dir(&kit.dir, &Selection::All) }.expect(
        "the kit itself must load: descriptor and build id are intact \
         without the error channel",
    );
    let result = unsafe { guest::bring_up(&capabilities, &plan) };
    // Handles stay open (pinned) for process lifetime (ticket 46).
    std::mem::forget(capabilities);
    result
}

/// Ticket 72 guard: a kit member that passes the same-build gate but lacks
/// `rudroid_elf_last_error` fails bring-up on the `GuestError::Symbol`
/// channel, naming the capability and the missing symbol. Before ticket 72
/// this exact kit loaded with `last_error = None` and degraded silently.
///
/// The variant is manufactured by renaming the export in a COPY of the
/// freshly built cdylib: same-length in-place surgery on the dynamic string
/// table (cli.rs-style fixture surgery). Offsets stay valid, hash tables are
/// untouched, so dlsym of the original name simply stops matching; the copy
/// lives in the temp deploy dir only — never in the tree or the target dir.
#[cfg(target_os = "linux")]
#[test]
fn missing_last_error_fails_bring_up_with_capability_and_reason() {
    const EXPORTED: &[u8] = b"rudroid_elf_last_error";
    const RENAMED: &[u8] = b"rudroid_elf_last_errox";

    let real = build_elf_load_capability();
    let kit = deploy_kit("missing-channel", &real);
    let deployed = kit.dir.join(lib_name("android_elf_load"));
    let mut bytes = std::fs::read(&deployed).unwrap();
    let mut renamed = 0;
    let mut i = 0;
    while i + EXPORTED.len() <= bytes.len() {
        if &bytes[i..i + EXPORTED.len()] == EXPORTED {
            bytes[i..i + EXPORTED.len()].copy_from_slice(RENAMED);
            renamed += 1;
            i += EXPORTED.len();
        } else {
            i += 1;
        }
    }
    assert!(
        renamed >= 1,
        "the exported symbol name must appear in the cdylib image"
    );
    std::fs::write(&deployed, &bytes).unwrap();

    let err = bring_up_kit(&kit).expect_err(
        "bring-up must fail when the same-build kit member lacks the \
         ticket-65 error channel",
    );
    match &err {
        guest::GuestError::Symbol { capability, reason } => {
            assert_eq!(*capability, "android_elf_load");
            assert!(
                reason.contains(symbols::elf_load::LAST_ERROR),
                "the reason must name the missing symbol: {reason}"
            );
        }
        other => panic!("expected GuestError::Symbol, got: {other:?}"),
    }
    let text = err.to_string();
    assert!(
        text.contains("android_elf_load") && text.contains(symbols::elf_load::LAST_ERROR),
        "the actionable message needs capability and reason: {text}"
    );
    let _ = std::fs::remove_dir_all(&kit.dir);
}

/// Ticket 65 regression at the guest level: with a healthy same-build kit
/// member, a guest mapping failure still surfaces the capability's specific
/// parse diagnostic through the error channel — not the generic fallback
/// reason a silent degrade would produce.
#[test]
fn same_build_kit_surfaces_specific_elf_diagnostics() {
    let real = build_elf_load_capability();
    let kit = deploy_kit("channel-intact", &real);
    let err = bring_up_kit(&kit)
        .expect_err("an unparseable required guest module must fail the load");
    match &err {
        guest::GuestError::LoadFailed {
            path,
            required: true,
            reason,
        } => {
            assert!(
                path.ends_with("lib/arm64-v8a/libmain.so"),
                "the failing module is named: {}",
                path.display()
            );
            assert!(
                reason.contains("magic"),
                "the ticket-65 diagnostic must survive to the guest error, \
                 got the generic fallback instead: {reason}"
            );
        }
        other => panic!("expected LoadFailed for libmain.so, got: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&kit.dir);
}
