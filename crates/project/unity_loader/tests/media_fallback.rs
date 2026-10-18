//! INPUT:  production ELF/framework/Bionic/media cdylibs and self-compiled bare-import C guest
//! OUTPUT:  actual data/function relocation, permanent errors and failed-bring-up source rollback
//! POS:    validates the optional media failure boundary without linking a capability rlib
#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU; use docker-rust.sh test"]
fn media_fallback_guest() { panic!("requires the ARM64 guest execution backend"); }

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[test]
fn media_fallback_guest() {
    use rudroid_common::{symbols, ModuleKind};
    use rudroid_test_support::cargo::ensure_workspace_cdylib;
    use rudroid_unity_loader::{guest, load_deploy_dir, runtime, Selection};
    use std::{ffi::{c_char,c_void,CString},fs,os::fd::AsRawFd,process::Command};
    let root = std::env::temp_dir().join(format!("rudroid-media-guest-{}",std::process::id()));
    let kit = root.join("kit"); fs::create_dir_all(&kit).unwrap();
    for (package,artifact) in [
        ("rudroid-android-framework","android_base"),
        ("rudroid-android-elf-load","android_elf_load"),
        ("rudroid-android-bionic","android_bionic"),
        ("rudroid-android-media-fallback","android_media_fallback"),
    ] {
        let path=ensure_workspace_cdylib(package,artifact);
        fs::copy(&path,kit.join(path.file_name().unwrap())).unwrap();
    }
    let compile = |source: &str, output: &str| {
        let result=Command::new("gcc").args(["-shared","-fPIC","-nostdlib","-fno-builtin",
            "-fno-stack-protector","-O2",source,"-o",output]).current_dir(&root).output().unwrap();
        assert!(result.status.success(),"gcc failed: {}",String::from_utf8_lossy(&result.stderr));
    };
    fs::write(root.join("media.c"),include_str!("fixtures/media_fallback.c")).unwrap();
    compile("media.c","guest.so");
    fs::write(root.join("bad.c"),"extern const char *AMEDIAFORMAT_KEY_DURATION; extern void media_not_supported(void); void probe(void) { if (AMEDIAFORMAT_KEY_DURATION) media_not_supported(); }\n").unwrap();
    compile("bad.c","bad.so");
    let capabilities=unsafe { load_deploy_dir(&kit,&Selection::All) }.unwrap();
    let plan=guest::GuestPlan { game_files:root.clone(), entries:vec![guest::GuestEntry {
        kind:Some(ModuleKind::Main),relative_path:"guest.so",required:true,gates:&[],
    }] };
    let mut bad=plan.clone(); bad.entries[0].relative_path="bad.so";
    let error=unsafe { guest::bring_up(&capabilities,&bad) }.unwrap_err().to_string();
    assert!(error.contains("media_not_supported"),"{error}");
    let elf=capabilities.iter().find(|c| c.descriptor.id=="android_elf_load").unwrap();
    unsafe {
        let load:libloading::Symbol<unsafe extern "C" fn(*const c_char)->*mut c_void>=
            elf.library().get(symbols::elf_exec::EXEC_LOAD.as_bytes()).unwrap();
        let path=CString::new(root.join("guest.so").to_str().unwrap()).unwrap();
        assert!(load(path.as_ptr()).is_null(),"failure must revoke the media source");
    }
    let events=unsafe { guest::bring_up(&capabilities,&plan) }.unwrap();
    let input=fs::File::create(root.join("input.mp4")).unwrap();
    unsafe {
        let address=runtime::guest_exec_symbol(events[0].base,"rudroid_media_probe").unwrap();
        let probe:unsafe extern "C" fn(i32)->i32=std::mem::transmute(address);
        for _ in 0..2000 { assert_eq!(probe(input.as_raw_fd()),0,"typed NDK failure path"); }
    }
    assert!(input.metadata().is_ok(),"failed datasource setup must not adopt or close caller fd");
    fs::remove_dir_all(root).unwrap();
}
