//! INPUT:  production ELF/framework/Bionic/zlib cdylibs and an independent LP64 C guest
//! OUTPUT:  real compressed round trips, callback-owned bytes, OOM/version failure and source rollback
//! POS:    validates native zlib sharing without another Rust stream/buffer/allocator
#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU; use docker-rust.sh test"]
fn zlib_guest() { panic!("requires the ARM64 guest execution backend"); }

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[test]
fn zlib_guest() {
    use rudroid_common::{symbols, ModuleKind};
    use rudroid_test_support::cargo::ensure_workspace_cdylib;
    use rudroid_unity_loader::{guest, load_deploy_dir, runtime, Selection};
    use std::{ffi::{c_char,c_void,CString},fs,process::Command};
    let root = std::env::temp_dir().join(format!("rudroid-zlib-guest-{}",std::process::id()));
    let kit = root.join("kit"); fs::create_dir_all(&kit).unwrap();
    for (package,artifact) in [
        ("rudroid-android-framework","android_base"),
        ("rudroid-android-elf-load","android_elf_load"),
        ("rudroid-android-bionic","android_bionic"),
        ("rudroid-android-zlib","android_zlib"),
    ] {
        let path=ensure_workspace_cdylib(package,artifact);
        fs::copy(&path,kit.join(path.file_name().unwrap())).unwrap();
    }
    let compile = |source: &str, output: &str| {
        let result=Command::new("gcc").args(["-shared","-fPIC","-nostdlib","-fno-builtin",
            "-fno-stack-protector","-O2",source,"-o",output]).current_dir(&root).output().unwrap();
        assert!(result.status.success(),"gcc failed: {}",String::from_utf8_lossy(&result.stderr));
    };
    fs::write(root.join("zlib.c"),include_str!("fixtures/zlib_guest.c")).unwrap();
    compile("zlib.c","guest.so");
    fs::write(root.join("bad.c"),"extern const char *zlibVersion(void); extern void zlib_not_supported(void); void probe(void) { if (zlibVersion()) zlib_not_supported(); }\n").unwrap();
    compile("bad.c","bad.so");
    let capabilities=unsafe { load_deploy_dir(&kit,&Selection::All) }.unwrap();
    let plan=guest::GuestPlan { game_files:root.clone(), entries:vec![guest::GuestEntry {
        kind:Some(ModuleKind::Main),relative_path:"guest.so",required:true,gates:&[],
    }] };
    let mut bad=plan.clone(); bad.entries[0].relative_path="bad.so";
    let error=unsafe { guest::bring_up(&capabilities,&bad) }.unwrap_err().to_string();
    assert!(error.contains("zlib_not_supported"),"{error}");
    let elf=capabilities.iter().find(|c| c.descriptor.id=="android_elf_load").unwrap();
    unsafe {
        let load:libloading::Symbol<unsafe extern "C" fn(*const c_char)->*mut c_void>=
            elf.library().get(symbols::elf_exec::EXEC_LOAD.as_bytes()).unwrap();
        let path=CString::new(root.join("guest.so").to_str().unwrap()).unwrap();
        assert!(load(path.as_ptr()).is_null(),"failure must revoke the zlib source");
    }
    let events=unsafe { guest::bring_up(&capabilities,&plan) }.unwrap();
    unsafe {
        let address=runtime::guest_exec_symbol(events[0].base,"rudroid_zlib_probe").unwrap();
        let probe:unsafe extern "C" fn(*mut u64)->i32=std::mem::transmute(address);
        let mut peak=0;
        for _ in 0..128 { assert_eq!(probe(&mut peak),0,"guest stream/callback/compression/OOM ABI"); }
        assert!(peak>0);
        println!("GUEST_ZLIB {{\"iterations\":128,\"custom_allocator_peak_requested_bytes\":{peak},\"live_after_end\":0}}");
    }
    fs::remove_dir_all(root).unwrap();
}
